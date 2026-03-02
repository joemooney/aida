// trace:FR-0278 | ai:claude:high
//! Unified Storage Abstraction for AIDA GUI
//!
//! This module provides a trait-based abstraction for storage operations,
//! enabling the GUI to work with both local and remote backends through
//! a unified gRPC-based interface.
//!
//! ## Architecture
//!
//! All storage access goes through gRPC:
//! - **Native Desktop**: Connects to embedded server (localhost) or remote server
//! - **Web Browser**: Connects to remote server via gRPC-Web
//!
//! This approach eliminates conditional compilation in business logic and
//! ensures consistent behavior across platforms.

#[cfg(feature = "native")]
mod embedded;
mod grpc_client;
mod traits;

#[cfg(feature = "native")]
pub use embedded::EmbeddedServer;
pub use grpc_client::GrpcStorageClient;
#[allow(unused_imports)]
pub use traits::{ServerStatus, StorageClient, StorageError};

// Re-export proto module for use by aida-web
pub use grpc_client::proto;

// Re-export conversion functions for external use
pub use grpc_client::{
    proto_to_comment, proto_to_requirement, proto_to_store, requirement_to_create_request,
    requirement_to_update_request,
};

use anyhow::Result;

/// Create a storage client for the given configuration
///
/// # Arguments
/// * `server_addr` - Optional server address. If None on native, starts embedded server.
/// * `db_path` - Optional database path for embedded server (native only).
///
/// # Returns
/// A boxed StorageClient implementation
pub fn create_storage_client(
    server_addr: Option<&str>,
    #[allow(unused_variables)] db_path: Option<std::path::PathBuf>,
) -> Result<Box<dyn StorageClient>> {
    match server_addr {
        // Connect to specified remote server
        Some(addr) => {
            let client = GrpcStorageClient::connect(addr)?;
            Ok(Box::new(client))
        }
        // No server specified
        None => {
            #[cfg(feature = "native")]
            {
                // Start embedded server for local storage
                let path = db_path.unwrap_or_else(|| {
                    // Default to requirements.yaml in current directory
                    std::path::PathBuf::from("requirements.yaml")
                });
                let embedded = EmbeddedServer::start(path)?;
                let client = GrpcStorageClient::connect(&embedded.address())?;
                // Store embedded server handle to keep it alive
                Ok(Box::new(EmbeddedStorageClient {
                    client,
                    _server: embedded,
                }))
            }
            #[cfg(not(feature = "native"))]
            {
                anyhow::bail!(
                    "No server address specified. In web mode, a server address is required."
                )
            }
        }
    }
}

/// Storage client that wraps a gRPC client and keeps embedded server alive
#[cfg(feature = "native")]
struct EmbeddedStorageClient {
    client: GrpcStorageClient,
    _server: EmbeddedServer, // Keep server alive as long as client exists
}

#[cfg(feature = "native")]
impl StorageClient for EmbeddedStorageClient {
    fn load(&self) -> Result<aida_core::RequirementsStore> {
        self.client.load()
    }

    fn save(&self, store: &aida_core::RequirementsStore) -> Result<()> {
        self.client.save(store)
    }

    fn display_path(&self) -> String {
        self.client.display_path()
    }

    fn is_remote(&self) -> bool {
        false // Embedded server is local
    }

    fn create_requirement(&self, req: &aida_core::Requirement) -> Result<aida_core::Requirement> {
        self.client.create_requirement(req)
    }

    fn update_requirement(&self, req: &aida_core::Requirement) -> Result<aida_core::Requirement> {
        self.client.update_requirement(req)
    }

    fn delete_requirement(&self, id: &str) -> Result<()> {
        self.client.delete_requirement(id)
    }

    fn add_comment(
        &self,
        req_id: &str,
        content: &str,
        author: &str,
        parent_id: Option<&str>,
    ) -> Result<aida_core::Comment> {
        self.client.add_comment(req_id, content, author, parent_id)
    }

    fn add_relationship(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &aida_core::RelationshipType,
        created_by: &str,
    ) -> Result<()> {
        self.client
            .add_relationship(source_id, target_id, rel_type, created_by)
    }

    fn get_server_status(&self) -> Result<ServerStatus> {
        self.client.get_server_status()
    }
}
