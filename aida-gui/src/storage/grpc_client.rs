// trace:FR-0278 | ai:claude:high
//! gRPC client implementation of StorageClient trait
//!
//! This module provides a gRPC client that implements the StorageClient trait,
//! enabling uniform access to AIDA servers. It supports both native (via tokio)
//! and WASM (via tonic-web-wasm-client) targets.

use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};

use aida_core::{Comment, RelationshipType, Requirement, RequirementsStore};

use super::traits::{ServerStatus, StorageClient};

// Proto module - generated code
pub mod proto {
    include!("../generated/aida.rs");
}

use proto::requirements_service_client::RequirementsServiceClient;

// =============================================================================
// Native implementation (using tokio runtime)
// =============================================================================

#[cfg(not(target_arch = "wasm32"))]
use tonic::transport::Channel;

/// gRPC-based storage client
///
/// This client connects to an AIDA server via gRPC and implements
/// the StorageClient trait for uniform access.
#[cfg(not(target_arch = "wasm32"))]
pub struct GrpcStorageClient {
    server_addr: String,
    runtime: tokio::runtime::Runtime,
    client: Arc<Mutex<Option<RequirementsServiceClient<Channel>>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl GrpcStorageClient {
    /// Connect to a gRPC server
    ///
    /// # Arguments
    /// * `server_addr` - Server address (e.g., "localhost:50051" or "http://localhost:50051")
    pub fn connect(server_addr: &str) -> Result<Self> {
        let runtime = tokio::runtime::Runtime::new()
            .context("Failed to create tokio runtime")?;

        let addr = normalize_addr(server_addr);

        // Try to connect
        let client = runtime.block_on(async {
            RequirementsServiceClient::connect(addr).await
        }).context("Failed to connect to AIDA server")?;

        Ok(GrpcStorageClient {
            server_addr: server_addr.to_string(),
            runtime,
            client: Arc::new(Mutex::new(Some(client))),
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl StorageClient for GrpcStorageClient {
    fn load(&self) -> Result<RequirementsStore> {
        let mut client_guard = self.client.lock().unwrap();
        let client = client_guard.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected to server"))?;

        self.runtime.block_on(async {
            let request = tonic::Request::new(proto::GetStoreRequest {});
            let response = client.get_store(request).await
                .context("Failed to get store from server")?;
            let proto_store = response.into_inner().store
                .ok_or_else(|| anyhow::anyhow!("Server returned empty store"))?;

            proto_to_store(&proto_store)
        })
    }

    fn save(&self, store: &RequirementsStore) -> Result<()> {
        let mut client_guard = self.client.lock().unwrap();
        let client = client_guard.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected to server"))?;

        self.runtime.block_on(async {
            // Get current server state to determine what needs to be created vs updated
            let server_store_request = tonic::Request::new(proto::GetStoreRequest {});
            let server_response = client.get_store(server_store_request).await
                .context("Failed to get current server state")?;
            let server_store = server_response.into_inner().store
                .ok_or_else(|| anyhow::anyhow!("Server returned empty store"))?;

            // Build sets of server requirement IDs for comparison
            let server_req_ids: std::collections::HashSet<String> = server_store.requirements
                .iter()
                .map(|r| r.id.clone())
                .collect();

            // Separate local requirements into creates and updates
            let mut to_create = Vec::new();
            let mut to_update = Vec::new();
            let local_req_ids: std::collections::HashSet<String> = store.requirements
                .iter()
                .map(|r| r.id.to_string())
                .collect();

            for req in &store.requirements {
                let req_id = req.id.to_string();
                if server_req_ids.contains(&req_id) {
                    to_update.push(req);
                } else {
                    to_create.push(req);
                }
            }

            // Find requirements to delete (on server but not locally)
            let to_delete: Vec<String> = server_req_ids
                .difference(&local_req_ids)
                .cloned()
                .collect();

            // Batch create new requirements
            if !to_create.is_empty() {
                let create_requests: Vec<proto::CreateRequirementRequest> = to_create
                    .iter()
                    .map(|r| requirement_to_create_request(r))
                    .collect();

                let batch_create = tonic::Request::new(proto::BatchCreateRequirementsRequest {
                    requirements: create_requests,
                });
                client.batch_create_requirements(batch_create).await
                    .context("Failed to create new requirements on server")?;
            }

            // Batch update existing requirements
            if !to_update.is_empty() {
                let update_requests: Vec<proto::UpdateRequirementRequest> = to_update
                    .iter()
                    .map(|r| requirement_to_update_request(r))
                    .collect();

                let batch_update = tonic::Request::new(proto::BatchUpdateRequirementsRequest {
                    requirements: update_requests,
                });
                client.batch_update_requirements(batch_update).await
                    .context("Failed to update requirements on server")?;
            }

            // Batch delete removed requirements
            if !to_delete.is_empty() {
                let batch_delete = tonic::Request::new(proto::BatchDeleteRequirementsRequest {
                    ids: to_delete,
                });
                client.batch_delete_requirements(batch_delete).await
                    .context("Failed to delete requirements on server")?;
            }

            Ok(())
        })
    }

    fn display_path(&self) -> String {
        self.server_addr.clone()
    }

    fn is_remote(&self) -> bool {
        true
    }

    fn create_requirement(&self, req: &Requirement) -> Result<Requirement> {
        let mut client_guard = self.client.lock().unwrap();
        let client = client_guard.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected to server"))?;

        self.runtime.block_on(async {
            let request = tonic::Request::new(requirement_to_create_request(req));
            let response = client.create_requirement(request).await
                .context("Failed to create requirement on server")?;
            let created = response.into_inner().requirement
                .ok_or_else(|| anyhow::anyhow!("Server returned empty requirement"))?;
            proto_to_requirement(&created)
                .ok_or_else(|| anyhow::anyhow!("Failed to parse server response"))
        })
    }

    fn update_requirement(&self, req: &Requirement) -> Result<Requirement> {
        let mut client_guard = self.client.lock().unwrap();
        let client = client_guard.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected to server"))?;

        self.runtime.block_on(async {
            let request = tonic::Request::new(requirement_to_update_request(req));
            let response = client.update_requirement(request).await
                .context("Failed to update requirement on server")?;
            let updated = response.into_inner().requirement
                .ok_or_else(|| anyhow::anyhow!("Server returned empty requirement"))?;
            proto_to_requirement(&updated)
                .ok_or_else(|| anyhow::anyhow!("Failed to parse server response"))
        })
    }

    fn delete_requirement(&self, id: &str) -> Result<()> {
        let mut client_guard = self.client.lock().unwrap();
        let client = client_guard.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected to server"))?;

        self.runtime.block_on(async {
            let request = tonic::Request::new(proto::DeleteRequirementRequest {
                id: id.to_string(),
            });
            let response = client.delete_requirement(request).await
                .context("Failed to delete requirement on server")?;
            if response.into_inner().success {
                Ok(())
            } else {
                Err(anyhow::anyhow!("Server rejected delete request"))
            }
        })
    }

    fn add_comment(
        &self,
        req_id: &str,
        content: &str,
        author: &str,
        parent_id: Option<&str>,
    ) -> Result<Comment> {
        let mut client_guard = self.client.lock().unwrap();
        let client = client_guard.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected to server"))?;

        self.runtime.block_on(async {
            let request = tonic::Request::new(proto::AddCommentRequest {
                requirement_id: req_id.to_string(),
                content: content.to_string(),
                author: author.to_string(),
                parent_comment_id: parent_id.unwrap_or("").to_string(),
            });
            let response = client.add_comment(request).await
                .context("Failed to add comment on server")?;
            let comment = response.into_inner().comment
                .ok_or_else(|| anyhow::anyhow!("Server returned empty comment"))?;
            proto_to_comment(&comment)
                .ok_or_else(|| anyhow::anyhow!("Failed to parse server response"))
        })
    }

    fn add_relationship(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &RelationshipType,
        created_by: &str,
    ) -> Result<()> {
        let mut client_guard = self.client.lock().unwrap();
        let client = client_guard.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected to server"))?;

        self.runtime.block_on(async {
            let (proto_rel_type, custom_name) = rel_type_to_proto(rel_type);
            let request = tonic::Request::new(proto::AddRelationshipRequest {
                source_id: source_id.to_string(),
                target_id: target_id.to_string(),
                rel_type: proto_rel_type as i32,
                custom_type_name: custom_name,
                created_by: created_by.to_string(),
            });
            let response = client.add_relationship(request).await
                .context("Failed to add relationship on server")?;
            if response.into_inner().success {
                Ok(())
            } else {
                Err(anyhow::anyhow!("Server rejected relationship"))
            }
        })
    }

    fn get_server_status(&self) -> Result<ServerStatus> {
        let mut client_guard = self.client.lock().unwrap();
        let client = client_guard.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected to server"))?;

        self.runtime.block_on(async {
            let request = tonic::Request::new(proto::GetServerStatusRequest {});
            let response = client.get_server_status(request).await
                .context("Failed to get server status")?;
            let status = response.into_inner();
            Ok(ServerStatus {
                version: status.version,
                status: status.status,
                uptime_seconds: status.uptime_seconds,
                active_connections: status.active_connections,
                storage_backend: status.storage_backend,
                storage_path: status.storage_path,
            })
        })
    }
}

// =============================================================================
// WASM implementation (using tonic-web-wasm-client)
// =============================================================================

#[cfg(target_arch = "wasm32")]
use tonic_web_wasm_client::Client as WasmClient;

/// gRPC-based storage client for WASM
///
/// This client connects to an AIDA server via gRPC-Web and implements
/// the StorageClient trait for uniform access in the browser.
#[cfg(target_arch = "wasm32")]
pub struct GrpcStorageClient {
    server_addr: String,
    client: Arc<Mutex<RequirementsServiceClient<WasmClient>>>,
}

#[cfg(target_arch = "wasm32")]
impl GrpcStorageClient {
    /// Connect to a gRPC-Web server
    ///
    /// # Arguments
    /// * `server_addr` - Server URL (e.g., "http://localhost:50051")
    pub fn connect(server_addr: &str) -> Result<Self> {
        let addr = normalize_addr(server_addr);
        let wasm_client = WasmClient::new(addr.clone());
        let client = RequirementsServiceClient::new(wasm_client);

        Ok(GrpcStorageClient {
            server_addr: server_addr.to_string(),
            client: Arc::new(Mutex::new(client)),
        })
    }

    /// Get a mutable reference to the inner client for async operations
    pub fn inner(&self) -> std::sync::MutexGuard<'_, RequirementsServiceClient<WasmClient>> {
        self.client.lock().unwrap()
    }
}

// Note: For WASM, we implement StorageClient with stub methods that return errors
// since the actual async operations should be done via the `inner()` method
// and `wasm_bindgen_futures::spawn_local()` pattern in the application layer.
#[cfg(target_arch = "wasm32")]
impl StorageClient for GrpcStorageClient {
    fn load(&self) -> Result<RequirementsStore> {
        // In WASM, we can't block on async. The caller should use spawn_local()
        // with the inner client directly for async operations.
        Err(anyhow::anyhow!(
            "Direct sync load() not supported in WASM. Use inner() with spawn_local() instead."
        ))
    }

    fn save(&self, _store: &RequirementsStore) -> Result<()> {
        Err(anyhow::anyhow!(
            "Direct sync save() not supported in WASM. Use inner() with spawn_local() instead."
        ))
    }

    fn display_path(&self) -> String {
        self.server_addr.clone()
    }

    fn is_remote(&self) -> bool {
        true
    }

    fn create_requirement(&self, _req: &Requirement) -> Result<Requirement> {
        Err(anyhow::anyhow!(
            "Direct sync create_requirement() not supported in WASM."
        ))
    }

    fn update_requirement(&self, _req: &Requirement) -> Result<Requirement> {
        Err(anyhow::anyhow!(
            "Direct sync update_requirement() not supported in WASM."
        ))
    }

    fn delete_requirement(&self, _id: &str) -> Result<()> {
        Err(anyhow::anyhow!(
            "Direct sync delete_requirement() not supported in WASM."
        ))
    }

    fn add_comment(
        &self,
        _req_id: &str,
        _content: &str,
        _author: &str,
        _parent_id: Option<&str>,
    ) -> Result<Comment> {
        Err(anyhow::anyhow!(
            "Direct sync add_comment() not supported in WASM."
        ))
    }

    fn add_relationship(
        &self,
        _source_id: &str,
        _target_id: &str,
        _rel_type: &RelationshipType,
        _created_by: &str,
    ) -> Result<()> {
        Err(anyhow::anyhow!(
            "Direct sync add_relationship() not supported in WASM."
        ))
    }

    fn get_server_status(&self) -> Result<ServerStatus> {
        Err(anyhow::anyhow!(
            "Direct sync get_server_status() not supported in WASM."
        ))
    }
}

// =============================================================================
// WASM async helper functions for use with spawn_local()
// =============================================================================

#[cfg(target_arch = "wasm32")]
impl GrpcStorageClient {
    /// Async version of load() for WASM
    pub async fn load_async(&self) -> Result<RequirementsStore> {
        let mut client = self.client.lock().unwrap();
        let request = proto::GetStoreRequest {};
        let response = client.get_store(request).await
            .map_err(|e| anyhow::anyhow!("Failed to get store: {}", e))?;
        let proto_store = response.into_inner().store
            .ok_or_else(|| anyhow::anyhow!("Server returned empty store"))?;
        proto_to_store(&proto_store)
    }

    /// Async version of get_server_status() for WASM
    pub async fn get_server_status_async(&self) -> Result<ServerStatus> {
        let mut client = self.client.lock().unwrap();
        let request = proto::GetServerStatusRequest {};
        let response = client.get_server_status(request).await
            .map_err(|e| anyhow::anyhow!("Failed to get server status: {}", e))?;
        let status = response.into_inner();
        Ok(ServerStatus {
            version: status.version,
            status: status.status,
            uptime_seconds: status.uptime_seconds,
            active_connections: status.active_connections,
            storage_backend: status.storage_backend,
            storage_path: status.storage_path,
        })
    }

    /// Async version of create_requirement() for WASM
    pub async fn create_requirement_async(&self, req: &Requirement) -> Result<Requirement> {
        let mut client = self.client.lock().unwrap();
        let request = requirement_to_create_request(req);
        let response = client.create_requirement(request).await
            .map_err(|e| anyhow::anyhow!("Failed to create requirement: {}", e))?;
        let created = response.into_inner().requirement
            .ok_or_else(|| anyhow::anyhow!("Server returned empty requirement"))?;
        proto_to_requirement(&created)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse server response"))
    }

    /// Async version of update_requirement() for WASM
    pub async fn update_requirement_async(&self, req: &Requirement) -> Result<Requirement> {
        let mut client = self.client.lock().unwrap();
        let request = requirement_to_update_request(req);
        let response = client.update_requirement(request).await
            .map_err(|e| anyhow::anyhow!("Failed to update requirement: {}", e))?;
        let updated = response.into_inner().requirement
            .ok_or_else(|| anyhow::anyhow!("Server returned empty requirement"))?;
        proto_to_requirement(&updated)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse server response"))
    }

    /// Async version of delete_requirement() for WASM
    pub async fn delete_requirement_async(&self, id: &str) -> Result<()> {
        let mut client = self.client.lock().unwrap();
        let request = proto::DeleteRequirementRequest {
            id: id.to_string(),
        };
        let response = client.delete_requirement(request).await
            .map_err(|e| anyhow::anyhow!("Failed to delete requirement: {}", e))?;
        if response.into_inner().success {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Server rejected delete request"))
        }
    }

    /// Async version of add_comment() for WASM
    pub async fn add_comment_async(
        &self,
        req_id: &str,
        content: &str,
        author: &str,
        parent_id: Option<&str>,
    ) -> Result<Comment> {
        let mut client = self.client.lock().unwrap();
        let request = proto::AddCommentRequest {
            requirement_id: req_id.to_string(),
            content: content.to_string(),
            author: author.to_string(),
            parent_comment_id: parent_id.unwrap_or("").to_string(),
        };
        let response = client.add_comment(request).await
            .map_err(|e| anyhow::anyhow!("Failed to add comment: {}", e))?;
        let comment = response.into_inner().comment
            .ok_or_else(|| anyhow::anyhow!("Server returned empty comment"))?;
        proto_to_comment(&comment)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse server response"))
    }

    /// Async version of add_relationship() for WASM
    pub async fn add_relationship_async(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &RelationshipType,
        created_by: &str,
    ) -> Result<()> {
        let mut client = self.client.lock().unwrap();
        let (proto_rel_type, custom_name) = rel_type_to_proto(rel_type);
        let request = proto::AddRelationshipRequest {
            source_id: source_id.to_string(),
            target_id: target_id.to_string(),
            rel_type: proto_rel_type as i32,
            custom_type_name: custom_name,
            created_by: created_by.to_string(),
        };
        let response = client.add_relationship(request).await
            .map_err(|e| anyhow::anyhow!("Failed to add relationship: {}", e))?;
        if response.into_inner().success {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Server rejected relationship"))
        }
    }
}

// =============================================================================
// Shared helper functions
// =============================================================================

fn normalize_addr(server_addr: &str) -> String {
    if server_addr.starts_with("http://") || server_addr.starts_with("https://") {
        server_addr.to_string()
    } else if server_addr.starts_with("grpc://") {
        server_addr.replace("grpc://", "http://")
    } else {
        format!("http://{}", server_addr)
    }
}

// =============================================================================
// Proto conversion functions (shared between native and WASM)
// =============================================================================

pub fn proto_to_store(store: &proto::RequirementsStore) -> Result<RequirementsStore> {
    use aida_core::{FeatureDefinition, IdConfiguration, IdFormat, NumberingStrategy, User};

    let requirements: Vec<Requirement> = store.requirements
        .iter()
        .filter_map(proto_to_requirement)
        .collect();

    let users: Vec<User> = store.users
        .iter()
        .map(|u| User {
            id: uuid::Uuid::parse_str(&u.id).unwrap_or_else(|_| uuid::Uuid::new_v4()),
            spec_id: if u.spec_id.is_empty() { None } else { Some(u.spec_id.clone()) },
            name: u.name.clone(),
            email: u.email.clone(),
            handle: u.handle.clone(),
            archived: false,
            created_at: chrono::Utc::now(),
            version: 0,
        })
        .collect();

    let features: Vec<FeatureDefinition> = store.features
        .iter()
        .map(|f| FeatureDefinition {
            number: f.number as u32,
            name: f.name.clone(),
            prefix: f.prefix.clone(),
            description: String::new(),
        })
        .collect();

    let id_config = store.id_config.as_ref().map(|c| {
        IdConfiguration {
            format: match c.format.as_str() {
                "single_level" => IdFormat::SingleLevel,
                "two_level" => IdFormat::TwoLevel,
                _ => IdFormat::SingleLevel,
            },
            numbering: match c.numbering.as_str() {
                "global" => NumberingStrategy::Global,
                "per_prefix" => NumberingStrategy::PerPrefix,
                "per_feature_type" => NumberingStrategy::PerFeatureType,
                _ => NumberingStrategy::Global,
            },
            digits: c.digits as u8,
            requirement_types: Vec::new(),
        }
    }).unwrap_or_default();

    let prefix_counters: std::collections::HashMap<String, u32> = store.prefix_counters
        .iter()
        .map(|(k, v)| (k.clone(), *v as u32))
        .collect();

    Ok(RequirementsStore {
        name: store.name.clone(),
        title: store.title.clone(),
        description: store.description.clone(),
        requirements,
        users,
        teams: Vec::new(),
        features,
        next_feature_number: 100,
        id_config,
        next_spec_number: store.next_spec_number as u32,
        prefix_counters,
        relationship_definitions: aida_core::RelationshipDefinition::defaults(),
        reaction_definitions: Vec::new(),
        meta_counters: std::collections::HashMap::new(),
        type_definitions: Vec::new(),
        allowed_prefixes: Vec::new(),
        restrict_prefixes: false,
        ai_prompts: aida_core::AiPromptConfig::default(),
        baselines: Vec::new(),
        store_version: 0,
        migrated_to: None,
    })
}

pub fn proto_to_requirement(req: &proto::Requirement) -> Option<Requirement> {
    use uuid::Uuid;

    let id = Uuid::parse_str(&req.id).ok()?;
    let status_enum = proto::RequirementStatus::try_from(req.status).unwrap_or(proto::RequirementStatus::Unspecified);
    let priority_enum = proto::RequirementPriority::try_from(req.priority).unwrap_or(proto::RequirementPriority::Unspecified);
    let type_enum = proto::RequirementType::try_from(req.req_type).unwrap_or(proto::RequirementType::Unspecified);

    Some(Requirement {
        id,
        spec_id: if req.spec_id.is_empty() { None } else { Some(req.spec_id.clone()) },
        prefix_override: if req.prefix_override.is_empty() { None } else { Some(req.prefix_override.clone()) },
        title: req.title.clone(),
        description: req.description.clone(),
        status: proto_to_status(status_enum),
        priority: proto_to_priority(priority_enum),
        owner: req.owner.clone(),
        feature: req.feature.clone(),
        created_at: proto_to_datetime(req.created_at.clone()),
        created_by: if req.created_by.is_empty() { None } else { Some(req.created_by.clone()) },
        modified_at: proto_to_datetime(req.modified_at.clone()),
        req_type: proto_to_req_type(type_enum),
        dependencies: req.dependency_ids.iter().filter_map(|id| Uuid::parse_str(id).ok()).collect(),
        tags: req.tags.iter().cloned().collect(),
        weight: None, // Not exposed via proto yet
        relationships: req.relationships.iter().filter_map(proto_to_relationship).collect(),
        comments: req.comments.iter().filter_map(proto_to_comment).collect(),
        history: Vec::new(),
        archived: req.archived,
        custom_status: if req.custom_status.is_empty() { None } else { Some(req.custom_status.clone()) },
        custom_priority: if req.custom_priority.is_empty() { None } else { Some(req.custom_priority.clone()) },
        custom_fields: req.custom_fields.clone(),
        urls: req.urls.iter().map(proto_to_url_link).collect(),
        attachments: Vec::new(), // Not exposed via proto yet
        trace_links: Vec::new(), // Not exposed via proto yet
        implementation_info: None, // Not exposed via proto yet
        ai_evaluation: None,
        version: 0,
    })
}

fn proto_to_datetime(ts: Option<proto::Timestamp>) -> chrono::DateTime<chrono::Utc> {
    use chrono::{TimeZone, Utc};
    match ts {
        Some(t) => Utc
            .timestamp_opt(t.seconds, t.nanos as u32)
            .single()
            .unwrap_or_else(Utc::now),
        None => Utc::now(),
    }
}

fn proto_to_status(status: proto::RequirementStatus) -> aida_core::RequirementStatus {
    use aida_core::RequirementStatus::*;
    match status {
        proto::RequirementStatus::Draft => Draft,
        proto::RequirementStatus::Approved => Approved,
        // Note: Proto may not have Planned/InProgress - handled via custom_status field
        proto::RequirementStatus::Completed => Completed,
        proto::RequirementStatus::Rejected => Rejected,
        _ => Draft,
    }
}

fn proto_to_priority(priority: proto::RequirementPriority) -> aida_core::RequirementPriority {
    use aida_core::RequirementPriority::*;
    match priority {
        proto::RequirementPriority::High => High,
        proto::RequirementPriority::Medium => Medium,
        proto::RequirementPriority::Low => Low,
        _ => Medium,
    }
}

fn proto_to_req_type(req_type: proto::RequirementType) -> aida_core::RequirementType {
    use aida_core::RequirementType::*;
    match req_type {
        proto::RequirementType::Functional => Functional,
        proto::RequirementType::NonFunctional => NonFunctional,
        proto::RequirementType::System => System,
        proto::RequirementType::User => User,
        proto::RequirementType::ChangeRequest => ChangeRequest,
        proto::RequirementType::Bug => Bug,
        proto::RequirementType::Epic => Epic,
        proto::RequirementType::Story => Story,
        proto::RequirementType::Task => Task,
        proto::RequirementType::Spike => Spike,
        proto::RequirementType::Sprint => Sprint,
        proto::RequirementType::Folder => Folder,
        _ => Functional,
    }
}

fn proto_to_relationship(rel: &proto::Relationship) -> Option<aida_core::Relationship> {
    use uuid::Uuid;

    let target_id = Uuid::parse_str(&rel.target_id).ok()?;
    let rel_type_enum = proto::RelationshipType::try_from(rel.rel_type).unwrap_or(proto::RelationshipType::Unspecified);
    Some(aida_core::Relationship {
        target_id,
        rel_type: proto_to_rel_type(rel_type_enum, &rel.custom_type_name),
        created_at: rel.created_at.clone().map(|t| proto_to_datetime(Some(t))),
        created_by: if rel.created_by.is_empty() { None } else { Some(rel.created_by.clone()) },
    })
}

fn proto_to_rel_type(rel_type: proto::RelationshipType, custom_name: &str) -> RelationshipType {
    use RelationshipType::*;
    match rel_type {
        proto::RelationshipType::Parent => Parent,
        proto::RelationshipType::Child => Child,
        proto::RelationshipType::Duplicate => Duplicate,
        proto::RelationshipType::Verifies => Verifies,
        proto::RelationshipType::VerifiedBy => VerifiedBy,
        proto::RelationshipType::References => References,
        proto::RelationshipType::Custom => Custom(custom_name.to_string()),
        _ => References,
    }
}

pub fn proto_to_comment(comment: &proto::Comment) -> Option<Comment> {
    use uuid::Uuid;

    let id = Uuid::parse_str(&comment.id).ok()?;
    Some(Comment {
        id,
        content: comment.content.clone(),
        author: comment.author.clone(),
        created_at: proto_to_datetime(comment.created_at.clone()),
        modified_at: proto_to_datetime(comment.modified_at.clone()),
        parent_id: if comment.parent_id.is_empty() { None } else { Uuid::parse_str(&comment.parent_id).ok() },
        replies: Vec::new(),
        reactions: comment.reactions.iter().filter_map(proto_to_reaction).collect(),
    })
}

fn proto_to_reaction(reaction: &proto::CommentReaction) -> Option<aida_core::CommentReaction> {
    Some(aida_core::CommentReaction {
        reaction: reaction.reaction.clone(),
        author: reaction.author.clone(),
        added_at: proto_to_datetime(reaction.added_at.clone()),
    })
}

fn proto_to_url_link(link: &proto::UrlLink) -> aida_core::UrlLink {
    use uuid::Uuid;

    aida_core::UrlLink {
        id: Uuid::parse_str(&link.id).unwrap_or_else(|_| Uuid::new_v4()),
        url: link.url.clone(),
        title: link.title.clone(),
        description: if link.description.is_empty() { None } else { Some(link.description.clone()) },
        added_at: proto_to_datetime(link.added_at.clone()),
        added_by: link.added_by.clone(),
        last_verified: None,
        last_verified_ok: None,
    }
}

// =============================================================================
// Core to Proto conversion functions (for save operations)
// =============================================================================

pub fn requirement_to_create_request(req: &Requirement) -> proto::CreateRequirementRequest {
    proto::CreateRequirementRequest {
        title: req.title.clone(),
        description: req.description.clone(),
        status: status_to_proto(&req.status) as i32,
        priority: priority_to_proto(&req.priority) as i32,
        owner: req.owner.clone(),
        feature: req.feature.clone(),
        req_type: req_type_to_proto(&req.req_type) as i32,
        tags: req.tags.iter().cloned().collect(),
        prefix_override: req.prefix_override.clone().unwrap_or_default(),
        created_by: req.created_by.clone().unwrap_or_default(),
    }
}

pub fn requirement_to_update_request(req: &Requirement) -> proto::UpdateRequirementRequest {
    proto::UpdateRequirementRequest {
        id: req.id.to_string(),
        title: Some(req.title.clone()),
        description: Some(req.description.clone()),
        status: Some(status_to_proto(&req.status) as i32),
        priority: Some(priority_to_proto(&req.priority) as i32),
        owner: Some(req.owner.clone()),
        feature: Some(req.feature.clone()),
        req_type: Some(req_type_to_proto(&req.req_type) as i32),
        tags: req.tags.iter().cloned().collect(),
        replace_tags: true,
        archived: Some(req.archived),
        custom_status: req.custom_status.clone(),
        custom_priority: req.custom_priority.clone(),
        custom_fields: req.custom_fields.clone(),
        replace_custom_fields: true,
        modified_by: String::new(),
    }
}

fn status_to_proto(status: &aida_core::RequirementStatus) -> proto::RequirementStatus {
    use aida_core::RequirementStatus::*;
    match status {
        Draft => proto::RequirementStatus::Draft,
        Approved => proto::RequirementStatus::Approved,
        // Note: Proto may not have Planned/InProgress - map to Approved for now
        // These statuses should be handled via custom_status field
        Planned => proto::RequirementStatus::Approved,
        InProgress => proto::RequirementStatus::Approved,
        Completed => proto::RequirementStatus::Completed,
        Rejected => proto::RequirementStatus::Rejected,
    }
}

fn priority_to_proto(priority: &aida_core::RequirementPriority) -> proto::RequirementPriority {
    use aida_core::RequirementPriority::*;
    match priority {
        High => proto::RequirementPriority::High,
        Medium => proto::RequirementPriority::Medium,
        Low => proto::RequirementPriority::Low,
    }
}

fn req_type_to_proto(req_type: &aida_core::RequirementType) -> proto::RequirementType {
    use aida_core::RequirementType::*;
    match req_type {
        Functional => proto::RequirementType::Functional,
        NonFunctional => proto::RequirementType::NonFunctional,
        System => proto::RequirementType::System,
        User => proto::RequirementType::User,
        ChangeRequest => proto::RequirementType::ChangeRequest,
        Bug => proto::RequirementType::Bug,
        Epic => proto::RequirementType::Epic,
        Story => proto::RequirementType::Story,
        Task => proto::RequirementType::Task,
        Spike => proto::RequirementType::Spike,
        Sprint => proto::RequirementType::Sprint,
        Folder => proto::RequirementType::Folder,
    }
}

fn rel_type_to_proto(rel_type: &RelationshipType) -> (proto::RelationshipType, String) {
    use RelationshipType::*;
    match rel_type {
        Parent => (proto::RelationshipType::Parent, String::new()),
        Child => (proto::RelationshipType::Child, String::new()),
        Duplicate => (proto::RelationshipType::Duplicate, String::new()),
        Verifies => (proto::RelationshipType::Verifies, String::new()),
        VerifiedBy => (proto::RelationshipType::VerifiedBy, String::new()),
        References => (proto::RelationshipType::References, String::new()),
        Custom(name) => (proto::RelationshipType::Custom, name.clone()),
    }
}
