// trace:FR-0227 | ai:claude:high
//! Remote gRPC client for AIDA GUI
//!
//! This module provides a client for connecting to an AIDA gRPC server.
//! It wraps the async gRPC client in a synchronous interface suitable for egui.

#[cfg(feature = "remote")]
pub mod proto {
    include!("generated/aida.rs");
}

#[cfg(feature = "remote")]
use proto::requirements_service_client::RequirementsServiceClient;

#[cfg(feature = "remote")]
use tonic::transport::Channel;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aida_core::{RequirementsStore, Storage as LocalStorage};

/// Storage backend abstraction - can be local file or remote gRPC server
pub enum StorageBackend {
    /// Local file-based storage
    Local(LocalStorage),
    /// Remote gRPC server (only available with "remote" feature)
    #[cfg(feature = "remote")]
    Remote(RemoteStorage),
}

impl StorageBackend {
    /// Create a new local storage backend
    pub fn new_local(path: PathBuf) -> Self {
        StorageBackend::Local(LocalStorage::new(path))
    }

    /// Create a new remote storage backend
    #[cfg(feature = "remote")]
    pub fn new_remote(server_addr: &str) -> Result<Self> {
        Ok(StorageBackend::Remote(RemoteStorage::new(server_addr)?))
    }

    /// Get the path (for local) or server address (for remote)
    pub fn path(&self) -> PathBuf {
        match self {
            StorageBackend::Local(storage) => storage.path().to_path_buf(),
            #[cfg(feature = "remote")]
            StorageBackend::Remote(remote) => PathBuf::from(&remote.server_addr),
        }
    }

    /// Check if this is a remote backend
    #[allow(dead_code)]
    pub fn is_remote(&self) -> bool {
        match self {
            StorageBackend::Local(_) => false,
            #[cfg(feature = "remote")]
            StorageBackend::Remote(_) => true,
        }
    }

    /// Load the requirements store
    pub fn load(&self) -> Result<RequirementsStore> {
        match self {
            StorageBackend::Local(storage) => storage.load(),
            #[cfg(feature = "remote")]
            StorageBackend::Remote(remote) => remote.load(),
        }
    }

    /// Save the requirements store
    pub fn save(&self, store: &RequirementsStore) -> Result<()> {
        match self {
            StorageBackend::Local(storage) => storage.save(store),
            #[cfg(feature = "remote")]
            StorageBackend::Remote(remote) => remote.save(store),
        }
    }
}

/// Server status information
#[cfg(feature = "remote")]
#[derive(Clone, Debug)]
pub struct ServerStatus {
    pub version: String,
    pub status: String,
    pub uptime_seconds: i64,
    pub active_connections: i32,
    pub storage_backend: String,
    pub storage_path: String,
}

/// Remote storage client (wraps async gRPC client)
#[cfg(feature = "remote")]
pub struct RemoteStorage {
    server_addr: String,
    runtime: tokio::runtime::Runtime,
    client: Arc<Mutex<Option<RequirementsServiceClient<Channel>>>>,
}

#[cfg(feature = "remote")]
impl RemoteStorage {
    /// Create a new remote storage client
    pub fn new(server_addr: &str) -> Result<Self> {
        let runtime = tokio::runtime::Runtime::new()
            .context("Failed to create tokio runtime")?;

        let addr = normalize_addr(server_addr);

        // Try to connect
        let client = runtime.block_on(async {
            RequirementsServiceClient::connect(addr).await
        }).context("Failed to connect to AIDA server")?;

        Ok(RemoteStorage {
            server_addr: server_addr.to_string(),
            runtime,
            client: Arc::new(Mutex::new(Some(client))),
        })
    }

    /// Load the requirements store from the server
    pub fn load(&self) -> Result<RequirementsStore> {
        let mut client_guard = self.client.lock().unwrap();
        let client = client_guard.as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected to server"))?;

        let store = self.runtime.block_on(async {
            let request = tonic::Request::new(proto::GetStoreRequest {});
            let response = client.get_store(request).await
                .context("Failed to get store from server")?;
            let proto_store = response.into_inner().store
                .ok_or_else(|| anyhow::anyhow!("Server returned empty store"))?;

            // Convert proto store to RequirementsStore
            proto_to_store(&proto_store)
        })?;

        Ok(store)
    }

    /// Save the requirements store to the server
    /// Note: This is a simplified implementation - full save would need bulk updates
    pub fn save(&self, _store: &RequirementsStore) -> Result<()> {
        // For now, we just log that save was requested
        // A full implementation would iterate through changes and send updates
        eprintln!("WARNING: Remote save not fully implemented - changes may not persist");
        Ok(())
    }

    /// Get server status
    #[allow(dead_code)]
    pub fn get_server_status(&self) -> Result<ServerStatus> {
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

#[cfg(feature = "remote")]
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
// Proto conversion functions
// =============================================================================

#[cfg(feature = "remote")]
fn proto_to_store(store: &proto::RequirementsStore) -> Result<RequirementsStore> {
    use aida_core::{FeatureDefinition, IdConfiguration, IdFormat, NumberingStrategy, User};

    let requirements: Vec<aida_core::Requirement> = store.requirements
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
    })
}

#[cfg(feature = "remote")]
fn proto_to_requirement(req: &proto::Requirement) -> Option<aida_core::Requirement> {
    use aida_core::{RequirementPriority, RequirementStatus, RequirementType};
    use uuid::Uuid;

    let id = Uuid::parse_str(&req.id).ok()?;
    let status_enum = proto::RequirementStatus::try_from(req.status).unwrap_or(proto::RequirementStatus::Unspecified);
    let priority_enum = proto::RequirementPriority::try_from(req.priority).unwrap_or(proto::RequirementPriority::Unspecified);
    let type_enum = proto::RequirementType::try_from(req.req_type).unwrap_or(proto::RequirementType::Unspecified);

    Some(aida_core::Requirement {
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
        relationships: req.relationships.iter().filter_map(proto_to_relationship).collect(),
        comments: req.comments.iter().filter_map(proto_to_comment).collect(),
        history: Vec::new(),
        archived: req.archived,
        custom_status: if req.custom_status.is_empty() { None } else { Some(req.custom_status.clone()) },
        custom_priority: if req.custom_priority.is_empty() { None } else { Some(req.custom_priority.clone()) },
        custom_fields: req.custom_fields.clone(),
        urls: req.urls.iter().map(proto_to_url_link).collect(),
        ai_evaluation: None,
    })
}

#[cfg(feature = "remote")]
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

#[cfg(feature = "remote")]
fn proto_to_status(status: proto::RequirementStatus) -> aida_core::RequirementStatus {
    use aida_core::RequirementStatus::*;
    match status {
        proto::RequirementStatus::Draft => Draft,
        proto::RequirementStatus::Approved => Approved,
        proto::RequirementStatus::Completed => Completed,
        proto::RequirementStatus::Rejected => Rejected,
        proto::RequirementStatus::Unspecified => Draft,
    }
}

#[cfg(feature = "remote")]
fn proto_to_priority(priority: proto::RequirementPriority) -> aida_core::RequirementPriority {
    use aida_core::RequirementPriority::*;
    match priority {
        proto::RequirementPriority::High => High,
        proto::RequirementPriority::Medium => Medium,
        proto::RequirementPriority::Low => Low,
        proto::RequirementPriority::Unspecified => Medium,
    }
}

#[cfg(feature = "remote")]
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
        proto::RequirementType::Unspecified => Functional,
    }
}

#[cfg(feature = "remote")]
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

#[cfg(feature = "remote")]
fn proto_to_rel_type(rel_type: proto::RelationshipType, custom_name: &str) -> aida_core::RelationshipType {
    use aida_core::RelationshipType::*;
    match rel_type {
        proto::RelationshipType::Parent => Parent,
        proto::RelationshipType::Child => Child,
        proto::RelationshipType::Duplicate => Duplicate,
        proto::RelationshipType::Verifies => Verifies,
        proto::RelationshipType::VerifiedBy => VerifiedBy,
        proto::RelationshipType::References => References,
        proto::RelationshipType::Custom => Custom(custom_name.to_string()),
        proto::RelationshipType::Unspecified => References,
    }
}

#[cfg(feature = "remote")]
fn proto_to_comment(comment: &proto::Comment) -> Option<aida_core::Comment> {
    use uuid::Uuid;

    let id = Uuid::parse_str(&comment.id).ok()?;
    Some(aida_core::Comment {
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

#[cfg(feature = "remote")]
fn proto_to_reaction(reaction: &proto::CommentReaction) -> Option<aida_core::CommentReaction> {
    Some(aida_core::CommentReaction {
        reaction: reaction.reaction.clone(),
        author: reaction.author.clone(),
        added_at: proto_to_datetime(reaction.added_at.clone()),
    })
}

#[cfg(feature = "remote")]
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
