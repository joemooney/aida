// trace:FR-0227 | ai:claude:high
//! REST API implementation for AIDA requirements management
//!
//! Provides JSON-based REST endpoints that mirror the gRPC service.
//! Supports multi-project via X-Project header.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::convert::*;
use crate::projects::ProjectManager;
use crate::proto;
use crate::service::ServerState;
use aida_core::models;

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
        .route("/api/v2/requirements", get(list_requirements_v2_legacy))
        .route("/api/v2/requirements/:id", get(get_requirement_v2_legacy))
        .route("/api/v2/requirements/:id", put(update_requirement_v2_legacy))
        .route("/api/v2/search", get(search_requirements_v2_legacy))
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
    #[serde(rename = "type")]
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
    #[serde(rename = "type")]
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
    #[serde(rename = "type")]
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
    #[serde(rename = "type")]
    req_type: Option<String>,
    tags: Option<Vec<String>>,
    replace_tags: Option<bool>,
    archived: Option<bool>,
    custom_status: Option<String>,
    custom_priority: Option<String>,
    custom_fields: Option<std::collections::HashMap<String, String>>,
    replace_custom_fields: Option<bool>,
    modified_by: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddCommentRequest {
    content: String,
    author: Option<String>,
    parent_comment_id: Option<String>,
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

    app_state
        .project_manager
        .get_backend(project)
        .await
        .map_err(|e| ApiError::new(StatusCode::NOT_FOUND, format!("Project error: {}", e)))
}

// ============================================================================
// V2 API Handlers (direct aida_core models)
// ============================================================================

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
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Project not found: {}", name)))?;

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

async fn ping(State(state): State<Arc<AppState>>) -> impl IntoResponse {
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
        storage_path: state.project_manager.data_dir().to_string_lossy().to_string(),
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

    let req = find_requirement(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {}", id)))?;

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

    let mut new_req = aida_core::Requirement::new(
        body.title,
        body.description.unwrap_or_default(),
    );
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

    let feature_prefix = store.features.iter()
        .find(|f| f.name == new_req.feature)
        .map(|f| f.prefix.clone());
    let type_prefix = store.type_definitions.iter()
        .find(|td| td.name == new_req.req_type.to_string().replace(" ", "").replace("-", ""))
        .and_then(|td| td.prefix.clone());

    store.add_requirement_with_id(
        new_req.clone(),
        feature_prefix.as_deref(),
        type_prefix.as_deref(),
    );

    let added_req = store.requirements.last()
        .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Failed to add requirement"))?;
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

    let idx = find_requirement_index(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {}", id)))?;

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

    let idx = find_requirement_index(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {}", id)))?;

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

    let idx = find_requirement_index(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {}", id)))?;

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

    let req = find_requirement(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {}", id)))?;

    Ok(Json(proto::GetRequirementResponse {
        requirement: Some(requirement_to_proto(req)),
    }))
}

async fn create_requirement_legacy(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<CreateRequirementRequest>,
) -> Result<(StatusCode, Json<proto::CreateRequirementResponse>), (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;

    let mut new_req = aida_core::Requirement::new(
        body.title,
        body.description.unwrap_or_default(),
    );
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

    let feature_prefix = store.features.iter()
        .find(|f| f.name == new_req.feature)
        .map(|f| f.prefix.clone());
    let type_prefix = store.type_definitions.iter()
        .find(|td| td.name == new_req.req_type.to_string().replace(" ", "").replace("-", ""))
        .and_then(|td| td.prefix.clone());

    store.add_requirement_with_id(
        new_req.clone(),
        feature_prefix.as_deref(),
        type_prefix.as_deref(),
    );

    let added_req = store.requirements.last()
        .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Failed to add requirement"))?;
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

    let idx = find_requirement_index(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {}", id)))?;

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

    let idx = find_requirement_index(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {}", id)))?;

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

    let idx = find_requirement_index(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {}", id)))?;

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
        "completed" => aida_core::RequirementStatus::Completed,
        "rejected" => aida_core::RequirementStatus::Rejected,
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
    let store = state.store.read().await;
    Ok(Json(store.requirements.clone()))
}

async fn get_requirement_v2_legacy(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Result<Json<models::Requirement>, (StatusCode, Json<ApiError>)> {
    let store = state.store.read().await;
    let req = find_requirement(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {id}")))?;
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
}

async fn update_requirement_v2_legacy(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateRequirementV2Request>,
) -> Result<Json<models::Requirement>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;
    let idx = find_requirement_index(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {id}")))?;

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
    store.requirements[idx].modified_at = chrono::Utc::now();

    let updated = store.requirements[idx].clone();

    // Persist changes
    if let Err(e) = state.backend.save(&store) {
        return Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }

    Ok(Json(updated))
}

async fn search_requirements_v2_legacy(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<models::Requirement>>, (StatusCode, Json<ApiError>)> {
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
