// trace:FR-0227 | ai:claude:high
//! REST API implementation for AIDA requirements management
//!
//! Provides JSON-based REST endpoints that mirror the gRPC service.
//! Supports multi-project via X-Project header.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post, put},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::convert::*;
use crate::projects::ProjectManager;
use crate::proto;
use crate::service::ServerState;
use crate::web_auth::{extract_token, AuthenticatedUser, OidcError, OidcUserInfo, WebAuthState};
use aida_core::models::{self, QueueEntry};

/// Application state containing the ProjectManager
pub struct AppState {
    pub project_manager: Arc<ProjectManager>,
}

/// Create the REST API router
pub fn create_rest_router(project_manager: Arc<ProjectManager>) -> Router {
    let state = Arc::new(AppState { project_manager });

    Router::new()
        // Project management (no X-Project header required)
        .route("/api/projects", get(list_projects))
        .route("/api/projects", post(create_project))
        .route("/api/projects/:name", get(get_project))
        .route("/api/projects/:name", delete(delete_project))
        // Server status
        .route("/api/status", get(get_status))
        .route("/api/ping", get(ping))
        // Store operations (require X-Project header)
        .route("/api/store", get(get_store))
        .route("/api/store/metadata", get(get_store_metadata))
        // Requirements CRUD (require X-Project header)
        .route("/api/requirements", get(list_requirements))
        .route("/api/requirements", post(create_requirement))
        .route("/api/requirements/:id", get(get_requirement))
        .route("/api/requirements/:id", put(update_requirement))
        .route("/api/requirements/:id", delete(delete_requirement))
        // Comments
        .route("/api/requirements/:id/comments", post(add_comment))
        // Search
        .route("/api/search", get(search_requirements))
        // New V2 API routes
        .route("/api/v2/auth/config", get(auth_config))
        .route("/api/v2/auth/login", post(auth_login))
        .route("/api/v2/auth/oidc/start", get(auth_oidc_start))
        .route("/api/v2/auth/oidc/callback", get(auth_oidc_callback))
        .route("/api/v2/auth/me", get(auth_me))
        .route("/api/v2/auth/logout", post(auth_logout))
        .route("/api/v2/auth/pin", put(auth_set_pin))
        .route("/api/v2/auth/register", post(auth_register))
        .route("/api/v2/users", get(list_users))
        .route("/api/v2/requirements", get(list_requirements_v2))
        .with_state(state)
}

/// Legacy router for backwards compatibility (single project mode)
pub fn create_rest_router_legacy(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/api/status", get(get_status_legacy))
        .route("/api/ping", get(ping_legacy))
        .route("/api/store", get(get_store_legacy))
        .route("/api/store/metadata", get(get_store_metadata_legacy))
        .route("/api/requirements", get(list_requirements_legacy))
        .route("/api/requirements", post(create_requirement_legacy))
        .route("/api/requirements/:id", get(get_requirement_legacy))
        .route("/api/requirements/:id", put(update_requirement_legacy))
        .route("/api/requirements/:id", delete(delete_requirement_legacy))
        .route("/api/requirements/:id/comments", post(add_comment_legacy))
        .route("/api/search", get(search_requirements_legacy))
        // V2 API routes (native JSON matching TypeScript types)
        .route("/api/v2/auth/config", get(auth_config_legacy))
        .route("/api/v2/auth/login", post(auth_login_legacy))
        .route("/api/v2/auth/oidc/start", get(auth_oidc_start_legacy))
        .route("/api/v2/auth/oidc/callback", get(auth_oidc_callback_legacy))
        .route("/api/v2/auth/me", get(auth_me))
        .route("/api/v2/auth/logout", post(auth_logout))
        .route("/api/v2/auth/pin", put(auth_set_pin_legacy))
        .route("/api/v2/auth/register", post(auth_register_legacy))
        .route("/api/v2/requirements", get(list_requirements_v2_legacy))
        .route("/api/v2/requirements", post(create_requirement_v2_legacy))
        .route("/api/v2/requirements/:id", get(get_requirement_v2_legacy))
        .route(
            "/api/v2/requirements/:id",
            put(update_requirement_v2_legacy),
        )
        .route("/api/v2/search", get(search_requirements_v2_legacy))
        // Sprint assignment endpoints
        .route("/api/v2/requirements/:id/sprint", put(assign_sprint_legacy))
        .route(
            "/api/v2/requirements/:id/sprint",
            delete(remove_sprint_legacy),
        )
        // Skills browser endpoints
        .route("/api/v2/skills", get(list_skills))
        .route("/api/v2/skills/:name", get(get_skill))
        .route("/api/v2/skills/:name", put(update_skill))
        // Docs browser endpoints
        .route("/api/v2/docs", get(list_docs))
        .route("/api/v2/docs/*path", get(get_doc))
        // Settings endpoints
        .route(
            "/api/v2/settings/metadata",
            get(get_settings_metadata).put(update_settings_metadata),
        )
        .route(
            "/api/v2/settings/relationship-definitions",
            get(list_relationship_defs).post(create_relationship_def),
        )
        .route(
            "/api/v2/settings/relationship-definitions/:name",
            put(update_relationship_def).delete(delete_relationship_def),
        )
        .route(
            "/api/v2/settings/type-definitions",
            get(list_type_defs).post(create_type_def),
        )
        .route(
            "/api/v2/settings/type-definitions/:name",
            put(update_type_def).delete(delete_type_def),
        )
        .route(
            "/api/v2/settings/reaction-definitions",
            get(list_reaction_defs).post(create_reaction_def),
        )
        .route(
            "/api/v2/settings/reaction-definitions/:name",
            put(update_reaction_def).delete(delete_reaction_def),
        )
        .route(
            "/api/v2/settings/id-config",
            get(get_id_config).put(update_id_config),
        )
        .route(
            "/api/v2/settings/prefixes",
            get(get_prefixes).put(update_prefixes),
        )
        // Queue endpoints (STORY-0367)
        .route("/api/v2/queue/:user_id", get(queue_list).post(queue_add))
        .route(
            "/api/v2/queue/:user_id/:req_id",
            delete(queue_remove).patch(queue_update),
        )
        .route("/api/v2/queue/:user_id/reorder", post(queue_reorder))
        // Parent assignment endpoint
        .route("/api/v2/requirements/:id/parent", put(set_parent_legacy))
        // Reload endpoint
        .route("/api/v2/reload", post(reload_legacy))
        // Analytics endpoint
        .route("/api/v2/analytics", get(get_analytics))
        // Jira sync endpoint
        .route("/api/v2/jira/sync", get(get_jira_sync))
        .with_state(state)
}

// ============================================================================
// Response types
// ============================================================================

#[derive(Serialize)]
struct ApiError {
    error: String,
    code: u16,
}

impl ApiError {
    fn new(code: StatusCode, message: impl Into<String>) -> (StatusCode, Json<Self>) {
        (
            code,
            Json(Self {
                error: message.into(),
                code: code.as_u16(),
            }),
        )
    }
}

#[derive(Serialize)]
struct PingResponse {
    status: String,
    version: String,
    uptime_seconds: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectResponse {
    name: String,
    description: String,
    created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectListResponse {
    projects: Vec<ProjectResponse>,
}

// ============================================================================
// Query parameters
// ============================================================================

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    status: Option<String>,
    priority: Option<String>,
    // URL-param canonical name stays `type` (idiomatic for query
    // strings); add `reqType` alias so the same name works in body and
    // query contexts. trace:BUG-1-050 | ai:claude
    #[serde(rename = "type", alias = "reqType")]
    req_type: Option<String>,
    feature: Option<String>,
    owner: Option<String>,
    include_archived: Option<bool>,
    limit: Option<i32>,
    offset: Option<i32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SearchQuery {
    q: Option<String>,
    search_title: Option<bool>,
    search_description: Option<bool>,
    search_comments: Option<bool>,
    search_spec_id: Option<bool>,
    status: Option<String>,
    // URL-param canonical name stays `type` (idiomatic for query
    // strings); add `reqType` alias so the same name works in body and
    // query contexts. trace:BUG-1-050 | ai:claude
    #[serde(rename = "type", alias = "reqType")]
    req_type: Option<String>,
    feature: Option<String>,
    include_archived: Option<bool>,
    limit: Option<i32>,
}

// ============================================================================
// Request types
// ============================================================================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProjectRequest {
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRequirementRequest {
    title: String,
    description: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    owner: Option<String>,
    feature: Option<String>,
    // Accept BOTH `reqType` (matches the read-side response shape — what
    // a client naturally tries first based on GET /api/requirements
    // output) AND `type` (back-compat with prior callers that knew
    // about the rename). trace:BUG-1-050 | ai:claude
    #[serde(rename = "reqType", alias = "type")]
    req_type: Option<String>,
    tags: Option<Vec<String>>,
    prefix_override: Option<String>,
    created_by: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateRequirementRequest {
    title: Option<String>,
    description: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    owner: Option<String>,
    feature: Option<String>,
    // See CreateRequirementRequest — accept reqType OR type.
    // trace:BUG-1-050 | ai:claude
    #[serde(rename = "reqType", alias = "type")]
    req_type: Option<String>,
    tags: Option<Vec<String>>,
    replace_tags: Option<bool>,
    archived: Option<bool>,
    custom_status: Option<String>,
    custom_priority: Option<String>,
    custom_fields: Option<std::collections::HashMap<String, String>>,
    replace_custom_fields: Option<bool>,
    #[allow(dead_code)]
    // accepted from clients for audit; not yet stamped into the requirement record
    modified_by: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddCommentRequest {
    content: String,
    author: Option<String>,
    #[allow(dead_code)]
    // accepted from clients for threading; not yet wired into the comment add path
    parent_comment_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthLoginRequest {
    identifier: String,
    #[serde(default)]
    pin: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthConfigResponse {
    mode: String,
    auth_enabled: bool,
    pin_enabled: bool,
    oidc_enabled: bool,
    default_role: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthUserResponse {
    id: String,
    spec_id: Option<String>,
    name: String,
    email: String,
    handle: String,
    archived: bool,
    has_pin: bool,
    role: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthLoginResponse {
    authenticated: bool,
    mode: String,
    session_token: String,
    user: AuthUserResponse,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthMeResponse {
    mode: String,
    authenticated: bool,
    user: AuthenticatedUser,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OidcStartResponse {
    mode: String,
    authorization_url: String,
    state: String,
}

#[derive(Deserialize)]
struct OidcCallbackQuery {
    code: String,
    state: String,
}

fn find_user<'a>(users: &'a [models::User], identifier: &str) -> Option<&'a models::User> {
    users.iter().find(|u| {
        u.handle.eq_ignore_ascii_case(identifier)
            || u.email.eq_ignore_ascii_case(identifier)
            || u.name.eq_ignore_ascii_case(identifier)
            || u.spec_id
                .as_ref()
                .map(|s| s.eq_ignore_ascii_case(identifier))
                .unwrap_or(false)
    })
}

fn auth_user_to_response(user: &models::User, role: &str) -> AuthUserResponse {
    AuthUserResponse {
        id: user.id.to_string(),
        spec_id: user.spec_id.clone(),
        name: user.name.clone(),
        email: user.email.clone(),
        handle: user.handle.clone(),
        archived: user.archived,
        has_pin: user.has_pin(),
        role: role.to_string(),
    }
}

fn oidc_identifier(info: &OidcUserInfo) -> Option<String> {
    if let Some(v) = info.preferred_username.as_ref().filter(|v| !v.is_empty()) {
        return Some(v.clone());
    }
    if let Some(v) = info.email.as_ref().filter(|v| !v.is_empty()) {
        return Some(v.clone());
    }
    if let Some(v) = info.sub.as_ref().filter(|v| !v.is_empty()) {
        return Some(v.clone());
    }
    if let Some(v) = info.name.as_ref().filter(|v| !v.is_empty()) {
        return Some(v.clone());
    }
    None
}

// ============================================================================
// Helper: Extract project from headers and get backend
// ============================================================================

async fn get_project_backend(
    app_state: &AppState,
    headers: &HeaderMap,
) -> Result<Arc<ServerState>, (StatusCode, Json<ApiError>)> {
    let project = headers
        .get("x-project")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "Missing X-Project header. Specify which project to access.",
            )
        })?;

    let backend = app_state
        .project_manager
        .get_backend(project)
        .await
        .map_err(|e| ApiError::new(StatusCode::NOT_FOUND, format!("Project error: {}", e)))?;
    backend.check_reload().await;
    Ok(backend)
}

// ============================================================================
// V2 API Handlers (direct aida_core models)
// ============================================================================

async fn auth_config(
    Extension(auth): Extension<Arc<WebAuthState>>,
) -> Result<Json<AuthConfigResponse>, (StatusCode, Json<ApiError>)> {
    Ok(Json(AuthConfigResponse {
        mode: auth.mode().as_str().to_string(),
        auth_enabled: auth.is_enabled(),
        pin_enabled: auth.mode().pin_enabled(),
        oidc_enabled: auth.oidc_enabled(),
        default_role: auth.role_for_handle("").as_str().to_string(),
    }))
}

async fn auth_login(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<Arc<WebAuthState>>,
    headers: HeaderMap,
    Json(body): Json<AuthLoginRequest>,
) -> Result<Json<AuthLoginResponse>, (StatusCode, Json<ApiError>)> {
    if !auth.is_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Authentication is disabled (AIDA_WEB_AUTH_MODE=none)",
        ));
    }
    if !auth.mode().pin_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "PIN login not enabled for this auth mode",
        ));
    }

    let backend = get_project_backend(&state, &headers).await?;
    let project = headers
        .get("x-project")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("default")
        .to_string();
    let store = backend.store.read().await;

    let user = find_user(&store.users, &body.identifier)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "User not found"))?;

    if user.has_pin() && !user.verify_pin(&body.pin) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Invalid PIN"));
    }

    let role = auth.role_for_handle(&user.handle);
    let session_token = auth
        .create_session(
            user.id.to_string(),
            user.handle.clone(),
            user.name.clone(),
            project,
            role,
        )
        .await;

    Ok(Json(AuthLoginResponse {
        authenticated: true,
        mode: auth.mode().as_str().to_string(),
        session_token,
        user: auth_user_to_response(user, role.as_str()),
    }))
}

async fn auth_oidc_start(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<Arc<WebAuthState>>,
    headers: HeaderMap,
) -> Result<Json<OidcStartResponse>, (StatusCode, Json<ApiError>)> {
    if !auth.oidc_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "OIDC is not configured",
        ));
    }

    let project = headers
        .get("x-project")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("default")
        .to_string();

    // Validate project early to avoid generating unusable URLs.
    let _ = state
        .project_manager
        .get_backend(&project)
        .await
        .map_err(|e| ApiError::new(StatusCode::NOT_FOUND, format!("Project error: {}", e)))?;

    let (authorization_url, state_token) = auth
        .build_oidc_authorize_url(&project)
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "Failed to build OIDC URL"))?;

    Ok(Json(OidcStartResponse {
        mode: auth.mode().as_str().to_string(),
        authorization_url,
        state: state_token,
    }))
}

async fn auth_oidc_callback(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<Arc<WebAuthState>>,
    Query(query): Query<OidcCallbackQuery>,
) -> Result<Json<AuthLoginResponse>, (StatusCode, Json<ApiError>)> {
    if !auth.oidc_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "OIDC is not configured",
        ));
    }

    let project = auth
        .consume_oidc_state(&query.state)
        .await
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "Invalid or expired OIDC state"))?;

    let backend = state
        .project_manager
        .get_backend(&project)
        .await
        .map_err(|e| ApiError::new(StatusCode::NOT_FOUND, format!("Project error: {}", e)))?;

    let userinfo = auth
        .exchange_oidc_code(&query.code)
        .await
        .map_err(|e| match e {
            OidcError::ExchangeFailed(msg) | OidcError::UserInfoFailed(msg) => {
                ApiError::new(StatusCode::UNAUTHORIZED, msg)
            }
            _ => ApiError::new(StatusCode::UNAUTHORIZED, "OIDC authentication failed"),
        })?;

    let identifier = oidc_identifier(&userinfo).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "OIDC user payload missing identifier",
        )
    })?;
    let store = backend.store.read().await;
    let user = find_user(&store.users, &identifier).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "No matching AIDA user for OIDC identity",
        )
    })?;

    let role = auth.role_for_handle(&user.handle);
    let session_token = auth
        .create_session(
            user.id.to_string(),
            user.handle.clone(),
            user.name.clone(),
            project,
            role,
        )
        .await;

    Ok(Json(AuthLoginResponse {
        authenticated: true,
        mode: auth.mode().as_str().to_string(),
        session_token,
        user: auth_user_to_response(user, role.as_str()),
    }))
}

async fn auth_me(
    Extension(auth): Extension<Arc<WebAuthState>>,
    user: Option<Extension<AuthenticatedUser>>,
) -> Result<Json<AuthMeResponse>, (StatusCode, Json<ApiError>)> {
    let user = user
        .map(|u| u.0)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "Not authenticated"))?;
    Ok(Json(AuthMeResponse {
        mode: auth.mode().as_str().to_string(),
        authenticated: true,
        user,
    }))
}

async fn auth_logout(
    Extension(auth): Extension<Arc<WebAuthState>>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    if let Some(token) = extract_token(&headers) {
        auth.remove_session(&token).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

// ── Account management ──────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterRequest {
    handle: String,
    name: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    pin: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetPinRequest {
    #[serde(default)]
    current_pin: String,
    new_pin: String,
}

async fn auth_register(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<Arc<WebAuthState>>,
    headers: HeaderMap,
    Json(body): Json<RegisterRequest>,
) -> Result<Json<AuthLoginResponse>, (StatusCode, Json<ApiError>)> {
    if !auth.is_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Authentication is disabled",
        ));
    }

    let handle = body.handle.trim().to_lowercase();
    if handle.is_empty() || handle.len() > 32 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Handle must be 1-32 characters",
        ));
    }

    let backend = get_project_backend(&state, &headers).await?;
    let project = headers
        .get("x-project")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("default")
        .to_string();

    // Check handle uniqueness
    {
        let store = backend.store.read().await;
        if store
            .users
            .iter()
            .any(|u| u.handle.to_lowercase() == handle)
        {
            return Err(ApiError::new(StatusCode::CONFLICT, "Handle already taken"));
        }
    }

    let mut user = models::User::new(
        body.name.trim().to_string(),
        body.email.trim().to_string(),
        handle.clone(),
    );
    if !body.pin.is_empty() {
        user.set_pin(&body.pin);
    }

    let mut store = backend.store.write().await;
    store.add_user(user.clone());
    if let Err(e) = backend.backend.save(&store) {
        tracing::error!("Failed to save after register: {e}");
    }

    let role = auth.role_for_handle(&user.handle);
    let session_token = auth
        .create_session(
            user.id.to_string(),
            user.handle.clone(),
            user.name.clone(),
            project,
            role,
        )
        .await;

    Ok(Json(AuthLoginResponse {
        authenticated: true,
        mode: auth.mode().as_str().to_string(),
        session_token,
        user: auth_user_to_response(&user, role.as_str()),
    }))
}

async fn auth_set_pin(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<Arc<WebAuthState>>,
    Extension(session_user): Extension<AuthenticatedUser>,
    headers: HeaderMap,
    Json(body): Json<SetPinRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    if !auth.mode().pin_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "PIN auth not enabled",
        ));
    }

    if body.new_pin.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "New PIN cannot be empty",
        ));
    }

    let backend = get_project_backend(&state, &headers).await?;
    let mut store = backend.store.write().await;

    let user = store
        .users
        .iter_mut()
        .find(|u| u.id.to_string() == session_user.user_id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "User not found"))?;

    // If user already has a PIN, verify the current one
    if user.has_pin() && !user.verify_pin(&body.current_pin) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Current PIN is incorrect",
        ));
    }

    user.set_pin(&body.new_pin);
    if let Err(e) = backend.backend.save(&store) {
        tracing::error!("Failed to save after PIN change: {e}");
    }

    Ok(StatusCode::NO_CONTENT)
}

// Legacy variants for single-project mode
async fn auth_register_legacy(
    State(state): State<Arc<ServerState>>,
    Extension(auth): Extension<Arc<WebAuthState>>,
    Json(body): Json<RegisterRequest>,
) -> Result<Json<AuthLoginResponse>, (StatusCode, Json<ApiError>)> {
    if !auth.is_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Authentication is disabled",
        ));
    }

    let handle = body.handle.trim().to_lowercase();
    if handle.is_empty() || handle.len() > 32 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Handle must be 1-32 characters",
        ));
    }

    state.check_reload().await;

    {
        let store = state.store.read().await;
        if store
            .users
            .iter()
            .any(|u| u.handle.to_lowercase() == handle)
        {
            return Err(ApiError::new(StatusCode::CONFLICT, "Handle already taken"));
        }
    }

    let mut user = models::User::new(
        body.name.trim().to_string(),
        body.email.trim().to_string(),
        handle.clone(),
    );
    if !body.pin.is_empty() {
        user.set_pin(&body.pin);
    }

    let mut store = state.store.write().await;
    store.add_user(user.clone());
    if let Err(e) = state.backend.save(&store) {
        tracing::error!("Failed to save after register: {e}");
    }

    let role = auth.role_for_handle(&user.handle);
    let session_token = auth
        .create_session(
            user.id.to_string(),
            user.handle.clone(),
            user.name.clone(),
            "default".to_string(),
            role,
        )
        .await;

    Ok(Json(AuthLoginResponse {
        authenticated: true,
        mode: auth.mode().as_str().to_string(),
        session_token,
        user: auth_user_to_response(&user, role.as_str()),
    }))
}

async fn auth_set_pin_legacy(
    State(state): State<Arc<ServerState>>,
    Extension(auth): Extension<Arc<WebAuthState>>,
    Extension(session_user): Extension<AuthenticatedUser>,
    Json(body): Json<SetPinRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    if !auth.mode().pin_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "PIN auth not enabled",
        ));
    }
    if body.new_pin.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "New PIN cannot be empty",
        ));
    }

    state.check_reload().await;
    let mut store = state.store.write().await;

    let user = store
        .users
        .iter_mut()
        .find(|u| u.id.to_string() == session_user.user_id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "User not found"))?;

    if user.has_pin() && !user.verify_pin(&body.current_pin) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Current PIN is incorrect",
        ));
    }

    user.set_pin(&body.new_pin);
    if let Err(e) = state.backend.save(&store) {
        tracing::error!("Failed to save after PIN change: {e}");
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn list_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<models::User>>, (StatusCode, Json<ApiError>)> {
    let backend = get_project_backend(&state, &headers).await?;
    let store = backend.store.read().await;

    Ok(Json(store.users.clone()))
}

async fn list_requirements_v2(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<models::Requirement>>, (StatusCode, Json<ApiError>)> {
    let backend = get_project_backend(&state, &headers).await?;
    let store = backend.store.read().await;

    Ok(Json(store.requirements.clone()))
}

async fn auth_config_legacy(
    Extension(auth): Extension<Arc<WebAuthState>>,
) -> Result<Json<AuthConfigResponse>, (StatusCode, Json<ApiError>)> {
    Ok(Json(AuthConfigResponse {
        mode: auth.mode().as_str().to_string(),
        auth_enabled: auth.is_enabled(),
        pin_enabled: auth.mode().pin_enabled(),
        oidc_enabled: auth.oidc_enabled(),
        default_role: auth.role_for_handle("").as_str().to_string(),
    }))
}

async fn auth_login_legacy(
    State(state): State<Arc<ServerState>>,
    Extension(auth): Extension<Arc<WebAuthState>>,
    Json(body): Json<AuthLoginRequest>,
) -> Result<Json<AuthLoginResponse>, (StatusCode, Json<ApiError>)> {
    if !auth.is_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Authentication is disabled (AIDA_WEB_AUTH_MODE=none)",
        ));
    }
    if !auth.mode().pin_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "PIN login not enabled for this auth mode",
        ));
    }

    state.check_reload().await;
    let store = state.store.read().await;

    let user = find_user(&store.users, &body.identifier)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "User not found"))?;

    if user.has_pin() && !user.verify_pin(&body.pin) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "Invalid PIN"));
    }

    let role = auth.role_for_handle(&user.handle);
    let session_token = auth
        .create_session(
            user.id.to_string(),
            user.handle.clone(),
            user.name.clone(),
            "default".to_string(),
            role,
        )
        .await;

    Ok(Json(AuthLoginResponse {
        authenticated: true,
        mode: auth.mode().as_str().to_string(),
        session_token,
        user: auth_user_to_response(user, role.as_str()),
    }))
}

async fn auth_oidc_start_legacy(
    Extension(auth): Extension<Arc<WebAuthState>>,
) -> Result<Json<OidcStartResponse>, (StatusCode, Json<ApiError>)> {
    if !auth.oidc_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "OIDC is not configured",
        ));
    }

    let (authorization_url, state_token) = auth
        .build_oidc_authorize_url("default")
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "Failed to build OIDC URL"))?;

    Ok(Json(OidcStartResponse {
        mode: auth.mode().as_str().to_string(),
        authorization_url,
        state: state_token,
    }))
}

async fn auth_oidc_callback_legacy(
    State(state): State<Arc<ServerState>>,
    Extension(auth): Extension<Arc<WebAuthState>>,
    Query(query): Query<OidcCallbackQuery>,
) -> Result<Json<AuthLoginResponse>, (StatusCode, Json<ApiError>)> {
    if !auth.oidc_enabled() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "OIDC is not configured",
        ));
    }

    let _project = auth
        .consume_oidc_state(&query.state)
        .await
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "Invalid or expired OIDC state"))?;

    let userinfo = auth
        .exchange_oidc_code(&query.code)
        .await
        .map_err(|e| match e {
            OidcError::ExchangeFailed(msg) | OidcError::UserInfoFailed(msg) => {
                ApiError::new(StatusCode::UNAUTHORIZED, msg)
            }
            _ => ApiError::new(StatusCode::UNAUTHORIZED, "OIDC authentication failed"),
        })?;
    let identifier = oidc_identifier(&userinfo).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "OIDC user payload missing identifier",
        )
    })?;

    state.check_reload().await;
    let store = state.store.read().await;
    let user = find_user(&store.users, &identifier).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "No matching AIDA user for OIDC identity",
        )
    })?;

    let role = auth.role_for_handle(&user.handle);
    let session_token = auth
        .create_session(
            user.id.to_string(),
            user.handle.clone(),
            user.name.clone(),
            "default".to_string(),
            role,
        )
        .await;

    Ok(Json(AuthLoginResponse {
        authenticated: true,
        mode: auth.mode().as_str().to_string(),
        session_token,
        user: auth_user_to_response(user, role.as_str()),
    }))
}

// ============================================================================
// Project management handlers
// ============================================================================

async fn list_projects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ProjectListResponse>, (StatusCode, Json<ApiError>)> {
    let projects = state.project_manager.list_projects().await;
    Ok(Json(ProjectListResponse {
        projects: projects
            .into_iter()
            .map(|p| ProjectResponse {
                name: p.name,
                description: p.description,
                created_at: p.created_at.to_rfc3339(),
            })
            .collect(),
    }))
}

async fn create_project(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<ProjectResponse>), (StatusCode, Json<ApiError>)> {
    let project = state
        .project_manager
        .create_project(&body.name, &body.description.unwrap_or_default())
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(ProjectResponse {
            name: project.name,
            description: project.description,
            created_at: project.created_at.to_rfc3339(),
        }),
    ))
}

async fn get_project(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<ProjectResponse>, (StatusCode, Json<ApiError>)> {
    let project = state
        .project_manager
        .get_project(&name)
        .await
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                format!("Project not found: {}", name),
            )
        })?;

    Ok(Json(ProjectResponse {
        name: project.name,
        description: project.description,
        created_at: project.created_at.to_rfc3339(),
    }))
}

async fn delete_project(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    state
        .project_manager
        .delete_project(&name)
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Project '{}' deleted", name)
    })))
}

// ============================================================================
// Server status handlers
// ============================================================================

async fn ping(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(PingResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: 0, // TODO: Track server start time
    })
}

async fn get_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<proto::GetServerStatusResponse>, (StatusCode, Json<ApiError>)> {
    // If X-Project header is provided, get status for that project
    if let Some(project_header) = headers.get("x-project") {
        if let Ok(project) = project_header.to_str() {
            if let Ok(backend) = state.project_manager.get_backend(project).await {
                let backend_type = backend.backend.backend_type();
                return Ok(Json(proto::GetServerStatusResponse {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    status: "running".to_string(),
                    uptime_seconds: backend.start_time.elapsed().as_secs() as i64,
                    active_connections: 0,
                    storage_backend: backend_type.to_string().to_lowercase(),
                    storage_path: project.to_string(),
                }));
            }
        }
    }

    // Generic status without project context
    Ok(Json(proto::GetServerStatusResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        status: "running".to_string(),
        uptime_seconds: 0,
        active_connections: 0,
        storage_backend: "multi-project".to_string(),
        storage_path: state
            .project_manager
            .data_dir()
            .to_string_lossy()
            .to_string(),
    }))
}

// ============================================================================
// Store handlers (require X-Project header)
// ============================================================================

async fn get_store(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<proto::GetStoreResponse>, (StatusCode, Json<ApiError>)> {
    let backend = get_project_backend(&state, &headers).await?;
    let store = backend.store.read().await;
    Ok(Json(proto::GetStoreResponse {
        store: Some(store_to_proto(&store)),
    }))
}

async fn get_store_metadata(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<proto::GetStoreMetadataResponse>, (StatusCode, Json<ApiError>)> {
    let backend = get_project_backend(&state, &headers).await?;
    let store = backend.store.read().await;
    Ok(Json(proto::GetStoreMetadataResponse {
        name: store.name.clone(),
        title: store.title.clone(),
        description: store.description.clone(),
        requirement_count: store.requirements.len() as i32,
        user_count: store.users.len() as i32,
        feature_count: store.features.len() as i32,
    }))
}

// ============================================================================
// Requirements handlers (require X-Project header)
// ============================================================================

async fn list_requirements(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<proto::ListRequirementsResponse>, (StatusCode, Json<ApiError>)> {
    let backend = get_project_backend(&state, &headers).await?;
    let store = backend.store.read().await;

    let mut requirements: Vec<_> = store
        .requirements
        .iter()
        .filter(|req| {
            if !query.include_archived.unwrap_or(false) && req.archived {
                return false;
            }
            if let Some(ref status) = query.status {
                if format!("{:?}", req.status).to_lowercase() != status.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref priority) = query.priority {
                if format!("{:?}", req.priority).to_lowercase() != priority.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref req_type) = query.req_type {
                if format!("{:?}", req.req_type).to_lowercase() != req_type.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref feature) = query.feature {
                if req.feature.to_lowercase() != feature.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref owner) = query.owner {
                if req.owner.to_lowercase() != owner.to_lowercase() {
                    return false;
                }
            }
            true
        })
        .map(requirement_to_proto)
        .collect();

    let total_count = requirements.len() as i32;

    let offset = query.offset.unwrap_or(0) as usize;
    let limit = query.limit.unwrap_or(0) as usize;

    if offset > 0 {
        requirements = requirements.into_iter().skip(offset).collect();
    }
    if limit > 0 {
        requirements = requirements.into_iter().take(limit).collect();
    }

    Ok(Json(proto::ListRequirementsResponse {
        requirements,
        total_count,
    }))
}

async fn get_requirement(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<proto::GetRequirementResponse>, (StatusCode, Json<ApiError>)> {
    let backend = get_project_backend(&state, &headers).await?;
    let store = backend.store.read().await;

    let req = find_requirement(&store, &id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {}", id),
        )
    })?;

    Ok(Json(proto::GetRequirementResponse {
        requirement: Some(requirement_to_proto(req)),
    }))
}

async fn create_requirement(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateRequirementRequest>,
) -> Result<(StatusCode, Json<proto::CreateRequirementResponse>), (StatusCode, Json<ApiError>)> {
    let backend = get_project_backend(&state, &headers).await?;
    let mut store = backend.store.write().await;

    let mut new_req = aida_core::Requirement::new(body.title, body.description.unwrap_or_default());
    new_req.status = parse_status(&body.status.unwrap_or_else(|| "Draft".to_string()));
    new_req.priority = parse_priority(&body.priority.unwrap_or_else(|| "Medium".to_string()));
    new_req.req_type = parse_req_type(&body.req_type.unwrap_or_else(|| "Functional".to_string()));
    new_req.owner = body.owner.unwrap_or_default();
    new_req.feature = body.feature.unwrap_or_else(|| "Uncategorized".to_string());
    new_req.prefix_override = body.prefix_override;
    new_req.created_by = body.created_by;
    if let Some(tags) = body.tags {
        new_req.tags = tags.into_iter().collect();
    }

    let feature_prefix = store
        .features
        .iter()
        .find(|f| f.name == new_req.feature)
        .map(|f| f.prefix.clone());
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

    store.add_requirement_with_id(
        new_req.clone(),
        feature_prefix.as_deref(),
        type_prefix.as_deref(),
    );

    let added_req = store.requirements.last().ok_or_else(|| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to add requirement",
        )
    })?;
    let spec_id = added_req.spec_id.clone().unwrap_or_default();
    let proto_req = requirement_to_proto(added_req);

    drop(store);
    if let Err(e) = backend.backend.save(&*backend.store.read().await) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save: {}", e),
        ));
    }

    Ok((
        StatusCode::CREATED,
        Json(proto::CreateRequirementResponse {
            requirement: Some(proto_req),
            spec_id,
        }),
    ))
}

async fn update_requirement(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateRequirementRequest>,
) -> Result<Json<proto::UpdateRequirementResponse>, (StatusCode, Json<ApiError>)> {
    let backend = get_project_backend(&state, &headers).await?;
    let mut store = backend.store.write().await;

    let idx = find_requirement_index(&store, &id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {}", id),
        )
    })?;

    let req = &mut store.requirements[idx];

    if let Some(title) = body.title {
        req.title = title;
    }
    if let Some(description) = body.description {
        req.description = description;
    }
    if let Some(status) = body.status {
        req.status = parse_status(&status);
    }
    if let Some(priority) = body.priority {
        req.priority = parse_priority(&priority);
    }
    if let Some(owner) = body.owner {
        req.owner = owner;
    }
    if let Some(feature) = body.feature {
        req.feature = feature;
    }
    if let Some(req_type) = body.req_type {
        req.req_type = parse_req_type(&req_type);
    }
    if let Some(archived) = body.archived {
        req.archived = archived;
    }
    if let Some(custom_status) = body.custom_status {
        req.custom_status = Some(custom_status);
    }
    if let Some(custom_priority) = body.custom_priority {
        req.custom_priority = Some(custom_priority);
    }
    if let Some(tags) = body.tags {
        if body.replace_tags.unwrap_or(false) {
            req.tags = tags.into_iter().collect();
        } else {
            req.tags.extend(tags);
        }
    }
    if let Some(custom_fields) = body.custom_fields {
        if body.replace_custom_fields.unwrap_or(false) {
            req.custom_fields = custom_fields;
        } else {
            req.custom_fields.extend(custom_fields);
        }
    }

    req.modified_at = chrono::Utc::now();

    let proto_req = requirement_to_proto(req);

    drop(store);
    if let Err(e) = backend.backend.save(&*backend.store.read().await) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save: {}", e),
        ));
    }

    Ok(Json(proto::UpdateRequirementResponse {
        requirement: Some(proto_req),
    }))
}

async fn delete_requirement(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<proto::DeleteRequirementResponse>, (StatusCode, Json<ApiError>)> {
    let backend = get_project_backend(&state, &headers).await?;
    let mut store = backend.store.write().await;

    let idx = find_requirement_index(&store, &id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {}", id),
        )
    })?;

    store.requirements.remove(idx);

    drop(store);
    if let Err(e) = backend.backend.save(&*backend.store.read().await) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save: {}", e),
        ));
    }

    Ok(Json(proto::DeleteRequirementResponse {
        success: true,
        message: format!("Deleted requirement: {}", id),
    }))
}

async fn add_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<AddCommentRequest>,
) -> Result<(StatusCode, Json<proto::AddCommentResponse>), (StatusCode, Json<ApiError>)> {
    let backend = get_project_backend(&state, &headers).await?;
    let mut store = backend.store.write().await;

    let idx = find_requirement_index(&store, &id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {}", id),
        )
    })?;

    let comment = aida_core::Comment::new(
        body.content,
        body.author.unwrap_or_else(|| "anonymous".to_string()),
    );

    let proto_comment = comment_to_proto(&comment);
    store.requirements[idx].comments.push(comment);

    drop(store);
    if let Err(e) = backend.backend.save(&*backend.store.read().await) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save: {}", e),
        ));
    }

    Ok((
        StatusCode::CREATED,
        Json(proto::AddCommentResponse {
            comment: Some(proto_comment),
        }),
    ))
}

async fn search_requirements(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<Json<proto::SearchRequirementsResponse>, (StatusCode, Json<ApiError>)> {
    let backend = get_project_backend(&state, &headers).await?;
    let store = backend.store.read().await;
    let search_text = query.q.unwrap_or_default().to_lowercase();

    let search_title = query.search_title.unwrap_or(true);
    let search_description = query.search_description.unwrap_or(true);
    let search_comments = query.search_comments.unwrap_or(false);
    let search_spec_id = query.search_spec_id.unwrap_or(true);

    let mut results: Vec<_> = store
        .requirements
        .iter()
        .filter(|req| {
            if !query.include_archived.unwrap_or(false) && req.archived {
                return false;
            }
            if let Some(ref status) = query.status {
                if format!("{:?}", req.status).to_lowercase() != status.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref req_type) = query.req_type {
                if format!("{:?}", req.req_type).to_lowercase() != req_type.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref feature) = query.feature {
                if req.feature.to_lowercase() != feature.to_lowercase() {
                    return false;
                }
            }

            if search_text.is_empty() {
                return true;
            }
            let mut matched = false;
            if search_title && req.title.to_lowercase().contains(&search_text) {
                matched = true;
            }
            if search_description && req.description.to_lowercase().contains(&search_text) {
                matched = true;
            }
            if search_spec_id {
                if let Some(ref spec_id) = req.spec_id {
                    if spec_id.to_lowercase().contains(&search_text) {
                        matched = true;
                    }
                }
            }
            if search_comments {
                for comment in &req.comments {
                    if comment.content.to_lowercase().contains(&search_text) {
                        matched = true;
                        break;
                    }
                }
            }
            matched
        })
        .map(requirement_to_proto)
        .collect();

    let total_matches = results.len() as i32;

    if let Some(limit) = query.limit {
        if limit > 0 {
            results = results.into_iter().take(limit as usize).collect();
        }
    }

    Ok(Json(proto::SearchRequirementsResponse {
        requirements: results,
        total_matches,
    }))
}

// ============================================================================
// Legacy handlers (for backwards compatibility with single-project mode)
// ============================================================================

async fn ping_legacy(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    Json(PingResponse {
        status: "ok".to_string(),
        version: state.version.clone(),
        uptime_seconds: state.start_time.elapsed().as_secs(),
    })
}

async fn get_status_legacy(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<proto::GetServerStatusResponse>, (StatusCode, Json<ApiError>)> {
    let _store = state.store.read().await;
    let backend_type = state.backend.backend_type();

    Ok(Json(proto::GetServerStatusResponse {
        version: state.version.clone(),
        status: "running".to_string(),
        uptime_seconds: state.start_time.elapsed().as_secs() as i64,
        active_connections: 0,
        storage_backend: backend_type.to_string().to_lowercase(),
        storage_path: format!("{}", backend_type),
    }))
}

async fn get_store_legacy(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<proto::GetStoreResponse>, (StatusCode, Json<ApiError>)> {
    let store = state.store.read().await;
    Ok(Json(proto::GetStoreResponse {
        store: Some(store_to_proto(&store)),
    }))
}

async fn get_store_metadata_legacy(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<proto::GetStoreMetadataResponse>, (StatusCode, Json<ApiError>)> {
    let store = state.store.read().await;
    Ok(Json(proto::GetStoreMetadataResponse {
        name: store.name.clone(),
        title: store.title.clone(),
        description: store.description.clone(),
        requirement_count: store.requirements.len() as i32,
        user_count: store.users.len() as i32,
        feature_count: store.features.len() as i32,
    }))
}

async fn list_requirements_legacy(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<proto::ListRequirementsResponse>, (StatusCode, Json<ApiError>)> {
    let store = state.store.read().await;

    let mut requirements: Vec<_> = store
        .requirements
        .iter()
        .filter(|req| {
            if !query.include_archived.unwrap_or(false) && req.archived {
                return false;
            }
            if let Some(ref status) = query.status {
                if format!("{:?}", req.status).to_lowercase() != status.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref priority) = query.priority {
                if format!("{:?}", req.priority).to_lowercase() != priority.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref req_type) = query.req_type {
                if format!("{:?}", req.req_type).to_lowercase() != req_type.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref feature) = query.feature {
                if req.feature.to_lowercase() != feature.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref owner) = query.owner {
                if req.owner.to_lowercase() != owner.to_lowercase() {
                    return false;
                }
            }
            true
        })
        .map(requirement_to_proto)
        .collect();

    let total_count = requirements.len() as i32;

    let offset = query.offset.unwrap_or(0) as usize;
    let limit = query.limit.unwrap_or(0) as usize;

    if offset > 0 {
        requirements = requirements.into_iter().skip(offset).collect();
    }
    if limit > 0 {
        requirements = requirements.into_iter().take(limit).collect();
    }

    Ok(Json(proto::ListRequirementsResponse {
        requirements,
        total_count,
    }))
}

async fn get_requirement_legacy(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Result<Json<proto::GetRequirementResponse>, (StatusCode, Json<ApiError>)> {
    let store = state.store.read().await;

    let req = find_requirement(&store, &id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {}", id),
        )
    })?;

    Ok(Json(proto::GetRequirementResponse {
        requirement: Some(requirement_to_proto(req)),
    }))
}

async fn create_requirement_legacy(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<CreateRequirementRequest>,
) -> Result<(StatusCode, Json<proto::CreateRequirementResponse>), (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;

    let mut new_req = aida_core::Requirement::new(body.title, body.description.unwrap_or_default());
    new_req.status = parse_status(&body.status.unwrap_or_else(|| "Draft".to_string()));
    new_req.priority = parse_priority(&body.priority.unwrap_or_else(|| "Medium".to_string()));
    new_req.req_type = parse_req_type(&body.req_type.unwrap_or_else(|| "Functional".to_string()));
    new_req.owner = body.owner.unwrap_or_default();
    new_req.feature = body.feature.unwrap_or_else(|| "Uncategorized".to_string());
    new_req.prefix_override = body.prefix_override;
    new_req.created_by = body.created_by;
    if let Some(tags) = body.tags {
        new_req.tags = tags.into_iter().collect();
    }

    let feature_prefix = store
        .features
        .iter()
        .find(|f| f.name == new_req.feature)
        .map(|f| f.prefix.clone());
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

    store.add_requirement_with_id(
        new_req.clone(),
        feature_prefix.as_deref(),
        type_prefix.as_deref(),
    );

    let added_req = store.requirements.last().ok_or_else(|| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to add requirement",
        )
    })?;
    let spec_id = added_req.spec_id.clone().unwrap_or_default();
    let proto_req = requirement_to_proto(added_req);

    drop(store);
    if let Err(e) = state.backend.save(&*state.store.read().await) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save: {}", e),
        ));
    }

    Ok((
        StatusCode::CREATED,
        Json(proto::CreateRequirementResponse {
            requirement: Some(proto_req),
            spec_id,
        }),
    ))
}

async fn update_requirement_legacy(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateRequirementRequest>,
) -> Result<Json<proto::UpdateRequirementResponse>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;

    let idx = find_requirement_index(&store, &id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {}", id),
        )
    })?;

    let req = &mut store.requirements[idx];

    if let Some(title) = body.title {
        req.title = title;
    }
    if let Some(description) = body.description {
        req.description = description;
    }
    if let Some(status) = body.status {
        req.status = parse_status(&status);
    }
    if let Some(priority) = body.priority {
        req.priority = parse_priority(&priority);
    }
    if let Some(owner) = body.owner {
        req.owner = owner;
    }
    if let Some(feature) = body.feature {
        req.feature = feature;
    }
    if let Some(req_type) = body.req_type {
        req.req_type = parse_req_type(&req_type);
    }
    if let Some(archived) = body.archived {
        req.archived = archived;
    }
    if let Some(custom_status) = body.custom_status {
        req.custom_status = Some(custom_status);
    }
    if let Some(custom_priority) = body.custom_priority {
        req.custom_priority = Some(custom_priority);
    }
    if let Some(tags) = body.tags {
        if body.replace_tags.unwrap_or(false) {
            req.tags = tags.into_iter().collect();
        } else {
            req.tags.extend(tags);
        }
    }
    if let Some(custom_fields) = body.custom_fields {
        if body.replace_custom_fields.unwrap_or(false) {
            req.custom_fields = custom_fields;
        } else {
            req.custom_fields.extend(custom_fields);
        }
    }

    req.modified_at = chrono::Utc::now();

    let proto_req = requirement_to_proto(req);

    drop(store);
    if let Err(e) = state.backend.save(&*state.store.read().await) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save: {}", e),
        ));
    }

    Ok(Json(proto::UpdateRequirementResponse {
        requirement: Some(proto_req),
    }))
}

async fn delete_requirement_legacy(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Result<Json<proto::DeleteRequirementResponse>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;

    let idx = find_requirement_index(&store, &id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {}", id),
        )
    })?;

    store.requirements.remove(idx);

    drop(store);
    if let Err(e) = state.backend.save(&*state.store.read().await) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save: {}", e),
        ));
    }

    Ok(Json(proto::DeleteRequirementResponse {
        success: true,
        message: format!("Deleted requirement: {}", id),
    }))
}

async fn add_comment_legacy(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(body): Json<AddCommentRequest>,
) -> Result<(StatusCode, Json<proto::AddCommentResponse>), (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;

    let idx = find_requirement_index(&store, &id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {}", id),
        )
    })?;

    let comment = aida_core::Comment::new(
        body.content,
        body.author.unwrap_or_else(|| "anonymous".to_string()),
    );

    let proto_comment = comment_to_proto(&comment);
    store.requirements[idx].comments.push(comment);

    drop(store);
    if let Err(e) = state.backend.save(&*state.store.read().await) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save: {}", e),
        ));
    }

    Ok((
        StatusCode::CREATED,
        Json(proto::AddCommentResponse {
            comment: Some(proto_comment),
        }),
    ))
}

async fn search_requirements_legacy(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<proto::SearchRequirementsResponse>, (StatusCode, Json<ApiError>)> {
    let store = state.store.read().await;
    let search_text = query.q.unwrap_or_default().to_lowercase();

    let search_title = query.search_title.unwrap_or(true);
    let search_description = query.search_description.unwrap_or(true);
    let search_comments = query.search_comments.unwrap_or(false);
    let search_spec_id = query.search_spec_id.unwrap_or(true);

    let mut results: Vec<_> = store
        .requirements
        .iter()
        .filter(|req| {
            if !query.include_archived.unwrap_or(false) && req.archived {
                return false;
            }
            if let Some(ref status) = query.status {
                if format!("{:?}", req.status).to_lowercase() != status.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref req_type) = query.req_type {
                if format!("{:?}", req.req_type).to_lowercase() != req_type.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref feature) = query.feature {
                if req.feature.to_lowercase() != feature.to_lowercase() {
                    return false;
                }
            }

            if search_text.is_empty() {
                return true;
            }
            let mut matched = false;
            if search_title && req.title.to_lowercase().contains(&search_text) {
                matched = true;
            }
            if search_description && req.description.to_lowercase().contains(&search_text) {
                matched = true;
            }
            if search_spec_id {
                if let Some(ref spec_id) = req.spec_id {
                    if spec_id.to_lowercase().contains(&search_text) {
                        matched = true;
                    }
                }
            }
            if search_comments {
                for comment in &req.comments {
                    if comment.content.to_lowercase().contains(&search_text) {
                        matched = true;
                        break;
                    }
                }
            }
            matched
        })
        .map(requirement_to_proto)
        .collect();

    let total_matches = results.len() as i32;

    if let Some(limit) = query.limit {
        if limit > 0 {
            results = results.into_iter().take(limit as usize).collect();
        }
    }

    Ok(Json(proto::SearchRequirementsResponse {
        requirements: results,
        total_matches,
    }))
}

// ============================================================================
// Helper functions
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

fn parse_status(s: &str) -> aida_core::RequirementStatus {
    match s.to_lowercase().as_str() {
        "draft" => aida_core::RequirementStatus::Draft,
        "approved" => aida_core::RequirementStatus::Approved,
        "planned" => aida_core::RequirementStatus::Planned,
        "inprogress" | "in_progress" | "in-progress" => aida_core::RequirementStatus::InProgress,
        "done" => aida_core::RequirementStatus::Done,
        "completed" => aida_core::RequirementStatus::Completed,
        "rejected" => aida_core::RequirementStatus::Rejected,
        "needsattention" | "needs_attention" | "needs-attention" => {
            aida_core::RequirementStatus::NeedsAttention
        }
        _ => aida_core::RequirementStatus::Draft,
    }
}

fn parse_priority(s: &str) -> aida_core::RequirementPriority {
    match s.to_lowercase().as_str() {
        "high" => aida_core::RequirementPriority::High,
        "medium" => aida_core::RequirementPriority::Medium,
        "low" => aida_core::RequirementPriority::Low,
        _ => aida_core::RequirementPriority::Medium,
    }
}

fn parse_req_type(s: &str) -> aida_core::RequirementType {
    match s.to_lowercase().as_str() {
        "functional" => aida_core::RequirementType::Functional,
        "nonfunctional" | "non_functional" | "non-functional" => {
            aida_core::RequirementType::NonFunctional
        }
        "system" => aida_core::RequirementType::System,
        "user" => aida_core::RequirementType::User,
        "changerequest" | "change_request" | "change-request" => {
            aida_core::RequirementType::ChangeRequest
        }
        "bug" => aida_core::RequirementType::Bug,
        "epic" => aida_core::RequirementType::Epic,
        "story" => aida_core::RequirementType::Story,
        "task" => aida_core::RequirementType::Task,
        "spike" => aida_core::RequirementType::Spike,
        "sprint" => aida_core::RequirementType::Sprint,
        "folder" => aida_core::RequirementType::Folder,
        "meta" => aida_core::RequirementType::Meta,
        _ => aida_core::RequirementType::Functional,
    }
}

// ============================================================================
// V2 legacy handlers (native JSON matching TypeScript types)
// ============================================================================

// trace:FR-0227 | ai:claude
async fn list_requirements_v2_legacy(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<Vec<models::Requirement>>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;
    Ok(Json(store.requirements.clone()))
}

async fn get_requirement_v2_legacy(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Result<Json<models::Requirement>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;
    let req = find_requirement(&store, &id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {id}"),
        )
    })?;
    Ok(Json(req.clone()))
}

#[derive(Deserialize)]
struct UpdateRequirementV2Request {
    status: Option<String>,
    priority: Option<String>,
    title: Option<String>,
    description: Option<String>,
    owner: Option<String>,
    feature: Option<String>,
    tags: Option<Vec<String>>,
    req_type: Option<String>,
    custom_fields: Option<std::collections::HashMap<String, String>>,
    archived: Option<bool>,
}

async fn update_requirement_v2_legacy(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateRequirementV2Request>,
) -> Result<Json<models::Requirement>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let mut store = state.store.write().await;
    let idx = find_requirement_index(&store, &id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {id}"),
        )
    })?;

    if let Some(status) = &body.status {
        store.requirements[idx].status = parse_status(status);
    }
    if let Some(priority) = &body.priority {
        store.requirements[idx].priority = parse_priority(priority);
    }
    if let Some(title) = &body.title {
        store.requirements[idx].title = title.clone();
    }
    if let Some(description) = &body.description {
        store.requirements[idx].description = description.clone();
    }
    if let Some(owner) = &body.owner {
        store.requirements[idx].owner = owner.clone();
    }
    if let Some(feature) = &body.feature {
        store.requirements[idx].feature = feature.clone();
    }
    if let Some(tags) = &body.tags {
        store.requirements[idx].tags = tags.iter().cloned().collect();
    }
    if let Some(req_type) = &body.req_type {
        store.requirements[idx].req_type = parse_req_type(req_type);
    }
    if let Some(custom_fields) = body.custom_fields {
        store.requirements[idx].custom_fields.extend(custom_fields);
    }
    if let Some(archived) = body.archived {
        store.requirements[idx].archived = archived;
    }
    store.requirements[idx].modified_at = chrono::Utc::now();

    let updated = store.requirements[idx].clone();

    // Persist changes
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    drop(store);
    state.mark_saved().await;

    Ok(Json(updated))
}

#[derive(Deserialize)]
struct CreateRequirementV2Request {
    title: String,
    description: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    owner: Option<String>,
    feature: Option<String>,
    req_type: Option<String>,
    tags: Option<Vec<String>>,
    custom_fields: Option<std::collections::HashMap<String, String>>,
}

async fn create_requirement_v2_legacy(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<CreateRequirementV2Request>,
) -> Result<(StatusCode, Json<models::Requirement>), (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let mut store = state.store.write().await;

    let mut new_req = aida_core::Requirement::new(body.title, body.description.unwrap_or_default());
    new_req.status = parse_status(&body.status.unwrap_or_else(|| "Draft".to_string()));
    new_req.priority = parse_priority(&body.priority.unwrap_or_else(|| "Medium".to_string()));
    new_req.req_type = parse_req_type(&body.req_type.unwrap_or_else(|| "Functional".to_string()));
    new_req.owner = body.owner.unwrap_or_default();
    new_req.feature = body.feature.unwrap_or_else(|| "Uncategorized".to_string());
    if let Some(tags) = body.tags {
        new_req.tags = tags.into_iter().collect();
    }
    if let Some(custom_fields) = body.custom_fields {
        new_req.custom_fields = custom_fields;
    }

    let feature_prefix = store
        .features
        .iter()
        .find(|f| f.name == new_req.feature)
        .map(|f| f.prefix.clone());
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

    store.add_requirement_with_id(new_req, feature_prefix.as_deref(), type_prefix.as_deref());

    let added = store
        .requirements
        .last()
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to add requirement",
            )
        })?
        .clone();

    drop(store);
    if let Err(e) = state.backend.save(&*state.store.read().await) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save: {}", e),
        ));
    }
    state.mark_saved().await;

    Ok((StatusCode::CREATED, Json(added)))
}

async fn search_requirements_v2_legacy(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<models::Requirement>>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;
    let search_text = query.q.unwrap_or_default().to_lowercase();

    let search_title = query.search_title.unwrap_or(true);
    let search_description = query.search_description.unwrap_or(true);
    let search_comments = query.search_comments.unwrap_or(false);
    let search_spec_id = query.search_spec_id.unwrap_or(true);

    let mut results: Vec<_> = store
        .requirements
        .iter()
        .filter(|req| {
            if !query.include_archived.unwrap_or(false) && req.archived {
                return false;
            }
            if let Some(ref status) = query.status {
                if format!("{:?}", req.status).to_lowercase() != status.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref req_type) = query.req_type {
                if format!("{:?}", req.req_type).to_lowercase() != req_type.to_lowercase() {
                    return false;
                }
            }
            if let Some(ref feature) = query.feature {
                if req.feature.to_lowercase() != feature.to_lowercase() {
                    return false;
                }
            }
            if search_text.is_empty() {
                return true;
            }
            let mut matched = false;
            if search_title && req.title.to_lowercase().contains(&search_text) {
                matched = true;
            }
            if search_description && req.description.to_lowercase().contains(&search_text) {
                matched = true;
            }
            if search_spec_id {
                if let Some(ref spec_id) = req.spec_id {
                    if spec_id.to_lowercase().contains(&search_text) {
                        matched = true;
                    }
                }
            }
            if search_comments {
                for comment in &req.comments {
                    if comment.content.to_lowercase().contains(&search_text) {
                        matched = true;
                        break;
                    }
                }
            }
            matched
        })
        .cloned()
        .collect();

    if let Some(limit) = query.limit {
        if limit > 0 {
            results = results.into_iter().take(limit as usize).collect();
        }
    }

    Ok(Json(results))
}

// ============================================================================
// Reload handler (legacy)
// ============================================================================

async fn reload_legacy(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    match state.reload().await {
        Ok(count) => Ok(Json(serde_json::json!({
            "reloaded": true,
            "requirements": count
        }))),
        Err(e) => Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Reload failed: {}", e),
        )),
    }
}

// ============================================================================
// Sprint assignment handlers (legacy)
// ============================================================================

#[derive(Deserialize)]
struct SprintAssignRequest {
    sprint_id: String,
    username: Option<String>,
}

async fn assign_sprint_legacy(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(body): Json<SprintAssignRequest>,
) -> Result<Json<models::Requirement>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let mut store = state.store.write().await;

    // Look up the requirement to assign
    let req_id = {
        let req = find_requirement(&store, &id).ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                format!("Requirement not found: {id}"),
            )
        })?;
        req.id
    };

    // Look up the sprint
    let sprint_id = {
        let sprint = find_requirement(&store, &body.sprint_id).ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                format!("Sprint not found: {}", body.sprint_id),
            )
        })?;
        if sprint.req_type != aida_core::RequirementType::Sprint {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("Target {} is not a Sprint", body.sprint_id),
            ));
        }
        sprint.id
    };

    let username = body.username.unwrap_or_else(|| "web-user".to_string());
    store.assign_to_sprint(req_id, sprint_id, &username);

    let updated = find_requirement(&store, &id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Requirement disappeared after assignment",
            )
        })?
        .clone();

    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    drop(store);
    state.mark_saved().await;

    Ok(Json(updated))
}

async fn remove_sprint_legacy(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Result<Json<models::Requirement>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let mut store = state.store.write().await;

    let req_id = {
        let req = find_requirement(&store, &id).ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                format!("Requirement not found: {id}"),
            )
        })?;
        req.id
    };

    store.remove_from_sprint(req_id, "web-user");

    let updated = find_requirement(&store, &id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Requirement disappeared after removal",
            )
        })?
        .clone();

    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    drop(store);
    state.mark_saved().await;

    Ok(Json(updated))
}

// ============================================================================
// Parent assignment handler (legacy)
// ============================================================================

// trace:STORY-0375 | ai:claude
#[derive(Deserialize)]
struct SetParentRequest {
    parent_id: Option<String>,
}

async fn set_parent_legacy(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(body): Json<SetParentRequest>,
) -> Result<Json<models::Requirement>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let mut store = state.store.write().await;

    // Look up the child requirement
    let child_id = {
        let req = find_requirement(&store, &id).ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                format!("Requirement not found: {id}"),
            )
        })?;
        req.id
    };

    match body.parent_id {
        Some(pid) => {
            // Resolve parent to UUID
            let parent_id = {
                let parent = find_requirement(&store, &pid).ok_or_else(|| {
                    ApiError::new(StatusCode::NOT_FOUND, format!("Parent not found: {pid}"))
                })?;
                parent.id
            };

            // set_relationship removes any existing Parent relationship first
            store
                .set_relationship(
                    &child_id,
                    aida_core::RelationshipType::Parent,
                    &parent_id,
                    true,
                )
                .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
        }
        None => {
            // Remove existing Parent relationship if any
            let parent_target: Option<uuid::Uuid> = {
                let req = find_requirement(&store, &id).ok_or_else(|| {
                    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Requirement disappeared")
                })?;
                req.relationships
                    .iter()
                    .find(|r| r.rel_type == aida_core::RelationshipType::Parent)
                    .map(|r| r.target_id)
            };
            if let Some(target) = parent_target {
                let _ = store.remove_relationship(
                    &child_id,
                    &aida_core::RelationshipType::Parent,
                    &target,
                    true,
                );
            }
        }
    }

    let updated = find_requirement(&store, &id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Requirement disappeared after update",
            )
        })?
        .clone();

    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    drop(store);
    state.mark_saved().await;

    Ok(Json(updated))
}

// ============================================================================
// Skills browser handlers
// ============================================================================

#[derive(Serialize)]
struct SkillInfo {
    name: String,
    description: String,
    kind: String,
    content: String,
}

#[derive(Serialize)]
struct SkillDetail {
    name: String,
    description: String,
    kind: String,
    content: String,
    allowed_tools: Vec<String>,
}

#[derive(Deserialize)]
struct UpdateSkillRequest {
    content: String,
}

/// Parse YAML frontmatter from a markdown file.
/// Returns (name, description, allowed_tools, full_content).
fn parse_skill_frontmatter(content: &str) -> (String, String, Vec<String>) {
    if !content.starts_with("---") {
        return (String::new(), String::new(), Vec::new());
    }
    // Find closing ---
    if let Some(end) = content[3..].find("\n---") {
        let yaml_block = &content[3..3 + end].trim();
        let mut name = String::new();
        let mut description = String::new();
        let mut allowed_tools = Vec::new();
        let mut in_tools = false;

        for line in yaml_block.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("name:") {
                name = trimmed
                    .strip_prefix("name:")
                    .unwrap_or("")
                    .trim()
                    .to_string();
                in_tools = false;
            } else if trimmed.starts_with("description:") {
                description = trimmed
                    .strip_prefix("description:")
                    .unwrap_or("")
                    .trim()
                    .to_string();
                in_tools = false;
            } else if trimmed.starts_with("allowed-tools:") {
                in_tools = true;
            } else if in_tools && trimmed.starts_with("- ") {
                allowed_tools.push(trimmed.strip_prefix("- ").unwrap_or("").trim().to_string());
            } else if !trimmed.starts_with('-') {
                in_tools = false;
            }
        }

        (name, description, allowed_tools)
    } else {
        (String::new(), String::new(), Vec::new())
    }
}

/// Get the .claude directory relative to the project root (database parent)
fn claude_dir(state: &ServerState) -> PathBuf {
    project_root(state).join(".claude")
}

/// Get the project root directory, derived from the database path.
/// For directory-based backends (git), the path itself IS the project root.
/// For file-based backends (sqlite/yaml), the parent directory is the project root.
fn project_root(state: &ServerState) -> PathBuf {
    let db_path = std::path::Path::new(state.backend.path());
    // If the backend path is a directory (git store), use it directly
    if db_path.is_dir() {
        return db_path.to_path_buf();
    }
    // For file-based backends, use the parent directory
    if let Some(parent) = db_path.parent() {
        if parent.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_default()
        } else {
            parent.to_path_buf()
        }
    } else {
        std::env::current_dir().unwrap_or_default()
    }
}

/// Scan a directory for .md files and return skill info entries
fn scan_skill_dir(dir: &std::path::Path, kind: &str) -> Vec<(SkillInfo, PathBuf)> {
    let mut results = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return results,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let file_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let (fm_name, description, _) = parse_skill_frontmatter(&content);
        let name = if fm_name.is_empty() {
            file_name
        } else {
            fm_name
        };
        // For commands without frontmatter, extract description from first non-empty, non-heading line
        let desc = if description.is_empty() {
            content
                .lines()
                .skip_while(|l| l.starts_with("---"))
                .skip_while(|l| l.trim().is_empty() || l.starts_with('#'))
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_string()
        } else {
            description
        };
        results.push((
            SkillInfo {
                name,
                description: desc,
                kind: kind.to_string(),
                content,
            },
            path,
        ));
    }
    results.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    results
}

async fn list_skills(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<Vec<SkillInfo>>, (StatusCode, Json<ApiError>)> {
    let base = claude_dir(&state);
    let mut all: Vec<SkillInfo> = Vec::new();

    for (info, _) in scan_skill_dir(&base.join("skills"), "skill") {
        all.push(info);
    }
    for (info, _) in scan_skill_dir(&base.join("commands"), "command") {
        all.push(info);
    }

    Ok(Json(all))
}

async fn get_skill(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
) -> Result<Json<SkillDetail>, (StatusCode, Json<ApiError>)> {
    let base = claude_dir(&state);

    // Search skills then commands
    let dirs = [
        (base.join("skills"), "skill"),
        (base.join("commands"), "command"),
    ];

    for (dir, kind) in &dirs {
        let path = dir.join(format!("{}.md", name));
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let (fm_name, description, allowed_tools) = parse_skill_frontmatter(&content);
            let skill_name = if fm_name.is_empty() {
                name.clone()
            } else {
                fm_name
            };
            let desc = if description.is_empty() {
                content
                    .lines()
                    .skip_while(|l| l.starts_with("---"))
                    .skip_while(|l| l.trim().is_empty() || l.starts_with('#'))
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .to_string()
            } else {
                description
            };
            return Ok(Json(SkillDetail {
                name: skill_name,
                description: desc,
                kind: kind.to_string(),
                content,
                allowed_tools,
            }));
        }
    }

    Err(ApiError::new(
        StatusCode::NOT_FOUND,
        format!("Skill not found: {name}"),
    ))
}

async fn update_skill(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
    Json(body): Json<UpdateSkillRequest>,
) -> Result<Json<SkillDetail>, (StatusCode, Json<ApiError>)> {
    let base = claude_dir(&state);

    let dirs = [
        (base.join("skills"), "skill"),
        (base.join("commands"), "command"),
    ];

    for (dir, kind) in &dirs {
        let path = dir.join(format!("{}.md", name));
        if path.exists() {
            // Resolve symlinks so we write to the master template
            let real_path = std::fs::canonicalize(&path)
                .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

            std::fs::write(&real_path, &body.content)
                .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

            let (fm_name, description, allowed_tools) = parse_skill_frontmatter(&body.content);
            let skill_name = if fm_name.is_empty() {
                name.clone()
            } else {
                fm_name
            };
            let desc = if description.is_empty() {
                body.content
                    .lines()
                    .skip_while(|l| l.starts_with("---"))
                    .skip_while(|l| l.trim().is_empty() || l.starts_with('#'))
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .to_string()
            } else {
                description
            };

            return Ok(Json(SkillDetail {
                name: skill_name,
                description: desc,
                kind: kind.to_string(),
                content: body.content,
                allowed_tools,
            }));
        }
    }

    Err(ApiError::new(
        StatusCode::NOT_FOUND,
        format!("Skill not found: {name}"),
    ))
}

// ============================================================================
// Docs browser handlers
// ============================================================================

#[derive(Serialize)]
struct DocInfo {
    name: String,
    title: String,
    path: String,
    section: String,
}

#[derive(Serialize)]
struct DocDetail {
    name: String,
    title: String,
    path: String,
    section: String,
    content: String,
}

/// Extract the first `# heading` from markdown content
fn extract_md_title(content: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            return heading.trim().to_string();
        }
    }
    String::new()
}

/// Get the docs/ directory relative to cwd
fn docs_dir(state: &ServerState) -> PathBuf {
    project_root(state).join("docs")
}

/// Recursively scan for .md files under a directory
fn scan_docs_recursive(dir: &std::path::Path, base: &std::path::Path) -> Vec<DocInfo> {
    let mut results = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return results,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            results.extend(scan_docs_recursive(&path, base));
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let rel_path = path.strip_prefix(base).unwrap_or(&path);
        let rel_str = rel_path.to_string_lossy().to_string();
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let title = extract_md_title(&content);
        let title = if title.is_empty() {
            name.clone()
        } else {
            title
        };
        let section = if rel_str.starts_with("plans/") || rel_str.starts_with("plans\\") {
            "plans".to_string()
        } else {
            "docs".to_string()
        };
        results.push(DocInfo {
            name,
            title,
            path: rel_str,
            section,
        });
    }
    results.sort_by(|a, b| a.path.cmp(&b.path));
    results
}

async fn list_docs(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<Vec<DocInfo>>, (StatusCode, Json<ApiError>)> {
    let base = docs_dir(&state);
    if !base.exists() {
        return Ok(Json(Vec::new()));
    }
    Ok(Json(scan_docs_recursive(&base, &base)))
}

async fn get_doc(
    State(state): State<Arc<ServerState>>,
    Path(rel_path): Path<String>,
) -> Result<Json<DocDetail>, (StatusCode, Json<ApiError>)> {
    let base = docs_dir(&state);
    let file_path = base.join(&rel_path);

    // Prevent path traversal
    let canonical = file_path
        .canonicalize()
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, format!("Doc not found: {rel_path}")))?;
    let canonical_base = base
        .canonicalize()
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "Docs directory not found"))?;
    if !canonical.starts_with(&canonical_base) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Invalid path"));
    }

    let content = std::fs::read_to_string(&canonical)
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, format!("Doc not found: {rel_path}")))?;

    let name = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let title = extract_md_title(&content);
    let title = if title.is_empty() {
        name.clone()
    } else {
        title
    };
    let section = if rel_path.starts_with("plans/") || rel_path.starts_with("plans\\") {
        "plans".to_string()
    } else {
        "docs".to_string()
    };

    Ok(Json(DocDetail {
        name,
        title,
        path: rel_path,
        section,
        content,
    }))
}

// ============================================================================
// Settings handlers
// ============================================================================

// trace:TASK-0001 | ai:claude
#[derive(Deserialize)]
struct UpdateMetadataRequest {
    name: Option<String>,
    title: Option<String>,
    description: Option<String>,
}

#[derive(Serialize)]
struct MetadataResponse {
    name: String,
    title: String,
    description: String,
}

async fn get_settings_metadata(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<MetadataResponse>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;
    Ok(Json(MetadataResponse {
        name: store.name.clone(),
        title: store.title.clone(),
        description: store.description.clone(),
    }))
}

async fn update_settings_metadata(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<UpdateMetadataRequest>,
) -> Result<Json<MetadataResponse>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;
    if let Some(name) = body.name {
        store.name = name;
    }
    if let Some(title) = body.title {
        store.title = title;
    }
    if let Some(description) = body.description {
        store.description = description;
    }
    let resp = MetadataResponse {
        name: store.name.clone(),
        title: store.title.clone(),
        description: store.description.clone(),
    };
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok(Json(resp))
}

// --- Relationship definitions ---

async fn list_relationship_defs(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<Vec<models::RelationshipDefinition>>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;
    Ok(Json(store.relationship_definitions.clone()))
}

async fn create_relationship_def(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<models::RelationshipDefinition>,
) -> Result<(StatusCode, Json<models::RelationshipDefinition>), (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;
    store
        .add_relationship_definition(body.clone())
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    let created = store
        .relationship_definitions
        .last()
        .cloned()
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to retrieve created definition",
            )
        })?;
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok((StatusCode::CREATED, Json(created)))
}

async fn update_relationship_def(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
    Json(body): Json<models::RelationshipDefinition>,
) -> Result<Json<models::RelationshipDefinition>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;
    store
        .update_relationship_definition(&name, body)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    let updated = store
        .relationship_definitions
        .iter()
        .find(|d| d.name == name.to_lowercase())
        .cloned()
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Definition not found after update",
            )
        })?;
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok(Json(updated))
}

async fn delete_relationship_def(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;
    store
        .remove_relationship_definition(&name)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok(Json(serde_json::json!({"message": "Deleted"})))
}

// --- Type definitions ---

async fn list_type_defs(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<Vec<models::CustomTypeDefinition>>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;
    Ok(Json(store.type_definitions.clone()))
}

async fn create_type_def(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<models::CustomTypeDefinition>,
) -> Result<(StatusCode, Json<models::CustomTypeDefinition>), (StatusCode, Json<ApiError>)> {
    if body.name.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Type name cannot be empty",
        ));
    }
    let mut store = state.store.write().await;
    if store
        .type_definitions
        .iter()
        .any(|d| d.name.eq_ignore_ascii_case(&body.name))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("Type definition '{}' already exists", body.name),
        ));
    }
    let def = models::CustomTypeDefinition {
        built_in: false,
        ..body
    };
    store.type_definitions.push(def.clone());
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok((StatusCode::CREATED, Json(def)))
}

async fn update_type_def(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
    Json(body): Json<models::CustomTypeDefinition>,
) -> Result<Json<models::CustomTypeDefinition>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;
    let def = store
        .type_definitions
        .iter_mut()
        .find(|d| d.name.eq_ignore_ascii_case(&name))
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                format!("Type definition '{}' not found", name),
            )
        })?;

    if def.built_in {
        // For built-in types, only update non-critical fields
        def.display_name = body.display_name;
        def.description = body.description;
        def.color = body.color;
        def.statuses = body.statuses;
        def.priorities = body.priorities;
        def.custom_fields = body.custom_fields;
    } else {
        let was_name = def.name.clone();
        *def = models::CustomTypeDefinition {
            name: was_name,
            built_in: false,
            ..body
        };
    }
    let updated = def.clone();
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok(Json(updated))
}

async fn delete_type_def(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;
    let def = store
        .type_definitions
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(&name))
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                format!("Type definition '{}' not found", name),
            )
        })?;
    if def.built_in {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("Cannot delete built-in type '{}'", name),
        ));
    }
    store
        .type_definitions
        .retain(|d| !d.name.eq_ignore_ascii_case(&name));
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok(Json(serde_json::json!({"message": "Deleted"})))
}

// --- Reaction definitions ---

async fn list_reaction_defs(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<Vec<models::ReactionDefinition>>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;
    Ok(Json(store.reaction_definitions.clone()))
}

async fn create_reaction_def(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<models::ReactionDefinition>,
) -> Result<(StatusCode, Json<models::ReactionDefinition>), (StatusCode, Json<ApiError>)> {
    if body.name.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Reaction name cannot be empty",
        ));
    }
    let mut store = state.store.write().await;
    if store
        .reaction_definitions
        .iter()
        .any(|d| d.name.eq_ignore_ascii_case(&body.name))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("Reaction '{}' already exists", body.name),
        ));
    }
    let def = models::ReactionDefinition {
        built_in: false,
        ..body
    };
    store.reaction_definitions.push(def.clone());
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok((StatusCode::CREATED, Json(def)))
}

async fn update_reaction_def(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
    Json(body): Json<models::ReactionDefinition>,
) -> Result<Json<models::ReactionDefinition>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;
    let def = store
        .reaction_definitions
        .iter_mut()
        .find(|d| d.name.eq_ignore_ascii_case(&name))
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                format!("Reaction '{}' not found", name),
            )
        })?;

    if def.built_in {
        // For built-in reactions, only update non-critical fields
        def.emoji = body.emoji;
        def.label = body.label;
        def.description = body.description;
    } else {
        let was_name = def.name.clone();
        *def = models::ReactionDefinition {
            name: was_name,
            built_in: false,
            ..body
        };
    }
    let updated = def.clone();
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok(Json(updated))
}

async fn delete_reaction_def(
    State(state): State<Arc<ServerState>>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;
    let def = store
        .reaction_definitions
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(&name))
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                format!("Reaction '{}' not found", name),
            )
        })?;
    if def.built_in {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("Cannot delete built-in reaction '{}'", name),
        ));
    }
    store
        .reaction_definitions
        .retain(|d| !d.name.eq_ignore_ascii_case(&name));
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok(Json(serde_json::json!({"message": "Deleted"})))
}

// --- ID config ---

async fn get_id_config(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<models::IdConfiguration>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;
    Ok(Json(store.id_config.clone()))
}

async fn update_id_config(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<models::IdConfiguration>,
) -> Result<Json<models::IdConfiguration>, (StatusCode, Json<ApiError>)> {
    if body.digits < 1 || body.digits > 6 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Digits must be between 1 and 6",
        ));
    }
    let mut store = state.store.write().await;
    store.id_config = body;
    let updated = store.id_config.clone();
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok(Json(updated))
}

// --- Prefixes ---

#[derive(Serialize, Deserialize)]
struct PrefixConfigResponse {
    allowed_prefixes: Vec<String>,
    restrict_prefixes: bool,
}

async fn get_prefixes(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<PrefixConfigResponse>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;
    Ok(Json(PrefixConfigResponse {
        allowed_prefixes: store.allowed_prefixes.clone(),
        restrict_prefixes: store.restrict_prefixes,
    }))
}

async fn update_prefixes(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<PrefixConfigResponse>,
) -> Result<Json<PrefixConfigResponse>, (StatusCode, Json<ApiError>)> {
    // Validate prefixes are non-empty uppercase strings
    for prefix in &body.allowed_prefixes {
        if prefix.trim().is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "Prefix cannot be empty",
            ));
        }
        if prefix != &prefix.to_uppercase() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("Prefix '{}' must be uppercase", prefix),
            ));
        }
    }
    // Check for duplicates
    let mut seen = std::collections::HashSet::new();
    for prefix in &body.allowed_prefixes {
        if !seen.insert(prefix) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("Duplicate prefix '{}'", prefix),
            ));
        }
    }
    let mut store = state.store.write().await;
    store.allowed_prefixes = body.allowed_prefixes;
    store.restrict_prefixes = body.restrict_prefixes;
    let resp = PrefixConfigResponse {
        allowed_prefixes: store.allowed_prefixes.clone(),
        restrict_prefixes: store.restrict_prefixes,
    };
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }
    Ok(Json(resp))
}

// ============================================================================
// Queue endpoints (STORY-0367)
// ============================================================================
// trace:STORY-0367 | ai:claude

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueAddRequest {
    requirement_id: String,
    position: Option<String>,
    note: Option<String>,
    added_by: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueUpdateRequest {
    position: Option<i64>,
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueReorderRequest {
    items: Vec<QueueReorderItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueReorderItem {
    requirement_id: String,
    position: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueListQuery {
    include_completed: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QueueEntryResponse {
    requirement_id: String,
    spec_id: Option<String>,
    title: String,
    status: String,
    priority: String,
    req_type: String,
    position: i64,
    added_by: String,
    note: Option<String>,
    added_at: String,
}

#[derive(Serialize)]
struct QueueListResponse {
    entries: Vec<QueueEntryResponse>,
    total: usize,
}

fn enrich_queue_entry(
    entry: &QueueEntry,
    store: &aida_core::RequirementsStore,
) -> QueueEntryResponse {
    let req = store
        .requirements
        .iter()
        .find(|r| r.id == entry.requirement_id);
    QueueEntryResponse {
        requirement_id: entry.requirement_id.to_string(),
        spec_id: req.and_then(|r| r.spec_id.clone()),
        title: req
            .map(|r| r.title.clone())
            .unwrap_or_else(|| "(deleted)".to_string()),
        status: req
            .map(|r| format!("{:?}", r.status))
            .unwrap_or_else(|| "Unknown".to_string()),
        priority: req
            .map(|r| format!("{:?}", r.priority))
            .unwrap_or_else(|| "Medium".to_string()),
        req_type: req
            .map(|r| format!("{:?}", r.req_type))
            .unwrap_or_else(|| "Task".to_string()),
        position: entry.position,
        added_by: entry.added_by.clone(),
        note: entry.note.clone(),
        added_at: entry.added_at.to_rfc3339(),
    }
}

async fn queue_list(
    State(state): State<Arc<ServerState>>,
    Path(user_id): Path<String>,
    Query(query): Query<QueueListQuery>,
) -> Result<Json<QueueListResponse>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let include_completed = query.include_completed.unwrap_or(false);

    let entries = state
        .backend
        .queue_list(&user_id, include_completed)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let store = state.store.read().await;
    let response_entries: Vec<QueueEntryResponse> = entries
        .iter()
        .map(|e| enrich_queue_entry(e, &store))
        .collect();
    let total = response_entries.len();

    Ok(Json(QueueListResponse {
        entries: response_entries,
        total,
    }))
}

async fn queue_add(
    State(state): State<Arc<ServerState>>,
    Path(user_id): Path<String>,
    Json(body): Json<QueueAddRequest>,
) -> Result<(StatusCode, Json<QueueEntryResponse>), (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;

    // Resolve requirement_id (UUID or spec_id)
    let req = find_requirement(&store, &body.requirement_id).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {}", body.requirement_id),
        )
    })?;
    let requirement_id = req.id;

    // Compute position (i64::MAX = sentinel for "auto-append to bottom")
    let position = match body.position.as_deref() {
        Some("top") => {
            let existing = state
                .backend
                .queue_list(&user_id, true)
                .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            existing.first().map(|e| e.position - 1000).unwrap_or(1000)
        }
        Some(n) if n != "bottom" => n.parse::<i64>().unwrap_or(i64::MAX),
        _ => i64::MAX, // bottom or unspecified: queue_add auto-assigns max+1000
    };

    let added_by = body.added_by.unwrap_or_else(|| user_id.clone());

    let entry = QueueEntry {
        user_id: user_id.clone(),
        requirement_id,
        position,
        added_by,
        note: body.note.clone(),
        added_at: chrono::Utc::now(),
        // REST API doesn't yet expose role/scope/session routing;
        // pass-through fields when caller supplies them via JSON body
        // in a future iteration.
        for_role: None,
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };

    state
        .backend
        .queue_add(entry)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Read back the actual stored entry (position may have been auto-assigned)
    let entries = state
        .backend
        .queue_list(&user_id, true)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let saved = entries
        .iter()
        .find(|e| e.requirement_id == requirement_id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Entry not found after save",
            )
        })?;

    let response = enrich_queue_entry(saved, &store);
    Ok((StatusCode::CREATED, Json(response)))
}

async fn queue_remove(
    State(state): State<Arc<ServerState>>,
    Path((user_id, req_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;

    let requirement_id = if let Ok(uuid) = uuid::Uuid::parse_str(&req_id) {
        uuid
    } else if let Some(req) = find_requirement(&store, &req_id) {
        req.id
    } else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {}", req_id),
        ));
    };

    drop(store);

    state
        .backend
        .queue_remove(&user_id, &requirement_id)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

async fn queue_update(
    State(state): State<Arc<ServerState>>,
    Path((user_id, req_id)): Path<(String, String)>,
    Json(body): Json<QueueUpdateRequest>,
) -> Result<Json<QueueEntryResponse>, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;

    let requirement_id = if let Ok(uuid) = uuid::Uuid::parse_str(&req_id) {
        uuid
    } else if let Some(req) = find_requirement(&store, &req_id) {
        req.id
    } else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("Requirement not found: {}", req_id),
        ));
    };

    // Get current entry
    let entries = state
        .backend
        .queue_list(&user_id, true)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let current = entries
        .iter()
        .find(|e| e.requirement_id == requirement_id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "Entry not in queue"))?;

    let mut updated = current.clone();
    if let Some(position) = body.position {
        updated.position = position;
    }
    if let Some(note) = body.note {
        updated.note = if note.is_empty() { None } else { Some(note) };
    }

    state
        .backend
        .queue_add(updated.clone())
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response = enrich_queue_entry(&updated, &store);
    Ok(Json(response))
}

async fn queue_reorder(
    State(state): State<Arc<ServerState>>,
    Path(user_id): Path<String>,
    Json(body): Json<QueueReorderRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    state.check_reload().await;
    let store = state.store.read().await;

    let items: Vec<(uuid::Uuid, i64)> = body
        .items
        .iter()
        .filter_map(|item| {
            let uuid = if let Ok(uuid) = uuid::Uuid::parse_str(&item.requirement_id) {
                uuid
            } else if let Some(req) = find_requirement(&store, &item.requirement_id) {
                req.id
            } else {
                return None;
            };
            Some((uuid, item.position))
        })
        .collect();

    drop(store);

    state
        .backend
        .queue_reorder(&user_id, &items)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ============================================================================
// Analytics
// ============================================================================

async fn get_analytics(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<aida_core::analytics::AnalyticsReport>, (StatusCode, Json<ApiError>)> {
    let store = state
        .backend
        .load()
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let report = aida_core::analytics::compute_analytics(&store.requirements);
    Ok(Json(report))
}

// ============================================================================
// Jira Sync
// ============================================================================

#[derive(Serialize)]
struct JiraSyncItem {
    aida_id: String,
    aida_title: String,
    aida_status: String,
    jira_key: String,
    jira_status: Option<String>,
    jira_summary: Option<String>,
    sync_status: String, // "in_sync", "drifted", "error"
    diffs: Vec<JiraSyncDiff>,
}

#[derive(Serialize)]
struct JiraSyncDiff {
    field: String,
    aida_value: String,
    jira_value: String,
}

#[derive(Serialize)]
struct JiraSyncResponse {
    items: Vec<JiraSyncItem>,
    total: usize,
    in_sync: usize,
    drifted: usize,
    errors: usize,
}

async fn get_jira_sync(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<JiraSyncResponse>, (StatusCode, Json<ApiError>)> {
    let store = state
        .backend
        .load()
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Find linked requirements (title starts with [KEY-N])
    let linked: Vec<(&aida_core::Requirement, String)> = store
        .requirements
        .iter()
        .filter_map(|r| {
            if r.title.starts_with('[') {
                if let Some(end) = r.title.find(']') {
                    let key = &r.title[1..end];
                    if key.contains('-')
                        && key
                            .split('-')
                            .next_back()
                            .map(|n| n.parse::<u64>().is_ok())
                            .unwrap_or(false)
                    {
                        return Some((r, key.to_string()));
                    }
                }
            }
            None
        })
        .collect();

    // Try to load Jira config and check issues
    let config = aida_core::JiraConfig::load()
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut items = Vec::new();
    let mut in_sync = 0;
    let mut drifted = 0;
    let mut errors = 0;

    if config.validate().is_ok() {
        let client = aida_core::JiraClient::new(config.clone())
            .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        for (req, jira_key) in &linked {
            match client.get_issue(jira_key).await {
                Ok(issue) => {
                    let mut diffs = Vec::new();
                    let aida_title = req
                        .title
                        .strip_prefix(&format!("[{}] ", jira_key))
                        .unwrap_or(&req.title);

                    if aida_title != issue.summary() {
                        diffs.push(JiraSyncDiff {
                            field: "title".into(),
                            aida_value: aida_title.to_string(),
                            jira_value: issue.summary().to_string(),
                        });
                    }

                    let expected_status = config.map_status(&req.effective_status());
                    if expected_status != issue.status_name() {
                        diffs.push(JiraSyncDiff {
                            field: "status".into(),
                            aida_value: format!(
                                "{} (→{})",
                                req.effective_status(),
                                expected_status
                            ),
                            jira_value: issue.status_name().to_string(),
                        });
                    }

                    let status = if diffs.is_empty() {
                        in_sync += 1;
                        "in_sync"
                    } else {
                        drifted += 1;
                        "drifted"
                    };

                    items.push(JiraSyncItem {
                        aida_id: req.display_id(),
                        aida_title: aida_title.to_string(),
                        aida_status: req.effective_status(),
                        jira_key: jira_key.clone(),
                        jira_status: Some(issue.status_name().to_string()),
                        jira_summary: Some(issue.summary().to_string()),
                        sync_status: status.into(),
                        diffs,
                    });
                }
                Err(e) => {
                    errors += 1;
                    items.push(JiraSyncItem {
                        aida_id: req.display_id(),
                        aida_title: req.title.clone(),
                        aida_status: req.effective_status(),
                        jira_key: jira_key.clone(),
                        jira_status: None,
                        jira_summary: None,
                        sync_status: "error".into(),
                        diffs: vec![JiraSyncDiff {
                            field: "connection".into(),
                            aida_value: "".into(),
                            jira_value: e.to_string(),
                        }],
                    });
                }
            }
        }
    } else {
        // No Jira config — return linked items without checking
        for (req, jira_key) in &linked {
            items.push(JiraSyncItem {
                aida_id: req.display_id(),
                aida_title: req.title.clone(),
                aida_status: req.effective_status(),
                jira_key: jira_key.clone(),
                jira_status: None,
                jira_summary: None,
                sync_status: "unchecked".into(),
                diffs: Vec::new(),
            });
        }
    }

    Ok(Json(JiraSyncResponse {
        total: items.len(),
        in_sync,
        drifted,
        errors,
        items,
    }))
}
