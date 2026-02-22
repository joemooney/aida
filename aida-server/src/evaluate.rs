// trace:STORY-0375 | ai:claude
//! Evaluate endpoint for AIDA — calls Claude API to evaluate requirement quality
//! and stores the result on the requirement.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::Serialize;
use tracing::error;

use aida_core::ai::{prompts, responses};
use aida_core::ai::responses::StoredAiEvaluation;
use crate::admin::AdminState;
use crate::service::ServerState;

// ============================================================================
// Combined state for evaluate handlers
// ============================================================================

pub struct EvalState {
    pub server: Arc<ServerState>,
    pub admin: Arc<AdminState>,
}

// ============================================================================
// Types
// ============================================================================

/// Minimal Claude API request (non-streaming)
#[derive(Serialize)]
struct ClaudeRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<ClaudeMessage>,
    stream: bool,
}

#[derive(Serialize)]
struct ClaudeMessage {
    role: String,
    content: String,
}

// ============================================================================
// Router
// ============================================================================

pub fn create_evaluate_router(server: Arc<ServerState>, admin: Arc<AdminState>) -> Router {
    let state = Arc::new(EvalState { server, admin });
    Router::new()
        .route("/api/v2/requirements/:id/evaluate", post(evaluate_requirement))
        .with_state(state)
}

// ============================================================================
// Handler
// ============================================================================

async fn evaluate_requirement(
    State(state): State<Arc<EvalState>>,
    Path(id): Path<String>,
) -> Result<Json<StoredAiEvaluation>, impl IntoResponse> {
    // 1. Validate API key from runtime store
    let keys = state.admin.api_keys.read().await;
    let api_key = keys.get("ANTHROPIC_API_KEY").cloned();
    drop(keys);

    let api_key = match api_key {
        Some(key) if !key.is_empty() => key,
        _ => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "ANTHROPIC_API_KEY not configured — set it in Settings → Admin → API Keys" })),
            ));
        }
    };

    // 2. Load requirement and build prompt
    state.server.check_reload().await;
    let store = state.server.store.read().await;

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

    // 3. Call Claude API (non-streaming)
    let model = std::env::var("AIDA_CHAT_MODEL")
        .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());

    let claude_req = ClaudeRequest {
        model,
        max_tokens: 4096,
        messages: vec![ClaudeMessage {
            role: "user".to_string(),
            content: prompt,
        }],
        stream: false,
    };

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
                return Err((
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": format!("Claude API error: {}", status) })),
                ));
            }
            r
        }
        Err(e) => {
            error!("Failed to reach Claude API: {}", e);
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("Failed to reach Claude API: {}", e) })),
            ));
        }
    };

    // 4. Extract text from response
    let body: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to parse Claude response: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to parse Claude response" })),
            ));
        }
    };

    let text = body
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|block| block.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    // 5. Parse evaluation response
    let eval_response = match responses::parse_evaluation_response(text) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to parse evaluation: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to parse evaluation: {}", e) })),
            ));
        }
    };

    // 6. Store result on the requirement
    let content_hash = req.content_hash();
    let stored_eval = StoredAiEvaluation::new(eval_response, content_hash);

    let mut store = state.server.store.write().await;
    let idx = find_requirement_index(&store, &id);
    if let Some(idx) = idx {
        store.requirements[idx].ai_evaluation = Some(stored_eval.clone());
        store.requirements[idx].modified_at = chrono::Utc::now();

        // Persist changes
        if let Err(e) = state.server.backend.save(&store) {
            error!("Failed to save evaluation: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to save: {}", e) })),
            ));
        }
    }
    drop(store);

    Ok(Json(stored_eval))
}

// ============================================================================
// Helpers (same as rest.rs)
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
