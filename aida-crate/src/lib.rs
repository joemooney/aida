//! # AIDA — AI-Native Requirements Management
//!
//! Track what you build and why, with structured context for AI coding agents.
//!
//! ## Install the CLI
//!
//! ```bash
//! cargo install aida-cli
//! ```
//!
//! ## Library Usage
//!
//! This crate re-exports `aida-core`. For most users, install the CLI instead.
//!
//! ```rust,no_run
//! use aida::models::{Requirement, RequirementsStore};
//! ```
//!
//! ## Links
//!
//! - [GitHub](https://github.com/joemooney/aida)
//! - [Getting Started](https://github.com/joemooney/aida/blob/main/docs/getting-started.md)
//! - [CLI (aida-cli)](https://crates.io/crates/aida-cli)
//! - [Core Library (aida-core)](https://crates.io/crates/aida-core)

pub use aida_core::*;
