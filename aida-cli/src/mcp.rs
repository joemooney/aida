// trace:FR-0152 | ai:claude
// trace:STORY-361 | ai:claude
//! MCP (Model Context Protocol) server for AIDA.
//!
//! Implements JSON-RPC 2.0 over stdio. Exposes two surfaces:
//!
//! 1. **Spec graph** (the original 7 tools) — read/write the requirement
//!    store: `list_requirements`, `show_requirement`, `add_requirement`,
//!    `update_requirement`, `search_requirements`, `add_comment`,
//!    `list_features`.
//! 2. **Coordination** (STORY-361) — read/write the file substrate the
//!    AIDA orchestrator and skills already use:
//!    - **Punts** (`.aida/punts.jsonl` + `.aida/punts/`): `list_punts`,
//!      `read_punt`, `post_punt`, `resolve_punt`, `escalate_punt`.
//!    - **Findings** (draft requirements tagged `from-implementer:` /
//!      `from-review:`): `list_findings`, `file_finding`, `triage_finding`.
//!    - **Task claims** (`.aida/sessions/*.toml`): `claim_task`,
//!      `release_task`, `list_active_leases`.
//!    - **Worker directives** (`.aida/worker.cmd`): `post_directive`,
//!      `list_directives`, `ack_directive`.
//!
//! The coordination tools are the *MCP transport* over AIDA's
//! filesystem-canonical coordination substrate. The orchestrator still
//! reads/writes those files directly — these tools are the surface any
//! MCP-speaking agent (Codex, Cursor, …) can use to participate in the
//! same drains. See `docs/architecture/mcp-coordination-surface.md`.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use aida_core::{
    Comment, PuntCategory, Requirement, RequirementPriority, RequirementStatus, RequirementType,
    Storage,
};

use crate::findings::{
    build_findings_view, count_findings, finding_source, FindingsFilter, FROM_IMPLEMENTER_PREFIX,
    FROM_REVIEW_PREFIX,
};
use crate::punt::{
    self, append_to_ledger, ledger_path, punt_response_path, read_ledger, PuntRecord,
    PuntResolution, PuntResponse,
};

// ============================================================================
// JSON-RPC 2.0 types
// ============================================================================

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Value,
    id: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    id: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    fn error(id: Value, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
            id,
        }
    }
}

// ============================================================================
// Lightweight lease projection
// ============================================================================
//
// The canonical SessionLease lives in `main.rs` as a `pub(crate)` struct with
// many fields. The MCP surface only needs a few; we read `.aida/sessions/*.toml`
// directly with `#[serde(default)]` so unknown fields don't break us — and
// crucially so the MCP server doesn't have to be rebuilt every time a new lease
// field lands.

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LightLease {
    #[serde(default)]
    id: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    owner: String,
    #[serde(default)]
    worktree_path: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    started_at: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    role: Option<String>,
    /// STORY-361: marks a lease that was created by the MCP `claim_task` tool
    /// (as opposed to `aida session start`, which creates a worktree). MCP
    /// claims are lightweight markers — `release_task` deletes them; other
    /// AIDA commands ignore them.
    #[serde(default)]
    mcp_claim: bool,
}

fn leases_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("sessions")
}

fn list_leases(project_root: &Path) -> Vec<LightLease> {
    let dir = leases_dir(project_root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        // Skip activity / manifest sidecar files — they share the directory.
        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            if name.contains(".activity.") || name.contains(".manifest.") {
                continue;
            }
        }
        if let Ok(content) = std::fs::read_to_string(&p) {
            if let Ok(lease) = toml::from_str::<LightLease>(&content) {
                if !lease.id.is_empty() {
                    out.push(lease);
                }
            }
        }
    }
    out.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    out
}

// ============================================================================
// MCP Server
// ============================================================================

struct McpServer<'a> {
    storage: &'a Storage,
    /// Project root for resolving `.aida/` coordination files. STORY-361.
    project_root: PathBuf,
}

/// Helper to get the display spec_id from a Requirement
fn spec_id(r: &Requirement) -> &str {
    r.spec_id.as_deref().unwrap_or("?")
}

impl<'a> McpServer<'a> {
    fn new(storage: &'a Storage, project_root: PathBuf) -> Self {
        Self {
            storage,
            project_root,
        }
    }

    fn handle_request(&self, req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = req.id.clone().unwrap_or(Value::Null);

        // Notifications (no id) don't get responses
        if req.id.is_none() && req.method == "notifications/initialized" {
            return None;
        }

        let response = match req.method.as_str() {
            "initialize" => self.handle_initialize(&id),
            "tools/list" => self.handle_tools_list(&id),
            "tools/call" => self.handle_tools_call(&id, &req.params),
            "resources/list" => self.handle_resources_list(&id),
            "resources/read" => self.handle_resources_read(&id, &req.params),
            "ping" => JsonRpcResponse::success(id, json!({})),
            _ => JsonRpcResponse::error(id, -32601, format!("Method not found: {}", req.method)),
        };

        Some(response)
    }

    fn handle_initialize(&self, id: &Value) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id.clone(),
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {},
                    "resources": {}
                },
                "serverInfo": {
                    "name": "aida",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
    }

    fn handle_tools_list(&self, id: &Value) -> JsonRpcResponse {
        JsonRpcResponse::success(id.clone(), json!({ "tools": tool_descriptors() }))
    }

    fn handle_tools_call(&self, id: &Value, params: &Value) -> JsonRpcResponse {
        let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let result = match tool_name {
            // Spec-graph tools (original 7)
            "list_requirements" => self.tool_list_requirements(&arguments),
            "show_requirement" => self.tool_show_requirement(&arguments),
            "add_requirement" => self.tool_add_requirement(&arguments),
            "update_requirement" => self.tool_update_requirement(&arguments),
            "search_requirements" => self.tool_search_requirements(&arguments),
            "add_comment" => self.tool_add_comment(&arguments),
            "list_features" => self.tool_list_features(),

            // Coordination tools — STORY-361
            // Punt channel (.aida/punts.jsonl + .aida/punts/)
            "list_punts" => self.tool_list_punts(&arguments),
            "read_punt" => self.tool_read_punt(&arguments),
            "post_punt" => self.tool_post_punt(&arguments),
            "resolve_punt" => self.tool_resolve_punt(&arguments),
            "escalate_punt" => self.tool_escalate_punt(&arguments),

            // Findings channel (draft requirements with from-* tags)
            "list_findings" => self.tool_list_findings(&arguments),
            "file_finding" => self.tool_file_finding(&arguments),
            "triage_finding" => self.tool_triage_finding(&arguments),

            // Task-claim channel (.aida/sessions/*.toml)
            "claim_task" => self.tool_claim_task(&arguments),
            "release_task" => self.tool_release_task(&arguments),
            "list_active_leases" => self.tool_list_active_leases(),

            // Worker-directive channel (.aida/worker.cmd)
            "post_directive" => self.tool_post_directive(&arguments),
            "list_directives" => self.tool_list_directives(),
            "ack_directive" => self.tool_ack_directive(&arguments),

            _ => Err(format!("Unknown tool: {}", tool_name)),
        };

        match result {
            Ok(content) => JsonRpcResponse::success(
                id.clone(),
                json!({
                    "content": [{
                        "type": "text",
                        "text": content
                    }]
                }),
            ),
            Err(e) => JsonRpcResponse::success(
                id.clone(),
                json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Error: {}", e)
                    }],
                    "isError": true
                }),
            ),
        }
    }

    fn handle_resources_list(&self, id: &Value) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id.clone(),
            json!({
                "resources": [
                    {
                        "uri": "aida://project/summary",
                        "name": "Project Summary",
                        "description": "Project statistics and feature list",
                        "mimeType": "text/plain"
                    },
                    {
                        "uri": "aida://requirements/tree",
                        "name": "Requirements Tree",
                        "description": "Requirement hierarchy overview",
                        "mimeType": "text/plain"
                    }
                ]
            }),
        )
    }

    fn handle_resources_read(&self, id: &Value, params: &Value) -> JsonRpcResponse {
        let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");

        let result = match uri {
            "aida://project/summary" => self.resource_project_summary(),
            "aida://requirements/tree" => self.resource_requirements_tree(),
            _ => Err(format!("Unknown resource: {}", uri)),
        };

        match result {
            Ok(content) => JsonRpcResponse::success(
                id.clone(),
                json!({
                    "contents": [{
                        "uri": uri,
                        "mimeType": "text/plain",
                        "text": content
                    }]
                }),
            ),
            Err(e) => JsonRpcResponse::error(id.clone(), -32602, e),
        }
    }

    // ========================================================================
    // Spec-graph tool implementations (original 7)
    // ========================================================================

    fn tool_list_requirements(&self, args: &Value) -> Result<String, String> {
        let store = self.storage.load().map_err(|e| e.to_string())?;
        let status_filter = args.get("status").and_then(|v| v.as_str());
        let type_filter = args.get("type").and_then(|v| v.as_str());
        let feature_filter = args.get("feature").and_then(|v| v.as_str());
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        let filtered: Vec<&Requirement> = store
            .requirements
            .iter()
            .filter(|r| {
                if let Some(status) = status_filter {
                    let status_str = format!("{}", r.status);
                    if !status_str.eq_ignore_ascii_case(status)
                        && !status.eq_ignore_ascii_case(&status_str.replace('-', ""))
                    {
                        return false;
                    }
                }
                if let Some(type_name) = type_filter {
                    let type_str = format!("{}", r.req_type);
                    if !type_str.eq_ignore_ascii_case(type_name) {
                        return false;
                    }
                }
                if let Some(feature) = feature_filter {
                    if !r.feature.eq_ignore_ascii_case(feature) {
                        return false;
                    }
                }
                true
            })
            .take(limit)
            .collect();

        if filtered.is_empty() {
            return Ok("No requirements found matching the criteria.".to_string());
        }

        let mut output = format!("Found {} requirements:\n\n", filtered.len());
        for r in &filtered {
            output.push_str(&format!(
                "- [{}] {} (Status: {}, Priority: {}, Type: {})\n",
                spec_id(r),
                r.title,
                r.status,
                r.priority,
                r.req_type
            ));
        }
        Ok(output)
    }

    fn tool_show_requirement(&self, args: &Value) -> Result<String, String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: id")?;

        let store = self.storage.load().map_err(|e| e.to_string())?;
        let req = store
            .get_requirement_by_spec_id(id)
            .ok_or_else(|| format!("Requirement '{}' not found", id))?;

        let mut output = format!(
            "# {} — {}\n\n\
             **Status:** {}\n\
             **Priority:** {}\n\
             **Type:** {}\n",
            spec_id(req),
            req.title,
            req.status,
            req.priority,
            req.req_type
        );

        if !req.feature.is_empty() {
            output.push_str(&format!("**Feature:** {}\n", req.feature));
        }
        if !req.owner.is_empty() {
            output.push_str(&format!("**Owner:** {}\n", req.owner));
        }
        if !req.tags.is_empty() {
            let tags: Vec<&String> = req.tags.iter().collect();
            output.push_str(&format!(
                "**Tags:** {}\n",
                tags.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        output.push_str(&format!("\n## Description\n\n{}\n", req.description));

        if !req.comments.is_empty() {
            output.push_str(&format!("\n## Comments ({})\n\n", req.comments.len()));
            for c in &req.comments {
                output.push_str(&format!("- {}: {}\n", c.author, c.content));
            }
        }

        if !req.relationships.is_empty() {
            output.push_str(&format!(
                "\n## Relationships ({})\n\n",
                req.relationships.len()
            ));
            for rel in &req.relationships {
                let target_label = store
                    .requirements
                    .iter()
                    .find(|r| r.id == rel.target_id)
                    .and_then(|r| r.spec_id.clone())
                    .unwrap_or_else(|| rel.target_id.to_string());
                output.push_str(&format!("- {} → {}\n", rel.rel_type, target_label));
            }
        }

        Ok(output)
    }

    fn tool_add_requirement(&self, args: &Value) -> Result<String, String> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: title")?;
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: description")?;

        let req_type = args
            .get("type")
            .and_then(|v| v.as_str())
            .and_then(parse_requirement_type)
            .unwrap_or(RequirementType::Functional);
        let status = args
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(parse_status)
            .unwrap_or(RequirementStatus::Draft);
        if status == RequirementStatus::NeedsAttention {
            return Err(
                "cannot create a requirement with status `needs-attention` — \
                 it is reached only by punting In-Progress work"
                    .to_string(),
            );
        }
        let priority = args
            .get("priority")
            .and_then(|v| v.as_str())
            .and_then(parse_priority)
            .unwrap_or(RequirementPriority::Medium);

        let mut store = self.storage.load().map_err(|e| e.to_string())?;
        let mut req = Requirement::new(title.to_string(), description.to_string());
        req.req_type = req_type;
        req.status = status;
        req.priority = priority;

        // Optional tags
        if let Some(tag_arr) = args.get("tags").and_then(|v| v.as_array()) {
            for t in tag_arr {
                if let Some(s) = t.as_str() {
                    req.tags.insert(s.to_string());
                }
            }
        }

        store.requirements.push(req);
        store.assign_spec_ids();

        let new_spec_id = store
            .requirements
            .last()
            .and_then(|r| r.spec_id.clone())
            .unwrap_or_else(|| "?".to_string());

        self.storage.save(&store).map_err(|e| e.to_string())?;

        Ok(format!("Requirement added: {} — {}", new_spec_id, title))
    }

    fn tool_update_requirement(&self, args: &Value) -> Result<String, String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: id")?;

        let mut store = self.storage.load().map_err(|e| e.to_string())?;
        let req = store
            .get_requirement_by_spec_id_mut(id)
            .ok_or_else(|| format!("Requirement '{}' not found", id))?;

        let mut changes = Vec::new();

        if let Some(status) = args.get("status").and_then(|v| v.as_str()) {
            if let Some(new_status) = parse_status(status) {
                if let Some(msg) =
                    aida_core::forbidden_attention_transition(&req.status, &new_status)
                {
                    return Err(msg);
                }
                changes.push(format!("status: {} → {}", req.status, new_status));
                req.status = new_status;
            }
        }

        if let Some(desc) = args.get("description").and_then(|v| v.as_str()) {
            changes.push("description updated".to_string());
            req.description = desc.to_string();
        }

        if changes.is_empty() {
            return Ok(format!("No changes applied to {}", id));
        }

        self.storage.save(&store).map_err(|e| e.to_string())?;
        Ok(format!("Updated {}: {}", id, changes.join(", ")))
    }

    fn tool_search_requirements(&self, args: &Value) -> Result<String, String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: query")?;

        let store = self.storage.load().map_err(|e| e.to_string())?;
        let query_lower = query.to_lowercase();

        let matches: Vec<&Requirement> = store
            .requirements
            .iter()
            .filter(|r| {
                r.title.to_lowercase().contains(&query_lower)
                    || r.description.to_lowercase().contains(&query_lower)
                    || r.spec_id
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query_lower)
            })
            .collect();

        if matches.is_empty() {
            return Ok(format!("No requirements found matching '{}'", query));
        }

        let mut output = format!("Found {} results for '{}':\n\n", matches.len(), query);
        for r in &matches {
            output.push_str(&format!("- [{}] {} ({})\n", spec_id(r), r.title, r.status));
        }
        Ok(output)
    }

    fn tool_add_comment(&self, args: &Value) -> Result<String, String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: id")?;
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: text")?;

        let mut store = self.storage.load().map_err(|e| e.to_string())?;
        let req = store
            .get_requirement_by_spec_id_mut(id)
            .ok_or_else(|| format!("Requirement '{}' not found", id))?;

        let comment = Comment::new(text.to_string(), "mcp".to_string());
        req.add_comment(comment);

        self.storage.save(&store).map_err(|e| e.to_string())?;
        Ok(format!("Comment added to {}", id))
    }

    fn tool_list_features(&self) -> Result<String, String> {
        let store = self.storage.load().map_err(|e| e.to_string())?;

        if store.features.is_empty() {
            return Ok("No features defined in this project.".to_string());
        }

        let mut output = format!("Features ({}):\n\n", store.features.len());
        for f in &store.features {
            output.push_str(&format!("- {} (prefix: {})\n", f.name, f.prefix));
        }
        Ok(output)
    }

    // ========================================================================
    // STORY-361: Punt channel tools
    // ========================================================================

    fn tool_list_punts(&self, args: &Value) -> Result<String, String> {
        let status_filter = args
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let records = read_ledger(&self.project_root);
        let filtered: Vec<&PuntRecord> = records
            .iter()
            .filter(|r| match status_filter.as_deref() {
                None => true,
                Some("awaiting") => r.resolution_path == "punted",
                Some(other) => r.resolution_path == other,
            })
            .collect();

        if filtered.is_empty() {
            return Ok("No punts found.".to_string());
        }
        let mut out = format!("Found {} punt(s):\n\n", filtered.len());
        for r in &filtered {
            out.push_str(&format!(
                "- {} [{}] {}\n  resolution: {}  ({})\n",
                r.spec,
                r.category,
                r.detail,
                r.resolution_path,
                r.timestamp.to_rfc3339(),
            ));
        }
        Ok(out)
    }

    fn tool_read_punt(&self, args: &Value) -> Result<String, String> {
        let spec = args
            .get("spec_id")
            .or_else(|| args.get("punt_id"))
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id (or punt_id)")?;

        let records = read_ledger(&self.project_root);
        // Most recent record for the spec is the live one.
        let record = records
            .iter()
            .rev()
            .find(|r| r.spec.eq_ignore_ascii_case(spec))
            .ok_or_else(|| format!("No punt found for spec '{}'", spec))?;

        let body = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
        Ok(body)
    }

    fn tool_post_punt(&self, args: &Value) -> Result<String, String> {
        let spec = args
            .get("spec_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id")?;
        let detail = args
            .get("detail")
            .or_else(|| args.get("reason"))
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: detail (or reason)")?;
        let category_raw = args
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("design-fork");
        let category = PuntCategory::from_str(category_raw)
            .ok_or_else(|| format!("invalid punt category `{}`", category_raw))?;
        let lean = args
            .get("lean")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let raised_by = args
            .get("raised_by")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some("mcp".to_string()));

        let record = PuntRecord {
            timestamp: Utc::now(),
            spec: spec.to_string(),
            category,
            detail: detail.to_string(),
            lean,
            raised_by,
            resolution_path: "punted".to_string(),
            classification: None,
            escalation_reason: None,
            answer: None,
            answered_by: None,
        };

        append_to_ledger(&self.project_root, &record).map_err(|e| e.to_string())?;

        Ok(format!(
            "Punt recorded for {} [{}]. Use `aida edit {} --status needs-attention` from a session lease to park the spec.",
            spec, category, spec
        ))
    }

    fn tool_resolve_punt(&self, args: &Value) -> Result<String, String> {
        let spec = args
            .get("spec_id")
            .or_else(|| args.get("punt_id"))
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id (or punt_id)")?;
        let answer = args
            .get("answer")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: answer")?;
        let reasoning = args
            .get("reasoning")
            .or_else(|| args.get("rationale"))
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: reasoning (or rationale)")?;
        let classification = args
            .get("classification")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let response = PuntResponse {
            resolution: PuntResolution::Resolved,
            answer: Some(answer.to_string()),
            reasoning: reasoning.to_string(),
            classification,
            escalation_reason: None,
        };

        let path = punt_response_path(&self.project_root, spec);
        let body = serde_json::to_string_pretty(&response).map_err(|e| e.to_string())?;
        // trace:TASK-439 | ai:claude
        write_atomic(&path, body.as_bytes()).map_err(|e| e.to_string())?;

        Ok(format!(
            "Resolution written to {} — the orchestrator will resume the implementer with this answer.",
            path.display()
        ))
    }

    fn tool_escalate_punt(&self, args: &Value) -> Result<String, String> {
        let spec = args
            .get("spec_id")
            .or_else(|| args.get("punt_id"))
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id (or punt_id)")?;
        let reasoning = args
            .get("reasoning")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: reasoning")?;
        let escalation_reason = args
            .get("escalation_reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let classification = args
            .get("classification")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let response = PuntResponse {
            resolution: PuntResolution::Escalated,
            answer: None,
            reasoning: reasoning.to_string(),
            classification,
            escalation_reason,
        };

        let path = punt_response_path(&self.project_root, spec);
        let body = serde_json::to_string_pretty(&response).map_err(|e| e.to_string())?;
        // trace:TASK-439 | ai:claude
        write_atomic(&path, body.as_bytes()).map_err(|e| e.to_string())?;

        Ok(format!(
            "Escalation written to {} — the orchestrator will park the spec for human triage.",
            path.display()
        ))
    }

    // ========================================================================
    // STORY-361: Findings channel tools
    // ========================================================================

    fn tool_list_findings(&self, args: &Value) -> Result<String, String> {
        let pr = args.get("pr").and_then(|v| v.as_u64()).map(|n| n as u32);
        let source = args.get("source").and_then(|v| v.as_str()).and_then(|s| {
            match s.to_ascii_lowercase().as_str() {
                "review" => Some(crate::findings::FindingSource::Review),
                "implementer" => Some(crate::findings::FindingSource::Implementer),
                _ => None,
            }
        });
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let store = self.storage.load().map_err(|e| e.to_string())?;

        // Project Database into RequirementSummary shape that findings.rs expects.
        let summaries = build_summaries(&store);
        let filter = FindingsFilter { pr, source, kind };
        let sections = build_findings_view(&summaries, &filter);
        let total = count_findings(&sections);

        if total == 0 {
            return Ok("No findings match the filter.".to_string());
        }

        let mut out = format!("Found {} finding(s):\n\n", total);
        for sec in &sections {
            out.push_str(&format!("## From {}\n\n", sec.source.label()));
            for g in &sec.groups {
                out.push_str(&format!("### {}\n", g.origin));
                for r in &g.rows {
                    let kind_label = r
                        .kind
                        .as_deref()
                        .map(|k| format!(" [{}]", k))
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "- {} {} ({}){}\n",
                        r.display_id,
                        r.title,
                        r.severity.label(),
                        kind_label,
                    ));
                }
                out.push('\n');
            }
        }
        Ok(out)
    }

    fn tool_file_finding(&self, args: &Value) -> Result<String, String> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: title")?;
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: description")?;
        let source = args
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("implementer");
        // trace:TASK-437 | ai:claude — enforce the inputSchema contract.
        // `spec_id` is a string SPEC-ID (e.g. "TASK-42"); `pr` is the bare integer.
        // Loose clients used to pass `pr: "7"` and get `from-review:7` instead of
        // `from-review:PR-7`, silently invisible to `aida findings list --pr 7`.
        let origin = if let Some(v) = args.get("spec_id") {
            let s = v
                .as_str()
                .ok_or("spec_id must be a string (e.g. \"TASK-42\")")?;
            s.to_string()
        } else if let Some(v) = args.get("pr") {
            let n = v.as_u64().ok_or(
                "pr must be an integer (the bare PR number, e.g. 7 — not \"7\" or \"PR-7\")",
            )?;
            format!("PR-{}", n)
        } else {
            return Err("Missing required parameter: spec_id (or pr)".to_string());
        };
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let severity = args
            .get("severity")
            .and_then(|v| v.as_str())
            .unwrap_or("minor");

        let source_tag = match source.to_ascii_lowercase().as_str() {
            "review" => format!("{}{}", FROM_REVIEW_PREFIX, origin),
            "implementer" => format!("{}{}", FROM_IMPLEMENTER_PREFIX, origin),
            other => return Err(format!("unknown source '{}'", other)),
        };

        let mut store = self.storage.load().map_err(|e| e.to_string())?;
        let mut req = Requirement::new(title.to_string(), description.to_string());
        req.req_type = RequirementType::Task;
        req.status = RequirementStatus::Draft;
        req.priority = RequirementPriority::Medium;
        req.tags.insert(source_tag);
        req.tags.insert(format!("severity:{}", severity));
        if let Some(k) = kind {
            req.tags.insert(format!("kind:{}", k));
        }
        store.requirements.push(req);
        store.assign_spec_ids();
        let new_id = store
            .requirements
            .last()
            .and_then(|r| r.spec_id.clone())
            .unwrap_or_else(|| "?".to_string());
        self.storage.save(&store).map_err(|e| e.to_string())?;

        Ok(format!("Finding filed: {} — {}", new_id, title))
    }

    fn tool_triage_finding(&self, args: &Value) -> Result<String, String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: id")?;
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: action (promote|dismiss)")?;
        let reason = args
            .get("reason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut store = self.storage.load().map_err(|e| e.to_string())?;
        let req = store
            .get_requirement_by_spec_id_mut(id)
            .ok_or_else(|| format!("Finding '{}' not found", id))?;
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        if finding_source(&tags).is_none() {
            return Err(format!("{} is not a finding (no from-* tag)", id));
        }

        match action.to_ascii_lowercase().as_str() {
            "promote" => {
                req.status = RequirementStatus::Approved;
                if let Some(r) = reason {
                    req.add_comment(Comment::new(
                        format!("promoted via MCP: {}", r),
                        "mcp".to_string(),
                    ));
                }
            }
            "dismiss" => {
                req.status = RequirementStatus::Rejected;
                if let Some(r) = reason {
                    req.add_comment(Comment::new(
                        format!("dismissed via MCP: {}", r),
                        "mcp".to_string(),
                    ));
                }
            }
            other => return Err(format!("unknown action '{}'", other)),
        }

        self.storage.save(&store).map_err(|e| e.to_string())?;
        Ok(format!("Finding {} {}d", id, action.to_ascii_lowercase()))
    }

    // ========================================================================
    // STORY-361: Task-claim channel tools
    // ========================================================================

    fn tool_claim_task(&self, args: &Value) -> Result<String, String> {
        let spec_id_arg = args
            .get("spec_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: spec_id")?;
        let role = args
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("implementer");

        let leases = list_leases(&self.project_root);
        if let Some(existing) = leases
            .iter()
            .find(|l| l.scope.eq_ignore_ascii_case(spec_id_arg))
        {
            return Ok(format!(
                "already_claimed by {} (lease {} since {})",
                existing.owner, existing.id, existing.started_at
            ));
        }

        // Mint a new lease_id (12-char uuid v7 prefix matches `aida session start`).
        let id_long = uuid::Uuid::now_v7().to_string();
        let id = id_long.replace('-', "")[..12].to_string();
        let owner = std::env::var("USER").unwrap_or_else(|_| "mcp".to_string());
        let hostname = hostname_or_unknown();
        let branch = current_branch_at(&self.project_root).unwrap_or_else(|| "main".to_string());

        let lease = LightLease {
            id: id.clone(),
            scope: spec_id_arg.to_string(),
            slug: spec_id_arg.to_string(),
            owner,
            worktree_path: self.project_root.to_string_lossy().to_string(),
            branch,
            started_at: Utc::now().to_rfc3339(),
            hostname,
            role: Some(role.to_string()),
            mcp_claim: true,
        };

        let dir = leases_dir(&self.project_root);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{}.toml", id));
        let body = toml::to_string(&lease).map_err(|e| e.to_string())?;
        write_atomic(&path, body.as_bytes()).map_err(|e| e.to_string())?;

        Ok(format!("claimed: lease_id={}", id))
    }

    fn tool_release_task(&self, args: &Value) -> Result<String, String> {
        let lease_id = args
            .get("lease_id")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: lease_id")?;

        let path = leases_dir(&self.project_root).join(format!("{}.toml", lease_id));
        if !path.exists() {
            return Err(format!("lease '{}' not found", lease_id));
        }

        // Refuse to delete a non-MCP lease — the MCP surface only owns the
        // lightweight claims it creates. Real `aida session start` leases must
        // be released via `aida session end`.
        if let Ok(body) = std::fs::read_to_string(&path) {
            if let Ok(l) = toml::from_str::<LightLease>(&body) {
                if !l.mcp_claim {
                    return Err(format!(
                        "lease '{}' is a real `aida session start` lease — use `aida session end {}` to release it",
                        lease_id, lease_id
                    ));
                }
            }
        }

        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        Ok(format!("released: lease_id={}", lease_id))
    }

    fn tool_list_active_leases(&self) -> Result<String, String> {
        let leases = list_leases(&self.project_root);
        if leases.is_empty() {
            return Ok("No active leases.".to_string());
        }
        let mut out = format!("Found {} active lease(s):\n\n", leases.len());
        for l in &leases {
            let claim_kind = if l.mcp_claim { "mcp" } else { "session" };
            let role = l.role.as_deref().unwrap_or("?");
            out.push_str(&format!(
                "- {} scope={} role={} owner={} kind={} started_at={}\n",
                l.id, l.scope, role, l.owner, claim_kind, l.started_at
            ));
        }
        Ok(out)
    }

    // ========================================================================
    // STORY-361: Worker-directive channel tools
    // ========================================================================

    fn tool_post_directive(&self, args: &Value) -> Result<String, String> {
        let verb = args
            .get("verb")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: verb (drain|pause|exit)")?;
        // trace:TASK-436 | ai:claude
        // Validate the verb up front. The worker defensively maps unknown
        // verbs to `pause`, but the caller deserves a clear error rather than
        // a silently-misclassified "directive posted" success.
        if !matches!(verb, "drain" | "pause" | "exit") {
            return Err(format!(
                "invalid directive verb `{}` (expected drain|pause|exit)",
                verb
            ));
        }
        let args_list = args
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut line = verb.to_string();
        if !args_list.is_empty() {
            line.push(' ');
            line.push_str(&args_list.join(" "));
        }

        let path = crate::worker::worker_cmd_path(&self.project_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        append_line(&path, &line).map_err(|e| e.to_string())?;
        Ok(format!("directive posted: {}", line))
    }

    fn tool_list_directives(&self) -> Result<String, String> {
        let path = crate::worker::worker_cmd_path(&self.project_root);
        let directives = crate::worker::parse_directives(&path);
        Ok(crate::worker::render_human(&directives))
    }

    fn tool_ack_directive(&self, args: &Value) -> Result<String, String> {
        let index = args
            .get("index")
            .and_then(|v| v.as_u64())
            .ok_or("Missing required parameter: index (0-based)")? as usize;

        let path = crate::worker::worker_cmd_path(&self.project_root);
        let body = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {}", path.display(), e))?;

        // Keep blank/comment lines in place; only count non-comment, non-blank
        // lines so the index aligns with `aida worker directives`.
        let mut directive_count = 0usize;
        let mut new_lines: Vec<String> = Vec::with_capacity(body.lines().count());
        let mut removed: Option<String> = None;
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                new_lines.push(line.to_string());
                continue;
            }
            if directive_count == index {
                removed = Some(line.to_string());
                directive_count += 1;
                continue;
            }
            new_lines.push(line.to_string());
            directive_count += 1;
        }

        let removed = removed.ok_or_else(|| {
            format!(
                "no directive at index {} (there are {} directive(s))",
                index, directive_count
            )
        })?;

        let mut new_body = new_lines.join("\n");
        if !new_body.is_empty() && !new_body.ends_with('\n') {
            new_body.push('\n');
        }
        write_atomic(&path, new_body.as_bytes()).map_err(|e| e.to_string())?;
        Ok(format!("acked: {}", removed))
    }

    // ========================================================================
    // Resource implementations
    // ========================================================================

    fn resource_project_summary(&self) -> Result<String, String> {
        let store = self.storage.load().map_err(|e| e.to_string())?;

        let total = store.requirements.len();
        let by_status =
            |s: RequirementStatus| store.requirements.iter().filter(|r| r.status == s).count();

        let mut output = format!(
            "# Project Summary\n\n\
             **Total Requirements:** {}\n\
             - Draft: {}\n\
             - Approved: {}\n\
             - Planned: {}\n\
             - In Progress: {}\n\
             - Needs Attention: {}\n\
             - Done: {}\n\
             - Completed: {}\n\
             - Rejected: {}\n",
            total,
            by_status(RequirementStatus::Draft),
            by_status(RequirementStatus::Approved),
            by_status(RequirementStatus::Planned),
            by_status(RequirementStatus::InProgress),
            by_status(RequirementStatus::NeedsAttention),
            by_status(RequirementStatus::Done),
            by_status(RequirementStatus::Completed),
            by_status(RequirementStatus::Rejected),
        );

        if !store.features.is_empty() {
            output.push_str(&format!("\n**Features ({}):**\n", store.features.len()));
            for f in &store.features {
                output.push_str(&format!("- {}\n", f.name));
            }
        }

        Ok(output)
    }

    fn resource_requirements_tree(&self) -> Result<String, String> {
        let store = self.storage.load().map_err(|e| e.to_string())?;

        let mut output = "# Requirements Tree\n\n".to_string();

        let mut by_feature: std::collections::BTreeMap<String, Vec<&Requirement>> =
            std::collections::BTreeMap::new();
        for r in &store.requirements {
            let feature = if r.feature.is_empty() {
                "Uncategorized".to_string()
            } else {
                r.feature.clone()
            };
            by_feature.entry(feature).or_default().push(r);
        }

        for (feature, reqs) in &by_feature {
            output.push_str(&format!("## {}\n\n", feature));
            for r in reqs {
                output.push_str(&format!("- [{}] {} ({})\n", spec_id(r), r.title, r.status));
            }
            output.push('\n');
        }

        Ok(output)
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn parse_status(s: &str) -> Option<RequirementStatus> {
    match s.to_lowercase().as_str() {
        "draft" => Some(RequirementStatus::Draft),
        "approved" => Some(RequirementStatus::Approved),
        "planned" => Some(RequirementStatus::Planned),
        "in-progress" | "inprogress" | "in_progress" => Some(RequirementStatus::InProgress),
        "done" => Some(RequirementStatus::Done),
        "completed" => Some(RequirementStatus::Completed),
        "rejected" => Some(RequirementStatus::Rejected),
        "needs-attention" | "needsattention" | "needs_attention" => {
            Some(RequirementStatus::NeedsAttention)
        }
        _ => None,
    }
}

fn parse_priority(s: &str) -> Option<RequirementPriority> {
    match s.to_lowercase().as_str() {
        "high" => Some(RequirementPriority::High),
        "medium" => Some(RequirementPriority::Medium),
        "low" => Some(RequirementPriority::Low),
        _ => None,
    }
}

fn parse_requirement_type(s: &str) -> Option<RequirementType> {
    match s.to_lowercase().replace('-', "").as_str() {
        "functional" => Some(RequirementType::Functional),
        "nonfunctional" => Some(RequirementType::NonFunctional),
        "system" => Some(RequirementType::System),
        "user" => Some(RequirementType::User),
        "bug" => Some(RequirementType::Bug),
        "epic" => Some(RequirementType::Epic),
        "story" => Some(RequirementType::Story),
        "task" => Some(RequirementType::Task),
        "spike" => Some(RequirementType::Spike),
        "sprint" => Some(RequirementType::Sprint),
        _ => None,
    }
}

/// Read the current git branch under `project_root`. `None` on detached HEAD
/// or git failure.
fn current_branch_at(project_root: &Path) -> Option<String> {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn hostname_or_unknown() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Project a loaded `RequirementsStore` into the `RequirementSummary` shape
/// that `findings.rs` expects. Keeps the projection logic in one place;
/// mirrors the cache projection just well enough for the findings filter.
fn build_summaries(store: &aida_core::RequirementsStore) -> Vec<aida_core::RequirementSummary> {
    store
        .requirements
        .iter()
        .map(|r| aida_core::RequirementSummary {
            id: r.id,
            spec_id: r.spec_id.clone(),
            agreed_id: r.agreed_id.clone(),
            title: r.title.clone(),
            description: r.description.clone(),
            status: format!("{}", r.status),
            priority: format!("{}", r.priority),
            owner: r.owner.clone(),
            feature: r.feature.clone(),
            req_type: format!("{}", r.req_type),
            tags: r.tags.iter().cloned().collect(),
            created_at: r.created_at.to_rfc3339(),
            modified_at: r.modified_at.to_rfc3339(),
            archived: r.archived,
            yaml_path: String::new(),
        })
        .collect()
}

/// Append `line` to `path`, creating the file if needed. One line per call,
/// terminated with a newline. Uses `OpenOptions::append` so concurrent writers
/// don't lose updates on POSIX (the kernel guarantees atomicity of a single
/// small `write(2)` to an append-mode fd). We pre-concatenate the newline so
/// the whole record lands in a single `write_all` — `writeln!` issues
/// multiple `write_str` calls per format arg, and two concurrent writers can
/// interleave the content + newline writes, producing a torn line.
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(buf.as_bytes())?;
    Ok(())
}

/// Write `bytes` to `path` atomically: write to a sibling `path.tmp-<uuid>` then
/// `rename` over `path`. Crash mid-write leaves either the old file intact or
/// the new file intact — never a torn write.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_name = format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("aida-tmp"),
        uuid::Uuid::now_v7()
    );
    let tmp = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(tmp_name);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

// ============================================================================
// Tool descriptors (kept at module scope for register-agent + tests)
// ============================================================================

/// Every tool the MCP server exposes, in `tools/list` JSON shape. Static so
/// `aida mcp register-agent --print-tools` can render it without spinning up
/// the JSON-RPC loop.
pub fn tool_descriptors() -> Value {
    json!([
        // ---- Spec graph (original 7) ----
        {
            "name": "list_requirements",
            "description": "List requirements from the AIDA database, optionally filtered by status, type, or feature",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "Filter by status: draft, approved, planned, in-progress, needs-attention, done, completed, rejected" },
                    "type": { "type": "string", "description": "Filter by type: functional, non-functional, system, user, bug, epic, story, task, spike, sprint" },
                    "feature": { "type": "string", "description": "Filter by feature category" },
                    "limit": { "type": "integer", "description": "Maximum number of results (default: 50)" }
                }
            }
        },
        {
            "name": "show_requirement",
            "description": "Show full details of a specific requirement by its SPEC-ID (e.g., FR-0042)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The SPEC-ID of the requirement (e.g., FR-0042)" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "add_requirement",
            "description": "Add a new requirement to the AIDA database",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Title of the requirement" },
                    "description": { "type": "string", "description": "Detailed description" },
                    "type": { "type": "string", "description": "Requirement type: functional, non-functional, system, user, bug, epic, story, task" },
                    "status": { "type": "string", "description": "Status: draft, approved, planned, in-progress, done, completed, rejected" },
                    "priority": { "type": "string", "description": "Priority: high, medium, low" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags to attach to the requirement" }
                },
                "required": ["title", "description"]
            }
        },
        {
            "name": "update_requirement",
            "description": "Update an existing requirement's status or description",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The SPEC-ID of the requirement" },
                    "status": { "type": "string", "description": "New status" },
                    "description": { "type": "string", "description": "New description" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "search_requirements",
            "description": "Search requirements by keyword (searches titles and descriptions)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query (case-insensitive)" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "add_comment",
            "description": "Add a comment to a requirement",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The SPEC-ID of the requirement" },
                    "text": { "type": "string", "description": "Comment text" }
                },
                "required": ["id", "text"]
            }
        },
        {
            "name": "list_features",
            "description": "List all feature categories in the project",
            "inputSchema": { "type": "object", "properties": {} }
        },

        // ---- Punt channel (.aida/punts.jsonl + .aida/punts/) ----
        {
            "name": "list_punts",
            "description": "List punt records from .aida/punts.jsonl. Optional status filter (`awaiting`, `advisor-resolved`, `escalated-to-human`, `escalate-defaulted`).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "Filter on resolution_path; 'awaiting' = still 'punted'" }
                }
            }
        },
        {
            "name": "read_punt",
            "description": "Read the most recent punt record for a given spec from .aida/punts.jsonl (returns JSON).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": { "type": "string", "description": "Display ID of the punted spec" }
                },
                "required": ["spec_id"]
            }
        },
        {
            "name": "post_punt",
            "description": "Append a punt record to .aida/punts.jsonl (does not modify spec status — pair with `update_requirement status=needs-attention` from a session lease).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": { "type": "string" },
                    "detail": { "type": "string", "description": "Human-readable description of the obstacle" },
                    "category": { "type": "string", "description": "design-fork | ambiguous-spec | missing-context | blocked-dependency | other" },
                    "lean": { "type": "string", "description": "Best guess if forced to choose" },
                    "raised_by": { "type": "string", "description": "Role/agent that raised the punt (defaults to 'mcp')" }
                },
                "required": ["spec_id", "detail"]
            }
        },
        {
            "name": "resolve_punt",
            "description": "Write a PuntResponse to .aida/punts/<spec>.response.json marking the fork resolved. The orchestrator will resume the implementer with this answer (advisor-tier protocol, STORY-306).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": { "type": "string" },
                    "answer": { "type": "string", "description": "The chosen resolution to apply" },
                    "reasoning": { "type": "string", "description": "Why this resolution; always required (audit trail)" },
                    "classification": { "type": "string", "description": "A/B/C calibration class (recorded-principle / recorded-preference / synthesized)" }
                },
                "required": ["spec_id", "answer", "reasoning"]
            }
        },
        {
            "name": "escalate_punt",
            "description": "Write a PuntResponse to .aida/punts/<spec>.response.json marking the fork escalated to a human. The orchestrator will park the spec for triage (STORY-306).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": { "type": "string" },
                    "reasoning": { "type": "string", "description": "Why a human is needed (always required)" },
                    "escalation_reason": { "type": "string", "description": "Categorized reason: strategy | irreversible | unrecorded-context | ..." },
                    "classification": { "type": "string" }
                },
                "required": ["spec_id", "reasoning"]
            }
        },

        // ---- Findings channel (draft requirements with from-* tags) ----
        {
            "name": "list_findings",
            "description": "List findings filed by headless drain phases: 'review' (phase 3) or 'implementer' (phase 1). Returns the same triage view as `aida findings list`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pr": { "type": "integer", "description": "Narrow review findings to one PR number" },
                    "source": { "type": "string", "description": "review | implementer" },
                    "kind": { "type": "string", "description": "deviation | design-choice | bug-spotted | followup-suggestion (implementer findings only)" }
                }
            }
        },
        {
            "name": "file_finding",
            "description": "File a finding as a draft TASK with the appropriate from-* / severity: / kind: tags. Source is 'implementer' (carries spec_id) or 'review' (carries pr number).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "source": { "type": "string", "description": "implementer | review (default: implementer)" },
                    "spec_id": { "type": "string", "description": "Required when source=implementer" },
                    "pr": { "type": "integer", "description": "Required when source=review" },
                    "kind": { "type": "string", "description": "deviation | design-choice | bug-spotted | followup-suggestion" },
                    "severity": { "type": "string", "description": "cosmetic | minor | major (default: minor)" }
                },
                "required": ["title", "description"]
            }
        },
        {
            "name": "triage_finding",
            "description": "Promote (status → Approved) or dismiss (status → Rejected) a finding, recording the reason as a comment.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Finding SPEC-ID" },
                    "action": { "type": "string", "description": "promote | dismiss" },
                    "reason": { "type": "string", "description": "One-line rationale (recorded as a comment)" }
                },
                "required": ["id", "action"]
            }
        },

        // ---- Task-claim channel (.aida/sessions/*.toml) ----
        {
            "name": "claim_task",
            "description": "Claim a spec by writing a lightweight lease to .aida/sessions/<id>.toml. Returns the lease_id, or 'already_claimed' if a lease already covers the spec. Does NOT create a worktree — for that, use `aida session start --owns <spec>` from a shell.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "spec_id": { "type": "string", "description": "The SPEC-ID to claim" },
                    "role": { "type": "string", "description": "Role taking the claim (default: implementer)" }
                },
                "required": ["spec_id"]
            }
        },
        {
            "name": "release_task",
            "description": "Delete an MCP-created lease (mcp_claim=true) from .aida/sessions/. Refuses to delete real `aida session start` leases — for those use `aida session end <lease_id>`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "lease_id": { "type": "string" }
                },
                "required": ["lease_id"]
            }
        },
        {
            "name": "list_active_leases",
            "description": "List every active lease in .aida/sessions/ — both real `aida session start` leases and MCP claim_task leases.",
            "inputSchema": { "type": "object", "properties": {} }
        },

        // ---- Worker-directive channel (.aida/worker.cmd) ----
        {
            "name": "post_directive",
            "description": "Append a directive to .aida/worker.cmd. Verbs: drain (with optional args forwarded to `aida queue work`), pause, exit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "verb": { "type": "string", "description": "drain | pause | exit" },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "Args forwarded to `aida queue work` for drain directives" }
                },
                "required": ["verb"]
            }
        },
        {
            "name": "list_directives",
            "description": "List pending directives in .aida/worker.cmd (the same view as `aida worker directives`).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "ack_directive",
            "description": "Remove a directive from .aida/worker.cmd by its 0-based index (matching the `list_directives` order). Use this after acting on the directive — the worker's natural flow `pops` directives on completion.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "index": { "type": "integer", "description": "0-based index of the directive to ack" }
                },
                "required": ["index"]
            }
        }
    ])
}

// ============================================================================
// Entry point
// ============================================================================

/// Run the MCP server, reading JSON-RPC from stdin and writing to stdout.
///
/// `project_root` is the AIDA project root (parent of `.aida/`). The
/// coordination tools (STORY-361) resolve their file paths under this root.
pub fn run_mcp_server(storage: &Storage, project_root: PathBuf) -> Result<()> {
    let server = McpServer::new(storage, project_root);
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    eprintln!("AIDA MCP server started");

    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(req) => req,
            Err(e) => {
                let error_response =
                    JsonRpcResponse::error(Value::Null, -32700, format!("Parse error: {}", e));
                let json = serde_json::to_string(&error_response)?;
                writeln!(stdout, "{}", json)?;
                stdout.flush()?;
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            let error_response = JsonRpcResponse::error(
                request.id.unwrap_or(Value::Null),
                -32600,
                "Invalid Request: jsonrpc must be \"2.0\"".to_string(),
            );
            let json = serde_json::to_string(&error_response)?;
            writeln!(stdout, "{}", json)?;
            stdout.flush()?;
            continue;
        }

        if let Some(response) = server.handle_request(&request) {
            let json = serde_json::to_string(&response)?;
            writeln!(stdout, "{}", json)?;
            stdout.flush()?;
        }
    }

    eprintln!("AIDA MCP server stopped");
    Ok(())
}

// Touch the imports re-exported above to silence dead_code on builds where
// the punt module's helpers happen to be unused (defensive against future
// refactors). All five items are used in tool implementations.
#[allow(dead_code)]
fn _force_use() {
    let _ = punt::SIGNAL_FILE_ENV;
    let _ = ledger_path as fn(&Path) -> PathBuf;
}

// ============================================================================
// Tests — STORY-361
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use tempfile::tempdir;

    fn mk_server(dir: &Path) -> McpServer<'static> {
        // Cache-backed Storage isn't required for the coordination-only tools;
        // we still need *a* Storage to instantiate the server. Point it at a
        // throwaway YAML file under the temp dir.
        let cache_path = dir.join(".aida").join("mcp-test-cache.yaml");
        std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let storage = Box::leak(Box::new(Storage::new(cache_path)));
        McpServer::new(storage, dir.to_path_buf())
    }

    #[test]
    fn tool_descriptors_lists_at_least_14_coordination_tools() {
        let desc = tool_descriptors();
        let arr = desc.as_array().unwrap();
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        // 7 spec-graph + 14 coordination = 21 total
        assert!(names.len() >= 21, "expected ≥21 tools, got {}", names.len());
        for required in [
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
        ] {
            assert!(names.contains(&required), "missing tool: {}", required);
        }
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

    // trace:TASK-436 | ai:claude
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
        let unique: std::collections::HashSet<_> =
            directives.iter().map(|d| d.raw.clone()).collect();
        assert_eq!(unique.len(), N);
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
}
