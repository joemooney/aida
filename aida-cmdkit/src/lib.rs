//! Shared command framework, independent of the concrete CLI registry.
// trace:TASK-3783 | ai:codex
mod cmdkit;
pub use cmdkit::*;
pub mod spec_arg;
pub mod stakeholder_role;
