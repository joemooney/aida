// trace:FR-0273 | ai:claude:high
//! Shared UI components for AIDA GUI
//!
//! This module provides reusable UI rendering functions that work with proto types
//! and can be shared between native (aida-desktop) and web (aida-web) builds.
//!
//! All functions are pure rendering functions that take egui::Ui and proto types,
//! making them suitable for both platforms.

mod badges;
mod comment_list;
mod detail_view;
mod formatters;
mod list_item;
mod requirement_form;

pub use badges::*;
pub use comment_list::*;
pub use detail_view::*;
pub use formatters::*;
pub use list_item::*;
pub use requirement_form::*;

// Re-export proto types for convenience
pub use crate::storage::proto;
