// trace:FR-0273 | ai:claude:high
//! AIDA Web - WASM browser client for requirements management
//!
//! This crate provides a WebAssembly-based browser client that connects
//! to the AIDA server via gRPC-Web protocol.
//!
//! ## Architecture
//! This crate uses two sets of proto types:
//! - `proto` - Re-exported from `aida_desktop::storage::proto` for UI component compatibility
//! - `generated` - Web-specific generated proto with gRPC-Web client and auth types

pub mod app;
pub mod client;

/// Generated proto types with gRPC-Web client support
pub mod generated {
    pub mod aida {
        include!("generated/aida.rs");
    }
}

/// Re-export proto types from aida-desktop for type compatibility with shared UI components
pub use aida_desktop::storage::proto;
