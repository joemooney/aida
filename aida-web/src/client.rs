// trace:FR-0273 | ai:claude:high
//! gRPC-Web client for communicating with the AIDA server
//!
//! This module wraps the shared GrpcStorageClient from aida-gui for use
//! in the web application. It provides a simplified async interface
//! for the WASM environment.

use tonic_web_wasm_client::Client;

use crate::proto::requirements_service_client::RequirementsServiceClient;
use crate::proto::*;

// Re-export shared storage types from aida-gui
pub use aida_gui::storage::{GrpcStorageClient, proto as shared_proto};

/// AIDA gRPC-Web client wrapper
pub struct AidaClient {
    client: RequirementsServiceClient<Client>,
}

impl AidaClient {
    /// Create a new client connected to the given server URL
    pub fn new(server_url: &str) -> Self {
        log::info!("Connecting to AIDA server at: {}", server_url);
        let client = Client::new(server_url.to_string());
        Self {
            client: RequirementsServiceClient::new(client),
        }
    }

    /// Create a new client using the shared GrpcStorageClient from aida-gui
    /// This provides access to the shared proto conversion functions
    pub fn new_shared(server_url: &str) -> Result<GrpcStorageClient, anyhow::Error> {
        GrpcStorageClient::connect(server_url)
    }

    /// Get the full requirements store
    pub async fn get_store(&mut self) -> Result<RequirementsStore, String> {
        log::debug!("Fetching requirements store...");
        let request = GetStoreRequest {};
        self.client
            .get_store(request)
            .await
            .map(|r| r.into_inner().store.unwrap_or_default())
            .map_err(|e| format!("Failed to get store: {}", e))
    }

    /// Get server status (ping)
    pub async fn get_status(&mut self) -> Result<GetServerStatusResponse, String> {
        log::debug!("Pinging server...");
        let request = GetServerStatusRequest {};
        self.client
            .get_server_status(request)
            .await
            .map(|r| r.into_inner())
            .map_err(|e| format!("Failed to get status: {}", e))
    }

    /// List requirements with optional filters
    pub async fn list_requirements(
        &mut self,
        filter: ListRequirementsRequest,
    ) -> Result<ListRequirementsResponse, String> {
        log::debug!("Listing requirements...");
        self.client
            .list_requirements(filter)
            .await
            .map(|r| r.into_inner())
            .map_err(|e| format!("Failed to list requirements: {}", e))
    }

    /// Get a single requirement by ID
    pub async fn get_requirement(&mut self, id: &str) -> Result<Requirement, String> {
        log::debug!("Getting requirement: {}", id);
        let request = GetRequirementRequest { id: id.to_string() };
        self.client
            .get_requirement(request)
            .await
            .map(|r| r.into_inner().requirement.unwrap_or_default())
            .map_err(|e| format!("Failed to get requirement: {}", e))
    }

    /// Create a new requirement
    pub async fn create_requirement(
        &mut self,
        title: String,
        description: String,
        status: RequirementStatus,
        priority: RequirementPriority,
        req_type: RequirementType,
        created_by: String,
    ) -> Result<CreateRequirementResponse, String> {
        log::debug!("Creating requirement: {}", title);
        let request = CreateRequirementRequest {
            title,
            description,
            status: status.into(),
            priority: priority.into(),
            req_type: req_type.into(),
            created_by,
            owner: String::new(),
            feature: String::new(),
            tags: vec![],
            prefix_override: String::new(),
        };
        self.client
            .create_requirement(request)
            .await
            .map(|r| r.into_inner())
            .map_err(|e| format!("Failed to create requirement: {}", e))
    }

    /// Update an existing requirement
    pub async fn update_requirement(
        &mut self,
        request: UpdateRequirementRequest,
    ) -> Result<Requirement, String> {
        log::debug!("Updating requirement: {}", request.id);
        self.client
            .update_requirement(request)
            .await
            .map(|r| r.into_inner().requirement.unwrap_or_default())
            .map_err(|e| format!("Failed to update requirement: {}", e))
    }

    /// Search requirements
    pub async fn search(
        &mut self,
        query: String,
        search_title: bool,
        search_description: bool,
        limit: i32,
    ) -> Result<Vec<Requirement>, String> {
        log::debug!("Searching for: {}", query);
        let request = SearchRequirementsRequest {
            query,
            search_title,
            search_description,
            search_comments: true,
            search_spec_id: true,
            status_filter: String::new(),
            type_filter: String::new(),
            feature_filter: String::new(),
            include_archived: false,
            limit,
        };
        self.client
            .search_requirements(request)
            .await
            .map(|r| r.into_inner().requirements)
            .map_err(|e| format!("Failed to search: {}", e))
    }

    /// Add a comment to a requirement
    pub async fn add_comment(
        &mut self,
        requirement_id: &str,
        content: String,
        author: String,
    ) -> Result<Comment, String> {
        log::debug!("Adding comment to: {}", requirement_id);
        let request = AddCommentRequest {
            requirement_id: requirement_id.to_string(),
            content,
            author,
            parent_comment_id: String::new(),
        };
        self.client
            .add_comment(request)
            .await
            .map(|r| r.into_inner().comment.unwrap_or_default())
            .map_err(|e| format!("Failed to add comment: {}", e))
    }
}
