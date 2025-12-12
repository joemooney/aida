// trace:FR-0273 | ai:claude:high
//! AIDA Web - WASM browser client for requirements management
//!
//! This crate provides a WebAssembly-based browser client that connects
//! to the AIDA server via gRPC-Web protocol.

pub mod app;
pub mod client;

/// Generated protobuf types and gRPC client
pub mod proto {
    include!("generated/aida.rs");
}
