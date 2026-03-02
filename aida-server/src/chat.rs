// trace:STORY-0374 | ai:claude
//! Chat endpoints for AIDA — streams AI responses from provider APIs
//! with full requirements context for PM/stakeholder Q&A.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse, Json,
    },
    routing::{get, post},
    Router,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, warn};

use crate::admin::AdminState;
use crate::llm::LlmProvider;
use crate::projects::ProjectManager;
use crate::service::ServerState;
use aida_core::ai::prompts;

// ============================================================================
// Combined state for chat handlers
// ============================================================================

enum ChatBackend {
    Single(Arc<ServerState>),
    Multi(Arc<ProjectManager>),
}

pub struct ChatState {
    backend: ChatBackend,
    pub admin: Arc<AdminState>,
}

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct ChatRequest {
    messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatStatusResponse {
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ChatMessage>,
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

pub fn create_chat_router(server: Arc<ServerState>, admin: Arc<AdminState>) -> Router {
    let state = Arc::new(ChatState {
        backend: ChatBackend::Single(server),
        admin,
    });
    Router::new()
        .route("/api/v2/chat", post(chat_stream))
        .route("/api/v2/chat/status", get(chat_status))
        .with_state(state)
}

pub fn create_chat_router_multi(
    project_manager: Arc<ProjectManager>,
    admin: Arc<AdminState>,
) -> Router {
    let state = Arc::new(ChatState {
        backend: ChatBackend::Multi(project_manager),
        admin,
    });
    Router::new()
        .route("/api/v2/chat", post(chat_stream))
        .route("/api/v2/chat/status", get(chat_status))
        .with_state(state)
}

// ============================================================================
// Helpers
// ============================================================================

async fn resolve_server_state(
    state: &ChatState,
    headers: &HeaderMap,
) -> Result<Arc<ServerState>, (StatusCode, Json<serde_json::Value>)> {
    match &state.backend {
        ChatBackend::Single(server) => Ok(server.clone()),
        ChatBackend::Multi(project_manager) => {
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

fn send_unauthorized() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "Unauthorized" })),
    )
}

// ============================================================================
// Handlers
// ============================================================================

async fn chat_status(
    State(state): State<Arc<ChatState>>,
    headers: HeaderMap,
) -> Json<ChatStatusResponse> {
    if !state.admin.authorize_server(&headers) {
        return Json(ChatStatusResponse {
            available: false,
            reason: Some("Unauthorized".to_string()),
        });
    }

    let provider = LlmProvider::from_env();
    let key_name = provider.api_key_name();
    let keys = state.admin.api_keys.read().await;
    let has_key = keys.get(key_name).map(|k| !k.is_empty()).unwrap_or(false);
    drop(keys);

    let backend = match resolve_server_state(&state, &headers).await {
        Ok(b) => b,
        Err(_) => {
            return Json(ChatStatusResponse {
                available: false,
                reason: Some("Missing or invalid project".to_string()),
            })
        }
    };

    let store = backend.store.read().await;
    let req_count = store.requirements.iter().filter(|r| !r.archived).count();

    if has_key {
        Json(ChatStatusResponse {
            available: true,
            reason: Some(format!("{} requirements loaded", req_count)),
        })
    } else {
        Json(ChatStatusResponse {
            available: false,
            reason: Some(format!(
                "{} not set — configure it in Settings -> Admin -> API Keys",
                key_name
            )),
        })
    }
}

async fn chat_stream(
    State(state): State<Arc<ChatState>>,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<ReceiverStream<Result<Event, axum::Error>>>, impl IntoResponse> {
    if !state.admin.authorize_server(&headers) {
        return Err(send_unauthorized());
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

    let backend = resolve_server_state(&state, &headers).await?;
    backend.check_reload().await;

    let store = backend.store.read().await;
    let project_context = prompts::build_project_context(&store);
    let requirements_summary = prompts::build_all_requirements_summary(&store);
    drop(store);

    let git_context = build_git_context().await;

    let system_prompt = format!(
        r#"You are AIDA Chat, an AI assistant that helps project managers and stakeholders understand project requirements, status, priorities, and progress.

You have access to the full requirements database and recent git history for this project. When answering questions:
- Always reference specific requirement IDs (spec IDs like FR-0042, STORY-0365, etc.) so they can be linked
- Be concise and actionable
- Focus on status, priorities, blockers, and progress
- When listing items, include their spec ID, status, and priority
- Use markdown formatting for readability

{project_context}

{requirements_summary}

{git_context}"#
    );

    let model = provider.resolve_model("AIDA_CHAT_MODEL", provider.default_chat_model());
    let endpoint = format!("{}/v1/messages", provider.base_url());

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
                    .post(endpoint)
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
                        data.push_str(d);
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

// ============================================================================
// Git context helper
// ============================================================================

/// Gather recent git log for chat context. Returns empty string if not in a git repo.
async fn build_git_context() -> String {
    tokio::task::spawn_blocking(|| {
        use std::process::Command;

        let output = Command::new("git")
            .args([
                "log",
                "--pretty=format:%h %ad %an | %s",
                "--date=short",
                "-50",
            ])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let log = String::from_utf8_lossy(&out.stdout).to_string();
                if log.is_empty() {
                    return String::new();
                }
                format!("## Recent Git Commits (last 50)\n{}", log)
            }
            _ => String::new(),
        }
    })
    .await
    .unwrap_or_default()
}
