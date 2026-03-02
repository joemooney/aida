// trace:STORY-0375 | ai:claude
//! Evaluate endpoint for AIDA — calls provider APIs to evaluate requirement quality
//! and stores the result on the requirement.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::Serialize;
use tracing::error;

use crate::admin::AdminState;
use crate::llm::LlmProvider;
use crate::projects::ProjectManager;
use crate::service::ServerState;
use aida_core::ai::responses::StoredAiEvaluation;
use aida_core::ai::{prompts, responses};

// ============================================================================
// Combined state for evaluate handlers
// ============================================================================

enum EvalBackend {
    Single(Arc<ServerState>),
    Multi(Arc<ProjectManager>),
}

pub struct EvalState {
    backend: EvalBackend,
    pub admin: Arc<AdminState>,
}

// ============================================================================
// Types
// ============================================================================

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    stream: bool,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
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

pub fn create_evaluate_router(server: Arc<ServerState>, admin: Arc<AdminState>) -> Router {
    let state = Arc::new(EvalState {
        backend: EvalBackend::Single(server),
        admin,
    });
    Router::new()
        .route(
            "/api/v2/requirements/:id/evaluate",
            post(evaluate_requirement),
        )
        .with_state(state)
}

pub fn create_evaluate_router_multi(
    project_manager: Arc<ProjectManager>,
    admin: Arc<AdminState>,
) -> Router {
    let state = Arc::new(EvalState {
        backend: EvalBackend::Multi(project_manager),
        admin,
    });
    Router::new()
        .route(
            "/api/v2/requirements/:id/evaluate",
            post(evaluate_requirement),
        )
        .with_state(state)
}

async fn resolve_server_state(
    state: &EvalState,
    headers: &HeaderMap,
) -> Result<Arc<ServerState>, (StatusCode, Json<serde_json::Value>)> {
    match &state.backend {
        EvalBackend::Single(server) => Ok(server.clone()),
        EvalBackend::Multi(project_manager) => {
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
// Handler
// ============================================================================

async fn evaluate_requirement(
    State(state): State<Arc<EvalState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<StoredAiEvaluation>, impl IntoResponse> {
    if !state.admin.authorize_server(&headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Unauthorized" })),
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

    let backend = resolve_server_state(&state, &headers).await?;

    backend.check_reload().await;
    let store = backend.store.read().await;

    let req = find_requirement(&store, &id);
    let req = match req {
        Some(r) => r.clone(),
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("Requirement not found: {}", id) })),
            ));
        }
    };

    let prompt = prompts::build_evaluation_prompt(&req, &store);
    drop(store);

    let model = provider.resolve_model("AIDA_EVAL_MODEL", provider.default_eval_model());
    let client = reqwest::Client::new();

    let response = match provider {
        LlmProvider::Anthropic => {
            let body = AnthropicRequest {
                model,
                max_tokens: 4096,
                messages: vec![AnthropicMessage {
                    role: "user".to_string(),
                    content: prompt,
                }],
                stream: false,
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
            let body = OpenAiChatRequest {
                model,
                max_tokens: 4096,
                stream: false,
                messages: vec![
                    OpenAiMessage {
                        role: "system".to_string(),
                        content: "You are a requirements quality evaluator. Return structured content as requested.".to_string(),
                    },
                    OpenAiMessage {
                        role: "user".to_string(),
                        content: prompt,
                    },
                ],
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
                return Err((
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": format!("Provider API error: {}", status) })),
                ));
            }
            r
        }
        Err(e) => {
            error!("Failed to reach provider API: {}", e);
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(
                    serde_json::json!({ "error": format!("Failed to reach provider API: {}", e) }),
                ),
            ));
        }
    };

    let body: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to parse provider response: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to parse provider response" })),
            ));
        }
    };

    let text = match provider {
        LlmProvider::Anthropic => body
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|block| block.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        LlmProvider::OpenAi => body
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|msg| msg.get("content"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
    };

    let eval_response = match responses::parse_evaluation_response(&text) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to parse evaluation: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to parse evaluation: {}", e) })),
            ));
        }
    };

    let content_hash = req.content_hash();
    let stored_eval = StoredAiEvaluation::new(eval_response, content_hash);

    let mut store = backend.store.write().await;
    let idx = find_requirement_index(&store, &id);
    if let Some(idx) = idx {
        store.requirements[idx].ai_evaluation = Some(stored_eval.clone());
        store.requirements[idx].modified_at = chrono::Utc::now();

        if let Err(e) = backend.backend.save(&store) {
            error!("Failed to save evaluation: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to save: {}", e) })),
            ));
        }
        backend.mark_saved().await;
    }

    Ok(Json(stored_eval))
}

// ============================================================================
// Helpers
// ============================================================================

fn find_requirement<'a>(
    store: &'a aida_core::RequirementsStore,
    id: &str,
) -> Option<&'a aida_core::Requirement> {
    if let Ok(uuid) = uuid::Uuid::parse_str(id) {
        return store.requirements.iter().find(|r| r.id == uuid);
    }
    store
        .requirements
        .iter()
        .find(|r| r.spec_id.as_ref().map(|s| s == id).unwrap_or(false))
}

fn find_requirement_index(store: &aida_core::RequirementsStore, id: &str) -> Option<usize> {
    if let Ok(uuid) = uuid::Uuid::parse_str(id) {
        return store.requirements.iter().position(|r| r.id == uuid);
    }
    store
        .requirements
        .iter()
        .position(|r| r.spec_id.as_ref().map(|s| s == id).unwrap_or(false))
}
