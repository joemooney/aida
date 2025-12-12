// trace:FR-0273 | ai:claude:high
//! AIDA Web - WASM browser client for requirements management
//!
//! This crate provides a WebAssembly-based browser client that connects
//! to the AIDA server via gRPC-Web protocol.
//!
//! ## Architecture
//! This crate uses shared proto types and UI components from `aida-gui`:
//! - `proto` - Re-exported from `aida_gui::storage::proto` for type compatibility
//! - `ui` - Shared UI components for consistent rendering

pub mod app;
pub mod client;

/// Re-export proto types from aida-gui for type compatibility with shared UI components
pub use aida_gui::storage::proto;
