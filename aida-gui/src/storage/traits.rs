// trace:FR-0278 | ai:claude:high
//! Storage client trait definitions
//!
//! This module defines the core trait that all storage backends must implement,
//! providing a unified interface for both local and remote storage access.

use anyhow::Result;
use aida_core::{Comment, RelationshipType, Requirement, RequirementsStore};

/// Error type for storage operations
#[derive(Debug)]
pub enum StorageError {
    /// Connection to server failed
    ConnectionFailed(String),
    /// Server returned an error
    ServerError(String),
    /// Request timed out
    Timeout,
    /// Invalid response from server
    InvalidResponse(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::ConnectionFailed(msg) => write!(f, "Connection failed: {}", msg),
            StorageError::ServerError(msg) => write!(f, "Server error: {}", msg),
            StorageError::Timeout => write!(f, "Request timed out"),
            StorageError::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
        }
    }
}

impl std::error::Error for StorageError {}

/// Server status information
#[derive(Clone, Debug)]
pub struct ServerStatus {
    /// Server version
    pub version: String,
    /// Server status (e.g., "running", "healthy")
    pub status: String,
    /// Uptime in seconds
    pub uptime_seconds: i64,
    /// Number of active connections
    pub active_connections: i32,
    /// Storage backend type (e.g., "sqlite", "yaml")
    pub storage_backend: String,
    /// Storage path on server
    pub storage_path: String,
}

/// Unified storage client trait for both local and remote backends
///
/// This trait provides a consistent interface for storage operations,
/// regardless of whether the backend is local (embedded server) or remote.
///
/// All methods are synchronous from the caller's perspective, even though
/// the underlying implementation may use async gRPC calls.
pub trait StorageClient: Send + Sync {
    /// Load the entire requirements store from the backend
    ///
    /// # Returns
    /// The complete requirements store, or an error if loading failed
    fn load(&self) -> Result<RequirementsStore>;

    /// Save the entire requirements store to the backend
    ///
    /// This performs a full sync, updating, creating, and deleting
    /// requirements as needed to match the provided store.
    ///
    /// # Arguments
    /// * `store` - The requirements store to save
    fn save(&self, store: &RequirementsStore) -> Result<()>;

    /// Get a display-friendly path or address for this storage
    ///
    /// For local storage, this is the file path.
    /// For remote storage, this is the server address.
    fn display_path(&self) -> String;

    /// Check if this is a remote connection
    ///
    /// Returns true for connections to remote servers,
    /// false for embedded local servers.
    fn is_remote(&self) -> bool;

    /// Create a new requirement on the backend
    ///
    /// # Arguments
    /// * `req` - The requirement to create (ID should be set)
    ///
    /// # Returns
    /// The created requirement with any server-assigned values
    fn create_requirement(&self, req: &Requirement) -> Result<Requirement>;

    /// Update an existing requirement on the backend
    ///
    /// # Arguments
    /// * `req` - The requirement with updated values
    ///
    /// # Returns
    /// The updated requirement
    fn update_requirement(&self, req: &Requirement) -> Result<Requirement>;

    /// Delete a requirement by its SPEC-ID
    ///
    /// # Arguments
    /// * `id` - The SPEC-ID of the requirement to delete
    fn delete_requirement(&self, id: &str) -> Result<()>;

    /// Add a comment to a requirement
    ///
    /// # Arguments
    /// * `req_id` - The SPEC-ID of the requirement
    /// * `content` - The comment text
    /// * `author` - The comment author
    /// * `parent_id` - Optional parent comment ID for threaded comments
    ///
    /// # Returns
    /// The created comment with server-assigned ID and timestamp
    fn add_comment(
        &self,
        req_id: &str,
        content: &str,
        author: &str,
        parent_id: Option<&str>,
    ) -> Result<Comment>;

    /// Add a relationship between two requirements
    ///
    /// # Arguments
    /// * `source_id` - The SPEC-ID of the source requirement
    /// * `target_id` - The SPEC-ID of the target requirement
    /// * `rel_type` - The type of relationship
    /// * `created_by` - The user creating the relationship
    fn add_relationship(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &RelationshipType,
        created_by: &str,
    ) -> Result<()>;

    /// Get the server status
    ///
    /// # Returns
    /// Current server status information
    fn get_server_status(&self) -> Result<ServerStatus>;
}
