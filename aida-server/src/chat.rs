// trace:STORY-0374 | ai:claude
//! Chat endpoints for AIDA — streams AI responses from the Claude API
//! with full requirements context for PM/stakeholder Q&A.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
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

use aida_core::ai::prompts;
use crate::service::ServerState;

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

/// Minimal subset of the Claude API request
#[derive(Serialize)]
struct ClaudeRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ChatMessage>,
    stream: bool,
}

// ============================================================================
// Router
// ============================================================================

pub fn create_chat_router(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/api/v2/chat", post(chat_stream))
        .route("/api/v2/chat/status", get(chat_status))
        .with_state(state)
}

/// Stub router for multi-project mode (chat not yet supported)
pub fn create_chat_router_stub() -> Router {
    Router::new()
        .route("/api/v2/chat/status", get(chat_status_unavailable))
}

// ============================================================================
// Handlers
// ============================================================================

async fn chat_status(State(state): State<Arc<ServerState>>) -> Json<ChatStatusResponse> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").ok();
    let has_key = api_key.map(|k| !k.is_empty()).unwrap_or(false);

    // Also verify the store is accessible
    let store_ok = state.store.read().await;
    let req_count = store_ok.requirements.iter().filter(|r| !r.archived).count();
    drop(store_ok);

    if has_key {
        Json(ChatStatusResponse {
            available: true,
            reason: Some(format!("{} requirements loaded", req_count)),
        })
    } else {
        Json(ChatStatusResponse {
            available: false,
            reason: Some("ANTHROPIC_API_KEY not set".to_string()),
        })
    }
}

async fn chat_status_unavailable() -> Json<ChatStatusResponse> {
    Json(ChatStatusResponse {
        available: false,
        reason: Some("Chat is not available in multi-project mode yet".to_string()),
    })
}

async fn chat_stream(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<ReceiverStream<Result<Event, axum::Error>>>, impl IntoResponse> {
    // 1. Validate API key
    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) if !key.is_empty() => key,
        _ => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "ANTHROPIC_API_KEY not configured" })),
            ));
        }
    };

    // 2. Validate request
    if req.messages.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "messages array must not be empty" })),
        ));
    }

    // 3. Build system prompt with requirements context
    let store = state.store.read().await;
    let project_context = prompts::build_project_context(&store);
    let requirements_summary = prompts::build_all_requirements_summary(&store);
    drop(store);

    let system_prompt = format!(
        r#"You are AIDA Chat, an AI assistant that helps project managers and stakeholders understand project requirements, status, priorities, and progress.

You have access to the full requirements database for this project. When answering questions:
- Always reference specific requirement IDs (spec IDs like FR-0042, STORY-0365, etc.) so they can be linked
- Be concise and actionable
- Focus on status, priorities, blockers, and progress
- When listing items, include their spec ID, status, and priority
- Use markdown formatting for readability

{project_context}

{requirements_summary}"#
    );

    let model = std::env::var("AIDA_CHAT_MODEL")
        .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());

    let claude_req = ClaudeRequest {
        model,
        max_tokens: 4096,
        system: system_prompt,
        messages: req.messages,
        stream: true,
    };

    // 4. Set up SSE channel
    let (tx, rx) = tokio::sync::mpsc::channel(64);

    // 5. Spawn background task to call Claude API and forward chunks
    tokio::spawn(async move {
        let client = reqwest::Client::new();

        let response = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&claude_req)
            .send()
            .await;

        let response = match response {
            Ok(r) => {
                if !r.status().is_success() {
                    let status = r.status();
                    let body = r.text().await.unwrap_or_default();
                    error!("Claude API error {}: {}", status, body);
                    let _ = tx
                        .send(Ok(Event::default()
                            .event("error")
                            .data(format!(r#"{{"error":"Claude API error: {}"}}"#, status))))
                        .await;
                    return;
                }
                r
            }
            Err(e) => {
                error!("Failed to reach Claude API: {}", e);
                let _ = tx
                    .send(Ok(Event::default()
                        .event("error")
                        .data(format!(r#"{{"error":"Failed to reach Claude API: {}"}}"#, e))))
                    .await;
                return;
            }
        };

        // 6. Parse Claude's SSE stream
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

            // Process complete SSE events from buffer
            while let Some(event_end) = buffer.find("\n\n") {
                let event_str = buffer[..event_end].to_string();
                buffer = buffer[event_end + 2..].to_string();

                // Parse SSE event lines
                let mut event_type = String::new();
                let mut data = String::new();

                for line in event_str.lines() {
                    if let Some(et) = line.strip_prefix("event: ") {
                        event_type = et.to_string();
                    } else if let Some(d) = line.strip_prefix("data: ") {
                        data = d.to_string();
                    }
                }

                // We only care about content_block_delta events with text
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
                                return; // Client disconnected
                            }
                        }
                    }
                } else if event_type == "message_stop" {
                    let _ = tx
                        .send(Ok(Event::default().event("done").data("{}")))
                        .await;
                    return;
                } else if event_type == "error" {
                    let _ = tx
                        .send(Ok(Event::default().event("error").data(data)))
                        .await;
                    return;
                }
            }
        }

        // Stream ended — send done event
        let _ = tx
            .send(Ok(Event::default().event("done").data("{}")))
            .await;
    });

    Ok(Sse::new(ReceiverStream::new(rx)))
}
