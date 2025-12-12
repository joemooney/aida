// trace:FR-0227 | ai:claude:high
//! REST API implementation for AIDA requirements management
//!
//! Provides JSON-based REST endpoints that mirror the gRPC service.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::convert::*;
use crate::proto;
use crate::service::ServerState;

/// Create the REST API router
pub fn create_rest_router(state: Arc<ServerState>) -> Router {
    Router::new()
        // Server status
        .route("/api/status", get(get_status))
        .route("/api/ping", get(ping))
        // Store operations
        .route("/api/store", get(get_store))
        .route("/api/store/metadata", get(get_store_metadata))
        // Requirements CRUD
        .route("/api/requirements", get(list_requirements))
        .route("/api/requirements", post(create_requirement))
        .route("/api/requirements/:id", get(get_requirement))
        .route("/api/requirements/:id", put(update_requirement))
        .route("/api/requirements/:id", delete(delete_requirement))
        // Comments
        .route("/api/requirements/:id/comments", post(add_comment))
        // Search
        .route("/api/search", get(search_requirements))
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
// Handlers
// ============================================================================

async fn ping(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    Json(PingResponse {
        status: "ok".to_string(),
        version: state.version.clone(),
        uptime_seconds: state.start_time.elapsed().as_secs(),
    })
}

async fn get_status(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<proto::GetServerStatusResponse>, (StatusCode, Json<ApiError>)> {
    let store = state.store.read().await;
    let backend = if state.storage.path().extension().map(|e| e == "db").unwrap_or(false) {
        "sqlite"
    } else {
        "yaml"
    };

    Ok(Json(proto::GetServerStatusResponse {
        version: state.version.clone(),
        status: "running".to_string(),
        uptime_seconds: state.start_time.elapsed().as_secs() as i64,
        active_connections: 0, // Not tracked in REST
        storage_backend: backend.to_string(),
        storage_path: state.storage.path().display().to_string(),
    }))
}

async fn get_store(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<proto::GetStoreResponse>, (StatusCode, Json<ApiError>)> {
    let store = state.store.read().await;
    Ok(Json(proto::GetStoreResponse {
        store: Some(store_to_proto(&store)),
    }))
}

async fn get_store_metadata(
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

async fn list_requirements(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<proto::ListRequirementsResponse>, (StatusCode, Json<ApiError>)> {
    let store = state.store.read().await;

    let mut requirements: Vec<_> = store
        .requirements
        .iter()
        .filter(|req| {
            // Filter by archived status
            if !query.include_archived.unwrap_or(false) && req.archived {
                return false;
            }
            // Filter by status
            if let Some(ref status) = query.status {
                if format!("{:?}", req.status).to_lowercase() != status.to_lowercase() {
                    return false;
                }
            }
            // Filter by priority
            if let Some(ref priority) = query.priority {
                if format!("{:?}", req.priority).to_lowercase() != priority.to_lowercase() {
                    return false;
                }
            }
            // Filter by type
            if let Some(ref req_type) = query.req_type {
                if format!("{:?}", req.req_type).to_lowercase() != req_type.to_lowercase() {
                    return false;
                }
            }
            // Filter by feature
            if let Some(ref feature) = query.feature {
                if req.feature.to_lowercase() != feature.to_lowercase() {
                    return false;
                }
            }
            // Filter by owner
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

    // Apply pagination
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

async fn create_requirement(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<CreateRequirementRequest>,
) -> Result<(StatusCode, Json<proto::CreateRequirementResponse>), (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;

    // Build the new requirement
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

    // Add to store with SPEC-ID assignment
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

    // Get the added requirement with its SPEC-ID
    let added_req = store.requirements.last()
        .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Failed to add requirement"))?;
    let spec_id = added_req.spec_id.clone().unwrap_or_default();
    let proto_req = requirement_to_proto(added_req);

    // Save
    drop(store);
    if let Err(e) = state.storage.save(&*state.store.read().await) {
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
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateRequirementRequest>,
) -> Result<Json<proto::UpdateRequirementResponse>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;

    let idx = find_requirement_index(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {}", id)))?;

    let req = &mut store.requirements[idx];

    // Apply updates
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

    // Save
    drop(store);
    if let Err(e) = state.storage.save(&*state.store.read().await) {
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
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Result<Json<proto::DeleteRequirementResponse>, (StatusCode, Json<ApiError>)> {
    let mut store = state.store.write().await;

    let idx = find_requirement_index(&store, &id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("Requirement not found: {}", id)))?;

    store.requirements.remove(idx);

    // Save
    drop(store);
    if let Err(e) = state.storage.save(&*state.store.read().await) {
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

    // Save
    drop(store);
    if let Err(e) = state.storage.save(&*state.store.read().await) {
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
            // Filter by archived
            if !query.include_archived.unwrap_or(false) && req.archived {
                return false;
            }
            // Filter by status
            if let Some(ref status) = query.status {
                if format!("{:?}", req.status).to_lowercase() != status.to_lowercase() {
                    return false;
                }
            }
            // Filter by type
            if let Some(ref req_type) = query.req_type {
                if format!("{:?}", req.req_type).to_lowercase() != req_type.to_lowercase() {
                    return false;
                }
            }
            // Filter by feature
            if let Some(ref feature) = query.feature {
                if req.feature.to_lowercase() != feature.to_lowercase() {
                    return false;
                }
            }

            // Text search
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

    // Apply limit
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
    // Try UUID first
    if let Ok(uuid) = uuid::Uuid::parse_str(id) {
        return store.requirements.iter().find(|r| r.id == uuid);
    }
    // Try SPEC-ID
    store
        .requirements
        .iter()
        .find(|r| r.spec_id.as_ref().map(|s| s == id).unwrap_or(false))
}

fn find_requirement_index(store: &aida_core::RequirementsStore, id: &str) -> Option<usize> {
    // Try UUID first
    if let Ok(uuid) = uuid::Uuid::parse_str(id) {
        return store.requirements.iter().position(|r| r.id == uuid);
    }
    // Try SPEC-ID
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
        _ => aida_core::RequirementType::Functional,
    }
}
