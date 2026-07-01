// trace:FR-0227 | ai:claude:high
//! gRPC service implementation for AIDA requirements management

use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::sync::RwLock;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use aida_core::db::DatabaseBackend;
use aida_core::{Requirement, RequirementsStore};

use crate::convert::*;
use crate::projects::ProjectManager;
use crate::proto;
use crate::proto::requirements_service_server::RequirementsService;

/// Server state shared across all connections
pub struct ServerState {
    pub backend: Box<dyn DatabaseBackend>,
    pub store: RwLock<RequirementsStore>,
    pub start_time: Instant,
    pub version: String,
    pub shutdown_requested: RwLock<bool>,
    last_loaded_mtime: RwLock<SystemTime>,
}

impl ServerState {
    pub fn new(backend: Box<dyn DatabaseBackend>) -> anyhow::Result<Self> {
        let store = backend.load()?;
        let mtime = Self::compute_db_mtime(backend.path());
        Ok(Self {
            backend,
            store: RwLock::new(store),
            start_time: Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            shutdown_requested: RwLock::new(false),
            last_loaded_mtime: RwLock::new(mtime),
        })
    }

    /// Save the current store to disk and update mtime to prevent unnecessary reloads.
    /// Uses block_in_place for PostgreSQL compatibility (sync postgres crate can't run
    /// inside a Tokio async context without this).
    async fn save(&self) -> Result<(), Status> {
        let store = self.store.read().await;
        let backend = &self.backend;
        tokio::task::block_in_place(|| {
            backend
                .save(&store)
                .map_err(|e| Status::internal(format!("Failed to save: {}", e)))
        })?;
        self.mark_saved().await;
        Ok(())
    }

    /// Update last_loaded_mtime after a direct backend.save() to prevent spurious reloads
    pub async fn mark_saved(&self) {
        *self.last_loaded_mtime.write().await = Self::compute_db_mtime(self.backend.path());
    }

    /// Get the latest mtime across the DB file and its WAL file (for SQLite WAL mode)
    fn compute_db_mtime(path: &std::path::Path) -> SystemTime {
        let main_mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        // SQLite WAL mode writes to a separate -wal file
        let wal_path = path.with_extension("db-wal");
        let wal_mtime = std::fs::metadata(&wal_path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        main_mtime.max(wal_mtime)
    }

    /// Reload the store from the database backend
    pub async fn reload(&self) -> anyhow::Result<usize> {
        let backend = &self.backend;
        let new_store = tokio::task::block_in_place(|| backend.load())?;
        let count = new_store.requirements.len();
        let mtime = Self::compute_db_mtime(self.backend.path());
        *self.store.write().await = new_store;
        *self.last_loaded_mtime.write().await = mtime;
        tracing::info!("Reloaded store from disk ({} requirements)", count);
        Ok(count)
    }

    /// Check if the DB file has been modified since last load and reload if so
    pub async fn check_reload(&self) {
        let current_mtime = Self::compute_db_mtime(self.backend.path());
        if current_mtime == SystemTime::UNIX_EPOCH {
            return;
        }
        let last = *self.last_loaded_mtime.read().await;
        if current_mtime > last {
            if let Err(e) = self.reload().await {
                tracing::warn!("Auto-reload failed: {}", e);
            }
        }
    }

    /// Find a requirement by UUID or SPEC-ID
    fn find_requirement<'a>(
        store: &'a RequirementsStore,
        id: &str,
    ) -> Option<(usize, &'a Requirement)> {
        // Try UUID first
        if let Ok(uuid) = Uuid::parse_str(id) {
            return store
                .requirements
                .iter()
                .enumerate()
                .find(|(_, r)| r.id == uuid);
        }
        // Try SPEC-ID
        store
            .requirements
            .iter()
            .enumerate()
            .find(|(_, r)| r.spec_id.as_ref().map(|s| s == id).unwrap_or(false))
    }
}

/// The gRPC service implementation
pub struct AidaService {
    state: Arc<ServerState>,
}

impl AidaService {
    pub fn new(state: Arc<ServerState>) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl RequirementsService for AidaService {
    // ========================================================================
    // Store operations
    // ========================================================================

    async fn get_store(
        &self,
        _request: Request<proto::GetStoreRequest>,
    ) -> Result<Response<proto::GetStoreResponse>, Status> {
        let store = self.state.store.read().await;
        Ok(Response::new(proto::GetStoreResponse {
            store: Some(store_to_proto(&store)),
        }))
    }

    async fn get_store_metadata(
        &self,
        _request: Request<proto::GetStoreMetadataRequest>,
    ) -> Result<Response<proto::GetStoreMetadataResponse>, Status> {
        let store = self.state.store.read().await;
        Ok(Response::new(proto::GetStoreMetadataResponse {
            name: store.name.clone(),
            title: store.title.clone(),
            description: store.description.clone(),
            requirement_count: store.requirements.len() as i32,
            user_count: store.users.len() as i32,
            feature_count: store.features.len() as i32,
        }))
    }

    // ========================================================================
    // Requirement CRUD
    // ========================================================================

    async fn list_requirements(
        &self,
        request: Request<proto::ListRequirementsRequest>,
    ) -> Result<Response<proto::ListRequirementsResponse>, Status> {
        let req = request.into_inner();
        let store = self.state.store.read().await;

        let mut requirements: Vec<&Requirement> = store.requirements.iter().collect();

        // Apply filters
        if !req.status_filter.is_empty() {
            requirements.retain(|r| r.effective_status() == req.status_filter);
        }
        if !req.priority_filter.is_empty() {
            requirements.retain(|r| r.priority.to_string() == req.priority_filter);
        }
        if !req.type_filter.is_empty() {
            requirements.retain(|r| r.req_type.to_string() == req.type_filter);
        }
        if !req.feature_filter.is_empty() {
            requirements.retain(|r| r.feature == req.feature_filter);
        }
        if !req.owner_filter.is_empty() {
            requirements.retain(|r| r.owner == req.owner_filter);
        }
        if !req.include_archived {
            requirements.retain(|r| !r.archived);
        }

        let total_count = requirements.len() as i32;

        // Apply pagination
        if req.offset > 0 {
            requirements = requirements.into_iter().skip(req.offset as usize).collect();
        }
        if req.limit > 0 {
            requirements = requirements.into_iter().take(req.limit as usize).collect();
        }

        Ok(Response::new(proto::ListRequirementsResponse {
            requirements: requirements
                .iter()
                .map(|r| requirement_to_proto(r))
                .collect(),
            total_count,
        }))
    }

    async fn get_requirement(
        &self,
        request: Request<proto::GetRequirementRequest>,
    ) -> Result<Response<proto::GetRequirementResponse>, Status> {
        let req = request.into_inner();
        let store = self.state.store.read().await;

        let (_, requirement) = ServerState::find_requirement(&store, &req.id)
            .ok_or_else(|| Status::not_found(format!("Requirement not found: {}", req.id)))?;

        Ok(Response::new(proto::GetRequirementResponse {
            requirement: Some(requirement_to_proto(requirement)),
        }))
    }

    async fn create_requirement(
        &self,
        request: Request<proto::CreateRequirementRequest>,
    ) -> Result<Response<proto::CreateRequirementResponse>, Status> {
        let req = request.into_inner();
        let mut store = self.state.store.write().await;

        let status_enum = proto::RequirementStatus::try_from(req.status)
            .unwrap_or(proto::RequirementStatus::Draft);
        let priority_enum = proto::RequirementPriority::try_from(req.priority)
            .unwrap_or(proto::RequirementPriority::Medium);
        let type_enum = proto::RequirementType::try_from(req.req_type)
            .unwrap_or(proto::RequirementType::Functional);

        let mut new_req = Requirement::new(req.title, req.description);
        new_req.status = proto_to_status(status_enum);
        new_req.priority = proto_to_priority(priority_enum);
        new_req.req_type = proto_to_req_type(type_enum);
        new_req.owner = req.owner;
        new_req.feature = if req.feature.is_empty() {
            "Uncategorized".to_string()
        } else {
            req.feature
        };
        new_req.tags = req.tags.into_iter().collect();
        new_req.prefix_override = if req.prefix_override.is_empty() {
            None
        } else {
            Some(req.prefix_override)
        };
        new_req.created_by = if req.created_by.is_empty() {
            None
        } else {
            Some(req.created_by)
        };

        // Add to store with SPEC-ID assignment
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

        // Get the added requirement with its SPEC-ID
        let added_req = store
            .requirements
            .last()
            .ok_or_else(|| Status::internal("Failed to add requirement"))?;
        let spec_id = added_req.spec_id.clone().unwrap_or_default();

        // Save to disk
        drop(store);
        self.state.save().await?;

        // Re-read to get the final requirement
        let store = self.state.store.read().await;
        let (_, final_req) = store
            .requirements
            .iter()
            .enumerate()
            .next_back()
            .ok_or_else(|| Status::internal("Requirement not found after save"))?;

        Ok(Response::new(proto::CreateRequirementResponse {
            requirement: Some(requirement_to_proto(final_req)),
            spec_id,
        }))
    }

    async fn update_requirement(
        &self,
        request: Request<proto::UpdateRequirementRequest>,
    ) -> Result<Response<proto::UpdateRequirementResponse>, Status> {
        let req = request.into_inner();
        let mut store = self.state.store.write().await;

        let (idx, _) = ServerState::find_requirement(&store, &req.id)
            .ok_or_else(|| Status::not_found(format!("Requirement not found: {}", req.id)))?;

        let requirement = &mut store.requirements[idx];
        let author = if req.modified_by.is_empty() {
            "system".to_string()
        } else {
            req.modified_by.clone()
        };

        let mut changes = Vec::new();

        // Apply updates
        if let Some(title) = req.title {
            if requirement.title != title {
                changes.push(Requirement::field_change(
                    "title",
                    requirement.title.clone(),
                    title.clone(),
                ));
                requirement.title = title;
            }
        }
        if let Some(description) = req.description {
            if requirement.description != description {
                changes.push(Requirement::field_change(
                    "description",
                    requirement.description.clone(),
                    description.clone(),
                ));
                requirement.description = description;
            }
        }
        if let Some(status) = req.status {
            let status_enum = proto::RequirementStatus::try_from(status)
                .unwrap_or(proto::RequirementStatus::Unspecified);
            if status_enum != proto::RequirementStatus::Unspecified {
                let new_status = proto_to_status(status_enum);
                if requirement.status != new_status {
                    changes.push(Requirement::field_change(
                        "status",
                        requirement.status.to_string(),
                        new_status.to_string(),
                    ));
                    requirement.status = new_status;
                }
            }
        }
        if let Some(priority) = req.priority {
            let priority_enum = proto::RequirementPriority::try_from(priority)
                .unwrap_or(proto::RequirementPriority::Unspecified);
            if priority_enum != proto::RequirementPriority::Unspecified {
                let new_priority = proto_to_priority(priority_enum);
                if requirement.priority != new_priority {
                    changes.push(Requirement::field_change(
                        "priority",
                        requirement.priority.to_string(),
                        new_priority.to_string(),
                    ));
                    requirement.priority = new_priority;
                }
            }
        }
        if let Some(owner) = req.owner {
            if requirement.owner != owner {
                changes.push(Requirement::field_change(
                    "owner",
                    requirement.owner.clone(),
                    owner.clone(),
                ));
                requirement.owner = owner;
            }
        }
        if let Some(feature) = req.feature {
            if requirement.feature != feature {
                changes.push(Requirement::field_change(
                    "feature",
                    requirement.feature.clone(),
                    feature.clone(),
                ));
                requirement.feature = feature;
            }
        }
        if let Some(req_type) = req.req_type {
            let type_enum = proto::RequirementType::try_from(req_type)
                .unwrap_or(proto::RequirementType::Unspecified);
            if type_enum != proto::RequirementType::Unspecified {
                let new_type = proto_to_req_type(type_enum);
                if requirement.req_type != new_type {
                    changes.push(Requirement::field_change(
                        "type",
                        requirement.req_type.to_string(),
                        new_type.to_string(),
                    ));
                    requirement.req_type = new_type;
                }
            }
        }
        if let Some(archived) = req.archived {
            if requirement.archived != archived {
                changes.push(Requirement::field_change(
                    "archived",
                    requirement.archived.to_string(),
                    archived.to_string(),
                ));
                requirement.archived = archived;
            }
        }
        if let Some(custom_status) = req.custom_status {
            if requirement.custom_status.as_deref() != Some(&custom_status) {
                requirement.custom_status = if custom_status.is_empty() {
                    None
                } else {
                    Some(custom_status)
                };
            }
        }
        if let Some(custom_priority) = req.custom_priority {
            if requirement.custom_priority.as_deref() != Some(&custom_priority) {
                requirement.custom_priority = if custom_priority.is_empty() {
                    None
                } else {
                    Some(custom_priority)
                };
            }
        }

        // Handle tags
        if !req.tags.is_empty() || req.replace_tags {
            if req.replace_tags {
                requirement.tags = req.tags.into_iter().collect();
            } else {
                for tag in req.tags {
                    requirement.tags.insert(tag);
                }
            }
        }

        // Handle custom fields
        if !req.custom_fields.is_empty() || req.replace_custom_fields {
            if req.replace_custom_fields {
                requirement.custom_fields = req.custom_fields;
            } else {
                for (key, value) in req.custom_fields {
                    requirement.custom_fields.insert(key, value);
                }
            }
        }

        // Record changes in history
        if !changes.is_empty() {
            requirement.record_change(author, changes);
        }

        requirement.modified_at = chrono::Utc::now();

        let updated_req = requirement_to_proto(requirement);

        // Save
        drop(store);
        self.state.save().await?;

        Ok(Response::new(proto::UpdateRequirementResponse {
            requirement: Some(updated_req),
        }))
    }

    async fn delete_requirement(
        &self,
        request: Request<proto::DeleteRequirementRequest>,
    ) -> Result<Response<proto::DeleteRequirementResponse>, Status> {
        let req = request.into_inner();
        let mut store = self.state.store.write().await;

        let (idx, _) = ServerState::find_requirement(&store, &req.id)
            .ok_or_else(|| Status::not_found(format!("Requirement not found: {}", req.id)))?;

        store.requirements.remove(idx);

        drop(store);
        self.state.save().await?;

        Ok(Response::new(proto::DeleteRequirementResponse {
            success: true,
            message: format!("Requirement {} deleted", req.id),
        }))
    }

    // ========================================================================
    // Batch operations
    // ========================================================================

    async fn batch_create_requirements(
        &self,
        request: Request<proto::BatchCreateRequirementsRequest>,
    ) -> Result<Response<proto::BatchCreateRequirementsResponse>, Status> {
        let req = request.into_inner();
        let mut results = Vec::new();
        let mut success_count = 0i32;
        let mut failure_count = 0i32;

        for create_req in req.requirements {
            match self.create_requirement(Request::new(create_req)).await {
                Ok(response) => {
                    results.push(response.into_inner());
                    success_count += 1;
                }
                Err(_) => {
                    failure_count += 1;
                }
            }
        }

        Ok(Response::new(proto::BatchCreateRequirementsResponse {
            results,
            success_count,
            failure_count,
        }))
    }

    async fn batch_update_requirements(
        &self,
        request: Request<proto::BatchUpdateRequirementsRequest>,
    ) -> Result<Response<proto::BatchUpdateRequirementsResponse>, Status> {
        let req = request.into_inner();
        let mut results = Vec::new();
        let mut success_count = 0i32;
        let mut failure_count = 0i32;

        for update_req in req.requirements {
            match self.update_requirement(Request::new(update_req)).await {
                Ok(response) => {
                    results.push(response.into_inner());
                    success_count += 1;
                }
                Err(_) => {
                    failure_count += 1;
                }
            }
        }

        Ok(Response::new(proto::BatchUpdateRequirementsResponse {
            results,
            success_count,
            failure_count,
        }))
    }

    async fn batch_delete_requirements(
        &self,
        request: Request<proto::BatchDeleteRequirementsRequest>,
    ) -> Result<Response<proto::BatchDeleteRequirementsResponse>, Status> {
        let req = request.into_inner();
        let mut success_count = 0i32;
        let mut failure_count = 0i32;
        let mut failed_ids = Vec::new();

        for id in req.ids {
            let delete_req = proto::DeleteRequirementRequest { id: id.clone() };
            match self.delete_requirement(Request::new(delete_req)).await {
                Ok(_) => success_count += 1,
                Err(_) => {
                    failure_count += 1;
                    failed_ids.push(id);
                }
            }
        }

        Ok(Response::new(proto::BatchDeleteRequirementsResponse {
            success_count,
            failure_count,
            failed_ids,
        }))
    }

    // ========================================================================
    // Comment operations
    // ========================================================================

    async fn add_comment(
        &self,
        request: Request<proto::AddCommentRequest>,
    ) -> Result<Response<proto::AddCommentResponse>, Status> {
        let req = request.into_inner();
        let mut store = self.state.store.write().await;

        let (idx, _) =
            ServerState::find_requirement(&store, &req.requirement_id).ok_or_else(|| {
                Status::not_found(format!("Requirement not found: {}", req.requirement_id))
            })?;

        let parent_id = if req.parent_comment_id.is_empty() {
            None
        } else {
            Some(
                Uuid::parse_str(&req.parent_comment_id)
                    .map_err(|_| Status::invalid_argument("Invalid parent comment ID"))?,
            )
        };

        let comment = aida_core::Comment {
            id: Uuid::now_v7(),
            content: req.content,
            author: req.author,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            parent_id,
            replies: Vec::new(),
            reactions: Vec::new(),
            // trace:TASK-330 | ai:claude — REST/gRPC writes carry no session id
            session_id: None,
        };

        store.requirements[idx].comments.push(comment.clone());

        drop(store);
        self.state.save().await?;

        Ok(Response::new(proto::AddCommentResponse {
            comment: Some(comment_to_proto(&comment)),
        }))
    }

    async fn update_comment(
        &self,
        request: Request<proto::UpdateCommentRequest>,
    ) -> Result<Response<proto::UpdateCommentResponse>, Status> {
        let req = request.into_inner();
        let mut store = self.state.store.write().await;

        let (idx, _) =
            ServerState::find_requirement(&store, &req.requirement_id).ok_or_else(|| {
                Status::not_found(format!("Requirement not found: {}", req.requirement_id))
            })?;

        let comment_id = Uuid::parse_str(&req.comment_id)
            .map_err(|_| Status::invalid_argument("Invalid comment ID"))?;

        let comment = store.requirements[idx]
            .comments
            .iter_mut()
            .find(|c| c.id == comment_id)
            .ok_or_else(|| Status::not_found("Comment not found"))?;

        comment.content = req.content;
        comment.modified_at = chrono::Utc::now();

        let updated_comment = comment_to_proto(comment);

        drop(store);
        self.state.save().await?;

        Ok(Response::new(proto::UpdateCommentResponse {
            comment: Some(updated_comment),
        }))
    }

    async fn delete_comment(
        &self,
        request: Request<proto::DeleteCommentRequest>,
    ) -> Result<Response<proto::DeleteCommentResponse>, Status> {
        let req = request.into_inner();
        let mut store = self.state.store.write().await;

        let (idx, _) =
            ServerState::find_requirement(&store, &req.requirement_id).ok_or_else(|| {
                Status::not_found(format!("Requirement not found: {}", req.requirement_id))
            })?;

        let comment_id = Uuid::parse_str(&req.comment_id)
            .map_err(|_| Status::invalid_argument("Invalid comment ID"))?;

        let original_len = store.requirements[idx].comments.len();
        store.requirements[idx]
            .comments
            .retain(|c| c.id != comment_id);

        if store.requirements[idx].comments.len() == original_len {
            return Err(Status::not_found("Comment not found"));
        }

        drop(store);
        self.state.save().await?;

        Ok(Response::new(proto::DeleteCommentResponse {
            success: true,
        }))
    }

    // ========================================================================
    // Relationship operations
    // ========================================================================

    async fn add_relationship(
        &self,
        request: Request<proto::AddRelationshipRequest>,
    ) -> Result<Response<proto::AddRelationshipResponse>, Status> {
        let req = request.into_inner();
        let mut store = self.state.store.write().await;

        let (source_idx, _) =
            ServerState::find_requirement(&store, &req.source_id).ok_or_else(|| {
                Status::not_found(format!("Source requirement not found: {}", req.source_id))
            })?;

        let (_, target) =
            ServerState::find_requirement(&store, &req.target_id).ok_or_else(|| {
                Status::not_found(format!("Target requirement not found: {}", req.target_id))
            })?;

        let rel_type_enum = proto::RelationshipType::try_from(req.rel_type)
            .unwrap_or(proto::RelationshipType::References);

        let relationship = aida_core::Relationship {
            target_id: target.id,
            rel_type: proto_to_rel_type(rel_type_enum, &req.custom_type_name),
            created_at: Some(chrono::Utc::now()),
            created_by: if req.created_by.is_empty() {
                None
            } else {
                Some(req.created_by)
            },
        };

        store.requirements[source_idx]
            .relationships
            .push(relationship);

        drop(store);
        self.state.save().await?;

        Ok(Response::new(proto::AddRelationshipResponse {
            success: true,
            message: "Relationship added".to_string(),
        }))
    }

    async fn remove_relationship(
        &self,
        request: Request<proto::RemoveRelationshipRequest>,
    ) -> Result<Response<proto::RemoveRelationshipResponse>, Status> {
        let req = request.into_inner();
        let mut store = self.state.store.write().await;

        let (source_idx, _) =
            ServerState::find_requirement(&store, &req.source_id).ok_or_else(|| {
                Status::not_found(format!("Source requirement not found: {}", req.source_id))
            })?;

        let target_uuid = if let Ok(uuid) = Uuid::parse_str(&req.target_id) {
            uuid
        } else {
            // Find by SPEC-ID
            ServerState::find_requirement(&store, &req.target_id)
                .map(|(_, r)| r.id)
                .ok_or_else(|| {
                    Status::not_found(format!("Target requirement not found: {}", req.target_id))
                })?
        };

        let rel_type_enum = proto::RelationshipType::try_from(req.rel_type)
            .unwrap_or(proto::RelationshipType::Unspecified);

        let original_len = store.requirements[source_idx].relationships.len();
        store.requirements[source_idx].relationships.retain(|r| {
            r.target_id != target_uuid || {
                let (r_type, _) = rel_type_to_proto(&r.rel_type);
                r_type != rel_type_enum
            }
        });

        if store.requirements[source_idx].relationships.len() == original_len {
            return Err(Status::not_found("Relationship not found"));
        }

        drop(store);
        self.state.save().await?;

        Ok(Response::new(proto::RemoveRelationshipResponse {
            success: true,
        }))
    }

    // ========================================================================
    // Search
    // ========================================================================

    async fn search_requirements(
        &self,
        request: Request<proto::SearchRequirementsRequest>,
    ) -> Result<Response<proto::SearchRequirementsResponse>, Status> {
        let req = request.into_inner();
        let store = self.state.store.read().await;
        let query = req.query.to_lowercase();

        let mut matches: Vec<&Requirement> = store
            .requirements
            .iter()
            .filter(|r| {
                let mut found = false;
                if req.search_title {
                    found = found || r.title.to_lowercase().contains(&query);
                }
                if req.search_description {
                    found = found || r.description.to_lowercase().contains(&query);
                }
                if req.search_spec_id {
                    found = found
                        || r.spec_id
                            .as_ref()
                            .map(|s| s.to_lowercase().contains(&query))
                            .unwrap_or(false);
                }
                if req.search_comments {
                    found = found
                        || r.comments
                            .iter()
                            .any(|c| c.content.to_lowercase().contains(&query));
                }
                found
            })
            .collect();

        // Apply additional filters
        if !req.status_filter.is_empty() {
            matches.retain(|r| r.effective_status() == req.status_filter);
        }
        if !req.type_filter.is_empty() {
            matches.retain(|r| r.req_type.to_string() == req.type_filter);
        }
        if !req.feature_filter.is_empty() {
            matches.retain(|r| r.feature == req.feature_filter);
        }
        if !req.include_archived {
            matches.retain(|r| !r.archived);
        }

        let total_matches = matches.len() as i32;

        if req.limit > 0 {
            matches = matches.into_iter().take(req.limit as usize).collect();
        }

        Ok(Response::new(proto::SearchRequirementsResponse {
            requirements: matches.iter().map(|r| requirement_to_proto(r)).collect(),
            total_matches,
        }))
    }

    // ========================================================================
    // Server status
    // ========================================================================

    async fn get_server_status(
        &self,
        _request: Request<proto::GetServerStatusRequest>,
    ) -> Result<Response<proto::GetServerStatusResponse>, Status> {
        let shutdown_requested = *self.state.shutdown_requested.read().await;
        let status = if shutdown_requested {
            "shutting_down"
        } else {
            "running"
        };

        let backend_type = self.state.backend.backend_type();
        Ok(Response::new(proto::GetServerStatusResponse {
            version: self.state.version.clone(),
            status: status.to_string(),
            uptime_seconds: self.state.start_time.elapsed().as_secs() as i64,
            active_connections: 0, // TODO: Track this
            storage_backend: backend_type.to_string().to_lowercase(),
            storage_path: format!("{}", backend_type),
        }))
    }

    async fn shutdown(
        &self,
        request: Request<proto::ShutdownRequest>,
    ) -> Result<Response<proto::ShutdownResponse>, Status> {
        let req = request.into_inner();
        let mut shutdown = self.state.shutdown_requested.write().await;
        *shutdown = true;

        // In a real implementation, we'd signal the server to shut down gracefully
        // For now, just acknowledge the request
        tracing::info!("Shutdown requested with timeout {}s", req.timeout_seconds);

        Ok(Response::new(proto::ShutdownResponse {
            accepted: true,
            message: format!("Shutdown initiated with {}s timeout", req.timeout_seconds),
        }))
    }

    // ========================================================================
    // Authentication
    // ========================================================================

    async fn login(
        &self,
        request: Request<proto::LoginRequest>,
    ) -> Result<Response<proto::LoginResponse>, Status> {
        let req = request.into_inner();
        let store = self.state.store.read().await;

        // Find user by handle or name
        let user = store.users.iter().find(|u| {
            u.handle.eq_ignore_ascii_case(&req.identifier)
                || u.name.eq_ignore_ascii_case(&req.identifier)
                || u.spec_id
                    .as_ref()
                    .map(|s| s.eq_ignore_ascii_case(&req.identifier))
                    .unwrap_or(false)
        });

        match user {
            Some(user) => {
                // Check if user has a PIN set
                if !user.has_pin() {
                    // No PIN set - allow login (first time login)
                    tracing::info!("User {} logged in (no PIN required)", user.handle);
                    Ok(Response::new(proto::LoginResponse {
                        success: true,
                        message: "Login successful (no PIN set)".to_string(),
                        user: Some(user_to_proto(user)),
                        session_token: String::new(), // Future: generate session token
                    }))
                } else if user.verify_pin(&req.pin) {
                    // PIN verified
                    tracing::info!("User {} logged in with PIN", user.handle);
                    Ok(Response::new(proto::LoginResponse {
                        success: true,
                        message: "Login successful".to_string(),
                        user: Some(user_to_proto(user)),
                        session_token: String::new(), // Future: generate session token
                    }))
                } else {
                    // PIN incorrect
                    tracing::warn!("Failed login attempt for user {}", user.handle);
                    Ok(Response::new(proto::LoginResponse {
                        success: false,
                        message: "Invalid PIN".to_string(),
                        user: None,
                        session_token: String::new(),
                    }))
                }
            }
            None => {
                tracing::warn!("Login attempt for unknown user: {}", req.identifier);
                Ok(Response::new(proto::LoginResponse {
                    success: false,
                    message: "User not found".to_string(),
                    user: None,
                    session_token: String::new(),
                }))
            }
        }
    }

    async fn set_user_pin(
        &self,
        request: Request<proto::SetUserPinRequest>,
    ) -> Result<Response<proto::SetUserPinResponse>, Status> {
        let req = request.into_inner();
        let mut store = self.state.store.write().await;

        // Find user by UUID or SPEC-ID
        let user_idx = store.users.iter().position(|u| {
            u.id.to_string() == req.user_id
                || u.spec_id
                    .as_ref()
                    .map(|s| s == &req.user_id)
                    .unwrap_or(false)
                || u.handle.eq_ignore_ascii_case(&req.user_id)
        });

        match user_idx {
            Some(idx) => {
                let user = &store.users[idx];

                // If user has an existing PIN, verify current PIN
                if user.has_pin() && !user.verify_pin(&req.current_pin) {
                    return Ok(Response::new(proto::SetUserPinResponse {
                        success: false,
                        message: "Current PIN is incorrect".to_string(),
                    }));
                }

                // Set new PIN
                store.users[idx].set_pin(&req.new_pin);

                drop(store);
                self.state.save().await?;

                tracing::info!("PIN set for user {}", req.user_id);
                Ok(Response::new(proto::SetUserPinResponse {
                    success: true,
                    message: "PIN set successfully".to_string(),
                }))
            }
            None => Ok(Response::new(proto::SetUserPinResponse {
                success: false,
                message: "User not found".to_string(),
            })),
        }
    }
}

// ============================================================================
// Multi-Project gRPC Service
// ============================================================================

/// Multi-project gRPC service that routes requests based on x-project metadata
pub struct AidaServiceMultiProject {
    project_manager: Arc<ProjectManager>,
}

impl AidaServiceMultiProject {
    pub fn new(project_manager: Arc<ProjectManager>) -> Self {
        Self { project_manager }
    }

    /// Extract project name from request metadata
    // why: the Err type is tonic's own `Status` (the gRPC error contract); every
    // handler returns `Result<_, Status>` so boxing here would diverge from tonic.
    #[allow(clippy::result_large_err)]
    fn extract_project<T>(request: &Request<T>) -> Result<String, Status> {
        request
            .metadata()
            .get("x-project")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| Status::invalid_argument("Missing x-project metadata header"))
    }

    /// Get the backend for a project
    async fn get_backend(&self, project: &str) -> Result<Arc<ServerState>, Status> {
        self.project_manager
            .get_backend(project)
            .await
            .map_err(|e| Status::not_found(format!("Project error: {}", e)))
    }
}

#[tonic::async_trait]
impl RequirementsService for AidaServiceMultiProject {
    async fn get_store(
        &self,
        request: Request<proto::GetStoreRequest>,
    ) -> Result<Response<proto::GetStoreResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let store = state.store.read().await;
        Ok(Response::new(proto::GetStoreResponse {
            store: Some(store_to_proto(&store)),
        }))
    }

    async fn get_store_metadata(
        &self,
        request: Request<proto::GetStoreMetadataRequest>,
    ) -> Result<Response<proto::GetStoreMetadataResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let store = state.store.read().await;
        Ok(Response::new(proto::GetStoreMetadataResponse {
            name: store.name.clone(),
            title: store.title.clone(),
            description: store.description.clone(),
            requirement_count: store.requirements.len() as i32,
            user_count: store.users.len() as i32,
            feature_count: store.features.len() as i32,
        }))
    }

    async fn list_requirements(
        &self,
        request: Request<proto::ListRequirementsRequest>,
    ) -> Result<Response<proto::ListRequirementsResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let store = state.store.read().await;

        let mut requirements: Vec<&Requirement> = store.requirements.iter().collect();

        if !req.status_filter.is_empty() {
            requirements.retain(|r| r.effective_status() == req.status_filter);
        }
        if !req.priority_filter.is_empty() {
            requirements.retain(|r| r.priority.to_string() == req.priority_filter);
        }
        if !req.type_filter.is_empty() {
            requirements.retain(|r| r.req_type.to_string() == req.type_filter);
        }
        if !req.feature_filter.is_empty() {
            requirements.retain(|r| r.feature == req.feature_filter);
        }
        if !req.owner_filter.is_empty() {
            requirements.retain(|r| r.owner == req.owner_filter);
        }
        if !req.include_archived {
            requirements.retain(|r| !r.archived);
        }

        let total_count = requirements.len() as i32;

        if req.offset > 0 {
            requirements = requirements.into_iter().skip(req.offset as usize).collect();
        }
        if req.limit > 0 {
            requirements = requirements.into_iter().take(req.limit as usize).collect();
        }

        Ok(Response::new(proto::ListRequirementsResponse {
            requirements: requirements
                .iter()
                .map(|r| requirement_to_proto(r))
                .collect(),
            total_count,
        }))
    }

    async fn get_requirement(
        &self,
        request: Request<proto::GetRequirementRequest>,
    ) -> Result<Response<proto::GetRequirementResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let store = state.store.read().await;

        let (_, requirement) = ServerState::find_requirement(&store, &req.id)
            .ok_or_else(|| Status::not_found(format!("Requirement not found: {}", req.id)))?;

        Ok(Response::new(proto::GetRequirementResponse {
            requirement: Some(requirement_to_proto(requirement)),
        }))
    }

    async fn create_requirement(
        &self,
        request: Request<proto::CreateRequirementRequest>,
    ) -> Result<Response<proto::CreateRequirementResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let mut store = state.store.write().await;

        let status_enum = proto::RequirementStatus::try_from(req.status)
            .unwrap_or(proto::RequirementStatus::Draft);
        let priority_enum = proto::RequirementPriority::try_from(req.priority)
            .unwrap_or(proto::RequirementPriority::Medium);
        let type_enum = proto::RequirementType::try_from(req.req_type)
            .unwrap_or(proto::RequirementType::Functional);

        let mut new_req = Requirement::new(req.title, req.description);
        new_req.status = proto_to_status(status_enum);
        new_req.priority = proto_to_priority(priority_enum);
        new_req.req_type = proto_to_req_type(type_enum);
        new_req.owner = req.owner;
        new_req.feature = if req.feature.is_empty() {
            "Uncategorized".to_string()
        } else {
            req.feature
        };
        new_req.tags = req.tags.into_iter().collect();
        new_req.prefix_override = if req.prefix_override.is_empty() {
            None
        } else {
            Some(req.prefix_override)
        };
        new_req.created_by = if req.created_by.is_empty() {
            None
        } else {
            Some(req.created_by)
        };

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

        let added_req = store
            .requirements
            .last()
            .ok_or_else(|| Status::internal("Failed to add requirement"))?;
        let spec_id = added_req.spec_id.clone().unwrap_or_default();

        drop(store);
        state.save().await?;

        let store = state.store.read().await;
        let (_, final_req) = store
            .requirements
            .iter()
            .enumerate()
            .next_back()
            .ok_or_else(|| Status::internal("Requirement not found after save"))?;

        Ok(Response::new(proto::CreateRequirementResponse {
            requirement: Some(requirement_to_proto(final_req)),
            spec_id,
        }))
    }

    async fn update_requirement(
        &self,
        request: Request<proto::UpdateRequirementRequest>,
    ) -> Result<Response<proto::UpdateRequirementResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let mut store = state.store.write().await;

        let (idx, _) = ServerState::find_requirement(&store, &req.id)
            .ok_or_else(|| Status::not_found(format!("Requirement not found: {}", req.id)))?;

        let requirement = &mut store.requirements[idx];
        let author = if req.modified_by.is_empty() {
            "system".to_string()
        } else {
            req.modified_by.clone()
        };

        let mut changes = Vec::new();

        if let Some(title) = req.title {
            if requirement.title != title {
                changes.push(Requirement::field_change(
                    "title",
                    requirement.title.clone(),
                    title.clone(),
                ));
                requirement.title = title;
            }
        }
        if let Some(description) = req.description {
            if requirement.description != description {
                changes.push(Requirement::field_change(
                    "description",
                    requirement.description.clone(),
                    description.clone(),
                ));
                requirement.description = description;
            }
        }
        if let Some(status) = req.status {
            let status_enum = proto::RequirementStatus::try_from(status)
                .unwrap_or(proto::RequirementStatus::Unspecified);
            if status_enum != proto::RequirementStatus::Unspecified {
                let new_status = proto_to_status(status_enum);
                if requirement.status != new_status {
                    changes.push(Requirement::field_change(
                        "status",
                        requirement.status.to_string(),
                        new_status.to_string(),
                    ));
                    requirement.status = new_status;
                }
            }
        }
        if let Some(priority) = req.priority {
            let priority_enum = proto::RequirementPriority::try_from(priority)
                .unwrap_or(proto::RequirementPriority::Unspecified);
            if priority_enum != proto::RequirementPriority::Unspecified {
                let new_priority = proto_to_priority(priority_enum);
                if requirement.priority != new_priority {
                    changes.push(Requirement::field_change(
                        "priority",
                        requirement.priority.to_string(),
                        new_priority.to_string(),
                    ));
                    requirement.priority = new_priority;
                }
            }
        }
        if let Some(owner) = req.owner {
            if requirement.owner != owner {
                changes.push(Requirement::field_change(
                    "owner",
                    requirement.owner.clone(),
                    owner.clone(),
                ));
                requirement.owner = owner;
            }
        }
        if let Some(feature) = req.feature {
            if requirement.feature != feature {
                changes.push(Requirement::field_change(
                    "feature",
                    requirement.feature.clone(),
                    feature.clone(),
                ));
                requirement.feature = feature;
            }
        }
        if let Some(req_type) = req.req_type {
            let type_enum = proto::RequirementType::try_from(req_type)
                .unwrap_or(proto::RequirementType::Unspecified);
            if type_enum != proto::RequirementType::Unspecified {
                let new_type = proto_to_req_type(type_enum);
                if requirement.req_type != new_type {
                    changes.push(Requirement::field_change(
                        "type",
                        requirement.req_type.to_string(),
                        new_type.to_string(),
                    ));
                    requirement.req_type = new_type;
                }
            }
        }
        if let Some(archived) = req.archived {
            if requirement.archived != archived {
                changes.push(Requirement::field_change(
                    "archived",
                    requirement.archived.to_string(),
                    archived.to_string(),
                ));
                requirement.archived = archived;
            }
        }
        if let Some(custom_status) = req.custom_status {
            if requirement.custom_status.as_deref() != Some(&custom_status) {
                requirement.custom_status = if custom_status.is_empty() {
                    None
                } else {
                    Some(custom_status)
                };
            }
        }
        if let Some(custom_priority) = req.custom_priority {
            if requirement.custom_priority.as_deref() != Some(&custom_priority) {
                requirement.custom_priority = if custom_priority.is_empty() {
                    None
                } else {
                    Some(custom_priority)
                };
            }
        }

        if !req.tags.is_empty() || req.replace_tags {
            if req.replace_tags {
                requirement.tags = req.tags.into_iter().collect();
            } else {
                for tag in req.tags {
                    requirement.tags.insert(tag);
                }
            }
        }

        if !req.custom_fields.is_empty() || req.replace_custom_fields {
            if req.replace_custom_fields {
                requirement.custom_fields = req.custom_fields;
            } else {
                for (key, value) in req.custom_fields {
                    requirement.custom_fields.insert(key, value);
                }
            }
        }

        if !changes.is_empty() {
            requirement.record_change(author, changes);
        }

        requirement.modified_at = chrono::Utc::now();

        let updated_req = requirement_to_proto(requirement);

        drop(store);
        state.save().await?;

        Ok(Response::new(proto::UpdateRequirementResponse {
            requirement: Some(updated_req),
        }))
    }

    async fn delete_requirement(
        &self,
        request: Request<proto::DeleteRequirementRequest>,
    ) -> Result<Response<proto::DeleteRequirementResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let mut store = state.store.write().await;

        let (idx, _) = ServerState::find_requirement(&store, &req.id)
            .ok_or_else(|| Status::not_found(format!("Requirement not found: {}", req.id)))?;

        store.requirements.remove(idx);

        drop(store);
        state.save().await?;

        Ok(Response::new(proto::DeleteRequirementResponse {
            success: true,
            message: format!("Requirement {} deleted", req.id),
        }))
    }

    async fn batch_create_requirements(
        &self,
        request: Request<proto::BatchCreateRequirementsRequest>,
    ) -> Result<Response<proto::BatchCreateRequirementsResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let req = request.into_inner();
        let mut results = Vec::new();
        let mut success_count = 0i32;
        let mut failure_count = 0i32;

        for create_req in req.requirements {
            let mut inner_request = Request::new(create_req);
            inner_request
                .metadata_mut()
                .insert("x-project", project.parse().unwrap());
            match self.create_requirement(inner_request).await {
                Ok(response) => {
                    results.push(response.into_inner());
                    success_count += 1;
                }
                Err(_) => {
                    failure_count += 1;
                }
            }
        }

        Ok(Response::new(proto::BatchCreateRequirementsResponse {
            results,
            success_count,
            failure_count,
        }))
    }

    async fn batch_update_requirements(
        &self,
        request: Request<proto::BatchUpdateRequirementsRequest>,
    ) -> Result<Response<proto::BatchUpdateRequirementsResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let req = request.into_inner();
        let mut results = Vec::new();
        let mut success_count = 0i32;
        let mut failure_count = 0i32;

        for update_req in req.requirements {
            let mut inner_request = Request::new(update_req);
            inner_request
                .metadata_mut()
                .insert("x-project", project.parse().unwrap());
            match self.update_requirement(inner_request).await {
                Ok(response) => {
                    results.push(response.into_inner());
                    success_count += 1;
                }
                Err(_) => {
                    failure_count += 1;
                }
            }
        }

        Ok(Response::new(proto::BatchUpdateRequirementsResponse {
            results,
            success_count,
            failure_count,
        }))
    }

    async fn batch_delete_requirements(
        &self,
        request: Request<proto::BatchDeleteRequirementsRequest>,
    ) -> Result<Response<proto::BatchDeleteRequirementsResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let req = request.into_inner();
        let mut success_count = 0i32;
        let mut failure_count = 0i32;
        let mut failed_ids = Vec::new();

        for id in req.ids {
            let delete_req = proto::DeleteRequirementRequest { id: id.clone() };
            let mut inner_request = Request::new(delete_req);
            inner_request
                .metadata_mut()
                .insert("x-project", project.parse().unwrap());
            match self.delete_requirement(inner_request).await {
                Ok(_) => success_count += 1,
                Err(_) => {
                    failure_count += 1;
                    failed_ids.push(id);
                }
            }
        }

        Ok(Response::new(proto::BatchDeleteRequirementsResponse {
            success_count,
            failure_count,
            failed_ids,
        }))
    }

    async fn add_comment(
        &self,
        request: Request<proto::AddCommentRequest>,
    ) -> Result<Response<proto::AddCommentResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let mut store = state.store.write().await;

        let (idx, _) =
            ServerState::find_requirement(&store, &req.requirement_id).ok_or_else(|| {
                Status::not_found(format!("Requirement not found: {}", req.requirement_id))
            })?;

        let parent_id = if req.parent_comment_id.is_empty() {
            None
        } else {
            Some(
                Uuid::parse_str(&req.parent_comment_id)
                    .map_err(|_| Status::invalid_argument("Invalid parent comment ID"))?,
            )
        };

        let comment = aida_core::Comment {
            id: Uuid::now_v7(),
            content: req.content,
            author: req.author,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            parent_id,
            replies: Vec::new(),
            reactions: Vec::new(),
            // trace:TASK-330 | ai:claude — REST/gRPC writes carry no session id
            session_id: None,
        };

        store.requirements[idx].comments.push(comment.clone());

        drop(store);
        state.save().await?;

        Ok(Response::new(proto::AddCommentResponse {
            comment: Some(comment_to_proto(&comment)),
        }))
    }

    async fn update_comment(
        &self,
        request: Request<proto::UpdateCommentRequest>,
    ) -> Result<Response<proto::UpdateCommentResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let mut store = state.store.write().await;

        let (idx, _) =
            ServerState::find_requirement(&store, &req.requirement_id).ok_or_else(|| {
                Status::not_found(format!("Requirement not found: {}", req.requirement_id))
            })?;

        let comment_id = Uuid::parse_str(&req.comment_id)
            .map_err(|_| Status::invalid_argument("Invalid comment ID"))?;

        let comment = store.requirements[idx]
            .comments
            .iter_mut()
            .find(|c| c.id == comment_id)
            .ok_or_else(|| Status::not_found("Comment not found"))?;

        comment.content = req.content;
        comment.modified_at = chrono::Utc::now();

        let updated_comment = comment_to_proto(comment);

        drop(store);
        state.save().await?;

        Ok(Response::new(proto::UpdateCommentResponse {
            comment: Some(updated_comment),
        }))
    }

    async fn delete_comment(
        &self,
        request: Request<proto::DeleteCommentRequest>,
    ) -> Result<Response<proto::DeleteCommentResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let mut store = state.store.write().await;

        let (idx, _) =
            ServerState::find_requirement(&store, &req.requirement_id).ok_or_else(|| {
                Status::not_found(format!("Requirement not found: {}", req.requirement_id))
            })?;

        let comment_id = Uuid::parse_str(&req.comment_id)
            .map_err(|_| Status::invalid_argument("Invalid comment ID"))?;

        let original_len = store.requirements[idx].comments.len();
        store.requirements[idx]
            .comments
            .retain(|c| c.id != comment_id);

        if store.requirements[idx].comments.len() == original_len {
            return Err(Status::not_found("Comment not found"));
        }

        drop(store);
        state.save().await?;

        Ok(Response::new(proto::DeleteCommentResponse {
            success: true,
        }))
    }

    async fn add_relationship(
        &self,
        request: Request<proto::AddRelationshipRequest>,
    ) -> Result<Response<proto::AddRelationshipResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let mut store = state.store.write().await;

        let (source_idx, _) =
            ServerState::find_requirement(&store, &req.source_id).ok_or_else(|| {
                Status::not_found(format!("Source requirement not found: {}", req.source_id))
            })?;

        let (_, target) =
            ServerState::find_requirement(&store, &req.target_id).ok_or_else(|| {
                Status::not_found(format!("Target requirement not found: {}", req.target_id))
            })?;

        let rel_type_enum = proto::RelationshipType::try_from(req.rel_type)
            .unwrap_or(proto::RelationshipType::References);

        let relationship = aida_core::Relationship {
            target_id: target.id,
            rel_type: proto_to_rel_type(rel_type_enum, &req.custom_type_name),
            created_at: Some(chrono::Utc::now()),
            created_by: if req.created_by.is_empty() {
                None
            } else {
                Some(req.created_by)
            },
        };

        store.requirements[source_idx]
            .relationships
            .push(relationship);

        drop(store);
        state.save().await?;

        Ok(Response::new(proto::AddRelationshipResponse {
            success: true,
            message: "Relationship added".to_string(),
        }))
    }

    async fn remove_relationship(
        &self,
        request: Request<proto::RemoveRelationshipRequest>,
    ) -> Result<Response<proto::RemoveRelationshipResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let mut store = state.store.write().await;

        let (source_idx, _) =
            ServerState::find_requirement(&store, &req.source_id).ok_or_else(|| {
                Status::not_found(format!("Source requirement not found: {}", req.source_id))
            })?;

        let target_uuid = if let Ok(uuid) = Uuid::parse_str(&req.target_id) {
            uuid
        } else {
            ServerState::find_requirement(&store, &req.target_id)
                .map(|(_, r)| r.id)
                .ok_or_else(|| {
                    Status::not_found(format!("Target requirement not found: {}", req.target_id))
                })?
        };

        let rel_type_enum = proto::RelationshipType::try_from(req.rel_type)
            .unwrap_or(proto::RelationshipType::Unspecified);

        let original_len = store.requirements[source_idx].relationships.len();
        store.requirements[source_idx].relationships.retain(|r| {
            r.target_id != target_uuid || {
                let (r_type, _) = rel_type_to_proto(&r.rel_type);
                r_type != rel_type_enum
            }
        });

        if store.requirements[source_idx].relationships.len() == original_len {
            return Err(Status::not_found("Relationship not found"));
        }

        drop(store);
        state.save().await?;

        Ok(Response::new(proto::RemoveRelationshipResponse {
            success: true,
        }))
    }

    async fn search_requirements(
        &self,
        request: Request<proto::SearchRequirementsRequest>,
    ) -> Result<Response<proto::SearchRequirementsResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let store = state.store.read().await;
        let query = req.query.to_lowercase();

        let mut matches: Vec<&Requirement> = store
            .requirements
            .iter()
            .filter(|r| {
                let mut found = false;
                if req.search_title {
                    found = found || r.title.to_lowercase().contains(&query);
                }
                if req.search_description {
                    found = found || r.description.to_lowercase().contains(&query);
                }
                if req.search_spec_id {
                    found = found
                        || r.spec_id
                            .as_ref()
                            .map(|s| s.to_lowercase().contains(&query))
                            .unwrap_or(false);
                }
                if req.search_comments {
                    found = found
                        || r.comments
                            .iter()
                            .any(|c| c.content.to_lowercase().contains(&query));
                }
                found
            })
            .collect();

        if !req.status_filter.is_empty() {
            matches.retain(|r| r.effective_status() == req.status_filter);
        }
        if !req.type_filter.is_empty() {
            matches.retain(|r| r.req_type.to_string() == req.type_filter);
        }
        if !req.feature_filter.is_empty() {
            matches.retain(|r| r.feature == req.feature_filter);
        }
        if !req.include_archived {
            matches.retain(|r| !r.archived);
        }

        let total_matches = matches.len() as i32;

        if req.limit > 0 {
            matches = matches.into_iter().take(req.limit as usize).collect();
        }

        Ok(Response::new(proto::SearchRequirementsResponse {
            requirements: matches.iter().map(|r| requirement_to_proto(r)).collect(),
            total_matches,
        }))
    }

    async fn get_server_status(
        &self,
        _request: Request<proto::GetServerStatusRequest>,
    ) -> Result<Response<proto::GetServerStatusResponse>, Status> {
        Ok(Response::new(proto::GetServerStatusResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
            status: "running".to_string(),
            uptime_seconds: 0,
            active_connections: 0,
            storage_backend: "multi-project".to_string(),
            storage_path: self
                .project_manager
                .data_dir()
                .to_string_lossy()
                .to_string(),
        }))
    }

    async fn shutdown(
        &self,
        request: Request<proto::ShutdownRequest>,
    ) -> Result<Response<proto::ShutdownResponse>, Status> {
        let req = request.into_inner();
        tracing::info!("Shutdown requested with timeout {}s", req.timeout_seconds);

        Ok(Response::new(proto::ShutdownResponse {
            accepted: true,
            message: format!("Shutdown initiated with {}s timeout", req.timeout_seconds),
        }))
    }

    async fn login(
        &self,
        request: Request<proto::LoginRequest>,
    ) -> Result<Response<proto::LoginResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let store = state.store.read().await;

        let user = store.users.iter().find(|u| {
            u.handle.eq_ignore_ascii_case(&req.identifier)
                || u.name.eq_ignore_ascii_case(&req.identifier)
                || u.spec_id
                    .as_ref()
                    .map(|s| s.eq_ignore_ascii_case(&req.identifier))
                    .unwrap_or(false)
        });

        match user {
            Some(user) => {
                if !user.has_pin() {
                    tracing::info!("User {} logged in (no PIN required)", user.handle);
                    Ok(Response::new(proto::LoginResponse {
                        success: true,
                        message: "Login successful (no PIN set)".to_string(),
                        user: Some(user_to_proto(user)),
                        session_token: String::new(),
                    }))
                } else if user.verify_pin(&req.pin) {
                    tracing::info!("User {} logged in with PIN", user.handle);
                    Ok(Response::new(proto::LoginResponse {
                        success: true,
                        message: "Login successful".to_string(),
                        user: Some(user_to_proto(user)),
                        session_token: String::new(),
                    }))
                } else {
                    tracing::warn!("Failed login attempt for user {}", user.handle);
                    Ok(Response::new(proto::LoginResponse {
                        success: false,
                        message: "Invalid PIN".to_string(),
                        user: None,
                        session_token: String::new(),
                    }))
                }
            }
            None => {
                tracing::warn!("Login attempt for unknown user: {}", req.identifier);
                Ok(Response::new(proto::LoginResponse {
                    success: false,
                    message: "User not found".to_string(),
                    user: None,
                    session_token: String::new(),
                }))
            }
        }
    }

    async fn set_user_pin(
        &self,
        request: Request<proto::SetUserPinRequest>,
    ) -> Result<Response<proto::SetUserPinResponse>, Status> {
        let project = Self::extract_project(&request)?;
        let state = self.get_backend(&project).await?;
        let req = request.into_inner();
        let mut store = state.store.write().await;

        let user_idx = store.users.iter().position(|u| {
            u.id.to_string() == req.user_id
                || u.spec_id
                    .as_ref()
                    .map(|s| s == &req.user_id)
                    .unwrap_or(false)
                || u.handle.eq_ignore_ascii_case(&req.user_id)
        });

        match user_idx {
            Some(idx) => {
                let user = &store.users[idx];

                if user.has_pin() && !user.verify_pin(&req.current_pin) {
                    return Ok(Response::new(proto::SetUserPinResponse {
                        success: false,
                        message: "Current PIN is incorrect".to_string(),
                    }));
                }

                store.users[idx].set_pin(&req.new_pin);

                drop(store);
                state.save().await?;

                tracing::info!("PIN set for user {}", req.user_id);
                Ok(Response::new(proto::SetUserPinResponse {
                    success: true,
                    message: "PIN set successfully".to_string(),
                }))
            }
            None => Ok(Response::new(proto::SetUserPinResponse {
                success: false,
                message: "User not found".to_string(),
            })),
        }
    }
}
