// trace:TASK-0001 | ai:claude
//! Skill runner infrastructure for executing AIDA skills from the web UI.
//!
//! Currently supports:
//! - `aida-compiler-warnings`: Run cargo clippy, parse JSON diagnostics,
//!   categorize warnings by risk level, and provide action handlers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse, Json,
    },
    routing::post,
    Router,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncBufReadExt;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info, warn};

use crate::admin::AdminState;
use crate::llm::LlmProvider;
use crate::projects::ProjectManager;
use crate::service::ServerState;

// ============================================================================
// State
// ============================================================================

enum SkillBackend {
    Single(Arc<ServerState>),
    Multi(Arc<ProjectManager>),
}

pub struct SkillRunnerState {
    backend: SkillBackend,
    pub admin: Arc<AdminState>,
    pub running: AtomicBool,
}

// ============================================================================
// Types — Warning structures
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    pub code: String,
    pub message: String,
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
    pub crate_name: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarningCategory {
    pub name: String,
    pub risk_level: String,
    pub description: String,
    pub recommended_action: String,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarningsReport {
    pub total_warnings: usize,
    pub crate_counts: HashMap<String, usize>,
    pub categories: Vec<WarningCategory>,
    pub raw_output: String,
}

// ============================================================================
// Types — SSE events
// ============================================================================

#[derive(Serialize)]
struct SseLogData {
    line: String,
    stream: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SseProgressData {
    phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pct: Option<u8>,
}

// ============================================================================
// Types — Action request/response
// ============================================================================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionRequest {
    action: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionResponse {
    success: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    spec_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff_summary: Option<String>,
}

// ============================================================================
// Types — Skill chat
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillChatRequest {
    messages: Vec<SkillChatMessage>,
    context: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<SkillChatMessage>,
    stream: bool,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct OpenAiChatRequest {
    model: String,
    max_tokens: u32,
    stream: bool,
    messages: Vec<OpenAiMessage>,
}

// ============================================================================
// Router
// ============================================================================

pub fn create_skill_runner_router(server: Arc<ServerState>, admin: Arc<AdminState>) -> Router {
    let state = Arc::new(SkillRunnerState {
        backend: SkillBackend::Single(server),
        admin,
        running: AtomicBool::new(false),
    });
    Router::new()
        .route("/api/v2/skills/:name/run", post(run_skill_sse))
        .route("/api/v2/skills/:name/action", post(skill_action))
        .route("/api/v2/skills/:name/chat", post(skill_chat_stream))
        .with_state(state)
}

pub fn create_skill_runner_router_multi(
    project_manager: Arc<ProjectManager>,
    admin: Arc<AdminState>,
) -> Router {
    let state = Arc::new(SkillRunnerState {
        backend: SkillBackend::Multi(project_manager),
        admin,
        running: AtomicBool::new(false),
    });
    Router::new()
        .route("/api/v2/skills/:name/run", post(run_skill_sse))
        .route("/api/v2/skills/:name/action", post(skill_action))
        .route("/api/v2/skills/:name/chat", post(skill_chat_stream))
        .with_state(state)
}

// ============================================================================
// SSE helpers
// ============================================================================

type SseTx = tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>;

async fn send_event(tx: &SseTx, event_type: &str, data: &str) -> Result<(), anyhow::Error> {
    tx.send(Ok(Event::default().event(event_type).data(data)))
        .await
        .map_err(|_| anyhow::anyhow!("SSE channel closed"))
}

async fn send_log(tx: &SseTx, line: &str, stream: &str) -> Result<(), anyhow::Error> {
    let data = SseLogData {
        line: line.to_string(),
        stream: stream.to_string(),
    };
    send_event(tx, "log", &serde_json::to_string(&data)?).await
}

async fn send_progress(tx: &SseTx, phase: &str, pct: Option<u8>) -> Result<(), anyhow::Error> {
    let data = SseProgressData {
        phase: phase.to_string(),
        pct,
    };
    send_event(tx, "progress", &serde_json::to_string(&data)?).await
}

async fn resolve_server_state(
    state: &SkillRunnerState,
    headers: &HeaderMap,
) -> Result<Arc<ServerState>, (StatusCode, Json<serde_json::Value>)> {
    match &state.backend {
        SkillBackend::Single(server) => Ok(server.clone()),
        SkillBackend::Multi(project_manager) => {
            let project = headers
                .get("x-project")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "Missing X-Project header" })),
                    )
                })?;
            project_manager.get_backend(project).await.map_err(|e| {
                (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": format!("Project error: {}", e) })),
                )
            })
        }
    }
}

// ============================================================================
// Handlers
// ============================================================================

async fn run_skill_sse(
    State(state): State<Arc<SkillRunnerState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Sse<ReceiverStream<Result<Event, std::convert::Infallible>>>, impl IntoResponse> {
    if !state.admin.authorize_server(&headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Unauthorized" })),
        ));
    }

    // Only compiler-warnings is supported right now
    if name != "aida-compiler-warnings" {
        return Err((
            StatusCode::NOT_FOUND,
            Json(
                serde_json::json!({ "error": format!("Skill '{}' is not runnable from the web UI", name) }),
            ),
        ));
    }

    // Prevent concurrent runs
    if state
        .running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "A skill is already running" })),
        ));
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(256);

    let run_state = state.clone();
    tokio::spawn(async move {
        let result = run_compiler_warnings(&tx).await;
        run_state.running.store(false, Ordering::SeqCst);
        if let Err(e) = result {
            warn!("Skill runner error: {}", e);
            let _ = send_event(
                &tx,
                "error",
                &serde_json::json!({ "message": e.to_string() }).to_string(),
            )
            .await;
        }
        let _ = send_event(&tx, "done", "{}").await;
    });

    Ok(Sse::new(ReceiverStream::new(rx)))
}

async fn skill_action(
    State(state): State<Arc<SkillRunnerState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<ActionRequest>,
) -> Result<Json<ActionResponse>, (StatusCode, Json<serde_json::Value>)> {
    if !state.admin.authorize_server(&headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Unauthorized" })),
        ));
    }

    if name != "aida-compiler-warnings" {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Skill '{}' is not runnable", name) })),
        ));
    }

    match body.action.as_str() {
        "auto_fix" => handle_auto_fix().await,
        "create_defect" => {
            let server = resolve_server_state(&state, &headers).await?;
            handle_create_requirement(&server, &body.params, "bug").await
        }
        "create_task" => {
            let server = resolve_server_state(&state, &headers).await?;
            handle_create_requirement(&server, &body.params, "task").await
        }
        other => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Unknown action: {}", other) })),
        )),
    }
}

// ============================================================================
// Compiler warnings runner
// ============================================================================

/// Find the workspace root by walking up from cwd looking for Cargo.toml + aida-server/
fn find_workspace_root() -> Option<std::path::PathBuf> {
    if let Ok(root) = std::env::var("AIDA_WORKSPACE_ROOT") {
        let p = std::path::PathBuf::from(root);
        if p.join("Cargo.toml").exists() {
            return Some(p);
        }
    }

    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("Cargo.toml").exists() && dir.join("aida-server").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent()?.to_path_buf();
        loop {
            if dir.join("Cargo.toml").exists() && dir.join("aida-server").is_dir() {
                return Some(dir);
            }
            if !dir.pop() {
                break;
            }
        }
    }

    None
}

async fn run_compiler_warnings(tx: &SseTx) -> Result<(), anyhow::Error> {
    let workspace_root = find_workspace_root().ok_or_else(|| {
        anyhow::anyhow!("Cannot find workspace root. Set AIDA_WORKSPACE_ROOT env var.")
    })?;

    info!("Running cargo clippy in {:?}", workspace_root);
    send_progress(tx, "Running clippy...", Some(0)).await?;

    let start = Instant::now();

    // Run cargo clippy with JSON output for machine parsing
    let mut child = tokio::process::Command::new("cargo")
        .args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--message-format=json",
        ])
        .current_dir(&workspace_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Collect JSON lines from stdout
    let tx_json = tx.clone();
    let json_handle = tokio::spawn(async move {
        let mut json_lines = Vec::new();
        if let Some(stdout) = stdout {
            let reader = tokio::io::BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // Send abbreviated log for progress feedback
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                    if val.get("reason").and_then(|r| r.as_str()) == Some("compiler-message") {
                        if let Some(msg) = val
                            .get("message")
                            .and_then(|m| m.get("rendered"))
                            .and_then(|r| r.as_str())
                        {
                            let short = msg.lines().next().unwrap_or("").to_string();
                            let _ = send_log(&tx_json, &short, "stdout").await;
                        }
                    }
                }
                json_lines.push(line);
            }
        }
        json_lines
    });

    // Stream stderr for build progress
    let tx_err = tx.clone();
    let stderr_handle = tokio::spawn(async move {
        let mut raw_output = String::new();
        if let Some(stderr) = stderr {
            let reader = tokio::io::BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                raw_output.push_str(&line);
                raw_output.push('\n');
                let _ = send_log(&tx_err, &line, "stderr").await;
            }
        }
        raw_output
    });

    let json_lines = json_handle.await.unwrap_or_default();
    let raw_output = stderr_handle.await.unwrap_or_default();

    let status = child.wait().await?;
    let duration_ms = start.elapsed().as_millis() as u64;

    info!(
        "Clippy finished in {}ms (exit code: {:?})",
        duration_ms,
        status.code()
    );
    send_progress(tx, "Parsing results...", Some(80)).await?;

    // Parse JSON diagnostics
    let report = parse_clippy_json(&json_lines, &raw_output);

    info!(
        "Parsed {} warnings across {} crates",
        report.total_warnings,
        report.crate_counts.len()
    );

    // Send the structured result
    send_event(tx, "result", &serde_json::to_string(&report)?).await?;
    send_progress(tx, "Done", Some(100)).await?;

    Ok(())
}

// ============================================================================
// Clippy JSON parsing
// ============================================================================

fn categorize_lint(code: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    // Returns (category_name, risk_level, description, recommended_action)
    match code {
        "unused_imports"
        | "unused_mut"
        | "unused_variables"
        | "unused_parens"
        | "unused_braces"
        | "redundant_semicolons" => (
            "Safe Auto-Fix",
            "none",
            "Trivial cleanups that cargo clippy --fix handles automatically",
            "Run auto-fix",
        ),
        "dead_code" | "unreachable_code" | "unused_macros" => (
            "Low Risk",
            "low",
            "Code that is never executed — usually safe to remove after verification",
            "Review and remove",
        ),
        "unused_assignments" | "unexpected_cfgs" | "field_never_read" | "unused_must_use" => (
            "Medium Risk",
            "medium",
            "May indicate logic errors or missing functionality",
            "Review carefully",
        ),
        _ if code.starts_with("clippy::") => {
            let clippy_code = &code["clippy::".len()..];
            match clippy_code {
                "needless_return"
                | "redundant_closure"
                | "len_zero"
                | "single_match"
                | "match_bool"
                | "manual_map"
                | "unnecessary_cast"
                | "needless_borrow"
                | "clone_on_copy"
                | "redundant_field_names"
                | "useless_format"
                | "needless_pass_by_value"
                | "explicit_auto_deref"
                | "ptr_arg"
                | "single_char_pattern"
                | "manual_is_ascii_check"
                | "unnecessary_to_owned"
                | "needless_borrows_for_generic_args" => (
                    "Safe Auto-Fix",
                    "none",
                    "Clippy style/simplification suggestions with mechanical fixes",
                    "Run auto-fix",
                ),
                "enum_variant_names"
                | "type_complexity"
                | "too_many_arguments"
                | "large_enum_variant"
                | "cognitive_complexity"
                | "module_name_repetitions"
                | "struct_excessive_bools"
                | "similar_names"
                | "wildcard_imports" => (
                    "Low Risk",
                    "low",
                    "Code quality and naming suggestions",
                    "Review and refactor",
                ),
                "unwrap_used" | "expect_used" | "indexing_slicing" | "panic" | "todo"
                | "unimplemented" | "unreachable" => (
                    "Review Needed",
                    "high",
                    "Potential runtime panics or incomplete implementations",
                    "Review and fix",
                ),
                _ => (
                    "Medium Risk",
                    "medium",
                    "Clippy suggestions that may improve correctness or performance",
                    "Review and apply",
                ),
            }
        }
        _ => (
            "Medium Risk",
            "medium",
            "Compiler warnings that should be reviewed",
            "Review and fix",
        ),
    }
}

fn parse_clippy_json(json_lines: &[String], raw_output: &str) -> WarningsReport {
    let mut warnings: Vec<Warning> = Vec::new();
    let mut crate_counts: HashMap<String, usize> = HashMap::new();

    for line in json_lines {
        let val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Only process compiler messages (not build status, etc.)
        if val.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
            continue;
        }

        let message = match val.get("message") {
            Some(m) => m,
            None => continue,
        };

        // Skip non-warning levels (errors, notes, help)
        let level = message.get("level").and_then(|l| l.as_str()).unwrap_or("");
        if level != "warning" {
            continue;
        }

        // Extract warning code
        let code = message
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|c| c.as_str())
            .unwrap_or("unknown");

        // Skip the generic "warnings" generated message
        if code == "unknown" {
            continue;
        }

        let msg_text = message
            .get("rendered")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();

        // Extract primary span location
        let spans = message.get("spans").and_then(|s| s.as_array());
        let primary_span = spans.and_then(|spans| {
            spans
                .iter()
                .find(|s| {
                    s.get("is_primary")
                        .and_then(|p| p.as_bool())
                        .unwrap_or(false)
                })
                .or(spans.first())
        });

        let file = primary_span
            .and_then(|s| s.get("file_name"))
            .and_then(|f| f.as_str())
            .unwrap_or("unknown")
            .to_string();

        let line_num = primary_span
            .and_then(|s| s.get("line_start"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0) as u32;

        let column = primary_span
            .and_then(|s| s.get("column_start"))
            .and_then(|c| c.as_u64())
            .map(|c| c as u32);

        // Extract crate name from target
        let crate_name = val
            .get("target")
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Extract suggestion if available
        let suggestion = message
            .get("children")
            .and_then(|c| c.as_array())
            .and_then(|children| {
                children.iter().find_map(|child| {
                    if child.get("level").and_then(|l| l.as_str()) == Some("help") {
                        child
                            .get("message")
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string())
                    } else {
                        None
                    }
                })
            });

        *crate_counts.entry(crate_name.clone()).or_insert(0) += 1;

        warnings.push(Warning {
            code: code.to_string(),
            message: msg_text,
            file,
            line: line_num,
            column,
            crate_name,
            suggestion,
        });
    }

    // Categorize warnings
    let mut category_map: HashMap<String, WarningCategory> = HashMap::new();

    for warning in &warnings {
        let (cat_name, risk, desc, action) = categorize_lint(&warning.code);
        let category =
            category_map
                .entry(cat_name.to_string())
                .or_insert_with(|| WarningCategory {
                    name: cat_name.to_string(),
                    risk_level: risk.to_string(),
                    description: desc.to_string(),
                    recommended_action: action.to_string(),
                    warnings: Vec::new(),
                });
        category.warnings.push(warning.clone());
    }

    // Sort categories by risk level
    let risk_order = |r: &str| match r {
        "none" => 0,
        "low" => 1,
        "medium" => 2,
        "high" => 3,
        _ => 4,
    };

    let mut categories: Vec<WarningCategory> = category_map.into_values().collect();
    categories.sort_by(|a, b| risk_order(&a.risk_level).cmp(&risk_order(&b.risk_level)));

    let total_warnings = warnings.len();

    WarningsReport {
        total_warnings,
        crate_counts,
        categories,
        raw_output: raw_output.to_string(),
    }
}

// ============================================================================
// Action handlers
// ============================================================================

async fn handle_auto_fix() -> Result<Json<ActionResponse>, (StatusCode, Json<serde_json::Value>)> {
    let workspace_root = find_workspace_root().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Cannot find workspace root" })),
        )
    })?;

    info!("Running cargo clippy --fix");

    let output = tokio::process::Command::new("cargo")
        .args([
            "clippy",
            "--fix",
            "--workspace",
            "--allow-dirty",
            "--allow-staged",
        ])
        .current_dir(&workspace_root)
        .output()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to run clippy --fix: {}", e) })),
            )
        })?;

    // Get git diff summary
    let diff_output = tokio::process::Command::new("git")
        .args(["diff", "--stat"])
        .current_dir(&workspace_root)
        .output()
        .await
        .ok();

    let diff_summary = diff_output
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();

    if output.status.success() {
        Ok(Json(ActionResponse {
            success: true,
            message: "Auto-fix completed successfully".to_string(),
            spec_id: None,
            diff_summary: Some(diff_summary),
        }))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(Json(ActionResponse {
            success: false,
            message: format!("Auto-fix completed with warnings: {}", stderr),
            spec_id: None,
            diff_summary: Some(diff_summary),
        }))
    }
}

async fn handle_create_requirement(
    server: &Arc<ServerState>,
    params: &serde_json::Value,
    req_type: &str,
) -> Result<Json<ActionResponse>, (StatusCode, Json<serde_json::Value>)> {
    let title = params
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("Compiler warnings cleanup")
        .to_string();

    let warnings_desc = params
        .get("warnings")
        .and_then(|w| w.as_array())
        .map(|arr| {
            arr.iter()
                .map(|w| {
                    let code = w.get("code").and_then(|c| c.as_str()).unwrap_or("?");
                    let file = w.get("file").and_then(|f| f.as_str()).unwrap_or("?");
                    let line = w.get("line").and_then(|l| l.as_u64()).unwrap_or(0);
                    format!("- `{code}` at {file}:{line}")
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    let category = params
        .get("category")
        .and_then(|c| c.as_str())
        .unwrap_or("unknown");

    let description = format!(
        "## Compiler Warnings — {}\n\nGenerated by /aida-compiler-warnings skill runner.\n\n### Warnings\n{}",
        category, warnings_desc
    );

    let assignee = params
        .get("assignee")
        .and_then(|a| a.as_str())
        .unwrap_or("")
        .to_string();

    // Create requirement using the store
    server.check_reload().await;
    let mut store = server.store.write().await;

    let mut new_req = aida_core::Requirement::new(title.clone(), description);
    new_req.status = aida_core::RequirementStatus::Draft;
    new_req.priority = aida_core::RequirementPriority::Medium;
    new_req.req_type = if req_type == "bug" {
        aida_core::RequirementType::Bug
    } else {
        aida_core::RequirementType::Task
    };
    new_req.owner = assignee;
    new_req.tags = vec!["compiler-warnings".to_string()].into_iter().collect();

    let type_prefix = store
        .type_definitions
        .iter()
        .find(|td| {
            td.name
                == new_req
                    .req_type
                    .to_string()
                    .replace(" ", "")
                    .replace("-", "")
        })
        .and_then(|td| td.prefix.clone());

    store.add_requirement_with_id(new_req, None, type_prefix.as_deref());

    let spec_id = store
        .requirements
        .last()
        .and_then(|r| r.spec_id.clone())
        .unwrap_or_default();

    drop(store);

    if let Err(e) = server.backend.save(&*server.store.read().await) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to save: {}", e) })),
        ));
    }
    server.mark_saved().await;

    info!("Created {} requirement: {}", req_type, spec_id);

    Ok(Json(ActionResponse {
        success: true,
        message: format!("Created {} {}: {}", req_type, spec_id, title),
        spec_id: Some(spec_id),
        diff_summary: None,
    }))
}

// ============================================================================
// Skill chat — context-aware AI Q&A on skill results
// ============================================================================

async fn skill_chat_stream(
    State(state): State<Arc<SkillRunnerState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(req): Json<SkillChatRequest>,
) -> Result<Sse<ReceiverStream<Result<Event, axum::Error>>>, impl IntoResponse> {
    if !state.admin.authorize_server(&headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Unauthorized" })),
        ));
    }

    if name != "aida-compiler-warnings" {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Skill '{}' does not support chat", name) })),
        ));
    }

    let provider = LlmProvider::from_env();
    let key_name = provider.api_key_name();

    let keys = state.admin.api_keys.read().await;
    let api_key = keys.get(key_name).cloned();
    drop(keys);

    let api_key = match api_key {
        Some(key) if !key.is_empty() => key,
        _ => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": format!("{} not configured", key_name) })),
            ));
        }
    };

    if req.messages.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "messages array must not be empty" })),
        ));
    }

    // Build system prompt with warnings context
    let context_str = serde_json::to_string_pretty(&req.context).unwrap_or_default();
    let system_prompt = format!(
        r#"You are AIDA's compiler warnings analyst. You have access to the full results from a `cargo clippy` run on this Rust workspace.

Your job is to help the developer understand, prioritize, and address compiler warnings. When answering:
- Reference specific files, line numbers, and warning codes
- Explain whether warnings are safe to fix or need careful review
- When asked to create defects or tasks, provide structured recommendations
- Be concise and actionable
- Use markdown formatting

## Compiler Warnings Report
{context_str}"#
    );

    let model = provider.resolve_model("AIDA_CHAT_MODEL", provider.default_chat_model());

    let (tx, rx) = tokio::sync::mpsc::channel(64);

    tokio::spawn(async move {
        let client = reqwest::Client::new();

        let response = match provider {
            LlmProvider::Anthropic => {
                let body = AnthropicRequest {
                    model,
                    max_tokens: 4096,
                    system: system_prompt,
                    messages: req.messages,
                    stream: true,
                };
                client
                    .post(format!("{}/v1/messages", provider.base_url()))
                    .header("x-api-key", &api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await
            }
            LlmProvider::OpenAi => {
                let messages = std::iter::once(OpenAiMessage {
                    role: "system".to_string(),
                    content: system_prompt,
                })
                .chain(req.messages.into_iter().map(|m| OpenAiMessage {
                    role: m.role,
                    content: m.content,
                }))
                .collect::<Vec<_>>();
                let body = OpenAiChatRequest {
                    model,
                    max_tokens: 4096,
                    stream: true,
                    messages,
                };
                client
                    .post(format!("{}/v1/chat/completions", provider.base_url()))
                    .header("authorization", format!("Bearer {}", api_key))
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await
            }
        };

        let response = match response {
            Ok(r) => {
                if !r.status().is_success() {
                    let status = r.status();
                    let body = r.text().await.unwrap_or_default();
                    error!("Provider API error {}: {}", status, body);
                    let _ = tx
                        .send(Ok(Event::default().event("error").data(format!(
                            r#"{{"error":"Provider API error: {}"}}"#,
                            status
                        ))))
                        .await;
                    return;
                }
                r
            }
            Err(e) => {
                error!("Failed to reach provider API: {}", e);
                let _ = tx
                    .send(Ok(Event::default().event("error").data(format!(
                        r#"{{"error":"Failed to reach provider API: {}"}}"#,
                        e
                    ))))
                    .await;
                return;
            }
        };

        // Parse provider SSE stream
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    warn!("Stream read error: {}", e);
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(event_end) = buffer.find("\n\n") {
                let event_str = buffer[..event_end].to_string();
                buffer = buffer[event_end + 2..].to_string();

                let mut event_type = String::new();
                let mut data = String::new();

                for line in event_str.lines() {
                    if let Some(et) = line.strip_prefix("event: ") {
                        event_type = et.to_string();
                    } else if let Some(d) = line.strip_prefix("data: ") {
                        data = d.to_string();
                    }
                }

                match provider {
                    LlmProvider::Anthropic => {
                        if event_type == "content_block_delta" {
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data) {
                                if let Some(text) = parsed
                                    .get("delta")
                                    .and_then(|d| d.get("text"))
                                    .and_then(|t| t.as_str())
                                {
                                    let payload = serde_json::json!({ "text": text });
                                    if tx
                                        .send(Ok(Event::default()
                                            .event("delta")
                                            .data(payload.to_string())))
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                        } else if event_type == "message_stop" {
                            let _ = tx.send(Ok(Event::default().event("done").data("{}"))).await;
                            return;
                        } else if event_type == "error" {
                            let _ = tx
                                .send(Ok(Event::default().event("error").data(data)))
                                .await;
                            return;
                        }
                    }
                    LlmProvider::OpenAi => {
                        if data == "[DONE]" {
                            let _ = tx.send(Ok(Event::default().event("done").data("{}"))).await;
                            return;
                        }
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data) {
                            if let Some(text) = parsed
                                .get("choices")
                                .and_then(|c| c.as_array())
                                .and_then(|arr| arr.first())
                                .and_then(|choice| choice.get("delta"))
                                .and_then(|d| d.get("content"))
                                .and_then(|t| t.as_str())
                            {
                                let payload = serde_json::json!({ "text": text });
                                if tx
                                    .send(Ok(Event::default()
                                        .event("delta")
                                        .data(payload.to_string())))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }

        let _ = tx.send(Ok(Event::default().event("done").data("{}"))).await;
    });

    Ok(Sse::new(ReceiverStream::new(rx)))
}
