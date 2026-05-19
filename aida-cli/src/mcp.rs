// trace:FR-0152 | ai:claude
//! MCP (Model Context Protocol) server for AIDA requirements.
//!
//! Implements JSON-RPC 2.0 over stdio, exposing AIDA requirements
//! as tools and resources for Claude Code integration.

use std::io::{self, BufRead, Write};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use aida_core::{
    Comment, Requirement, RequirementPriority, RequirementStatus, RequirementType, Storage,
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
// MCP Server
// ============================================================================

struct McpServer<'a> {
    storage: &'a Storage,
}

/// Helper to get the display spec_id from a Requirement
fn spec_id(r: &Requirement) -> &str {
    r.spec_id.as_deref().unwrap_or("?")
}

impl<'a> McpServer<'a> {
    fn new(storage: &'a Storage) -> Self {
        Self { storage }
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
        JsonRpcResponse::success(
            id.clone(),
            json!({
                "tools": [
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
                                "priority": { "type": "string", "description": "Priority: high, medium, low" }
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
                        "inputSchema": {
                            "type": "object",
                            "properties": {}
                        }
                    }
                ]
            }),
        )
    }

    fn handle_tools_call(&self, id: &Value, params: &Value) -> JsonRpcResponse {
        let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let result = match tool_name {
            "list_requirements" => self.tool_list_requirements(&arguments),
            "show_requirement" => self.tool_show_requirement(&arguments),
            "add_requirement" => self.tool_add_requirement(&arguments),
            "update_requirement" => self.tool_update_requirement(&arguments),
            "search_requirements" => self.tool_search_requirements(&arguments),
            "add_comment" => self.tool_add_comment(&arguments),
            "list_features" => self.tool_list_features(),
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
    // Tool implementations
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
                // Look up the target requirement's spec_id
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
        // STORY-332: a freshly-added spec cannot be born paused —
        // NeedsAttention is reached only by punting In-Progress work.
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

        store.requirements.push(req);
        // assign_spec_ids generates IDs for requirements that don't have one
        store.assign_spec_ids();

        // Get the spec_id that was just assigned
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
                // STORY-332: enforce the NeedsAttention transition rules —
                // into it only from In Progress (`aida punt`), out of it
                // only to Approved / In Progress / Rejected.
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

        // Group by feature
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

// ============================================================================
// Entry point
// ============================================================================

/// Run the MCP server, reading JSON-RPC from stdin and writing to stdout.
pub fn run_mcp_server(storage: &Storage) -> Result<()> {
    let server = McpServer::new(storage);
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
