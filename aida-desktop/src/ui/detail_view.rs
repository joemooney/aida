// trace:FR-0273 | ai:claude:high
//! Requirement detail view rendering
//!
//! Components for displaying full requirement details.

use egui::{Color32, RichText, ScrollArea, Ui};
use crate::storage::proto::Requirement;
use super::badges::{status_badge, priority_badge, type_badge};
use super::comment_list::{comment_list, comment_input, CommentInputConfig};
use super::formatters::format_timestamp;

/// Actions that can be triggered from the detail view
#[derive(Debug, Clone, PartialEq)]
pub enum DetailViewAction {
    /// Edit button was clicked
    Edit,
    /// Delete button was clicked
    Delete,
    /// A new comment should be added
    AddComment(String),
    /// Copy spec_id to clipboard
    CopySpecId,
    /// Open in external viewer (if applicable)
    OpenExternal,
}

/// Configuration for the detail view
#[derive(Clone)]
pub struct DetailViewConfig {
    /// Show edit button
    pub show_edit: bool,
    /// Show delete button
    pub show_delete: bool,
    /// Show comments section
    pub show_comments: bool,
    /// Show metadata section (collapsed by default)
    pub show_metadata: bool,
    /// Show relationships section
    pub show_relationships: bool,
    /// Default open state for metadata
    pub metadata_default_open: bool,
    /// Default open state for comments
    pub comments_default_open: bool,
}

impl Default for DetailViewConfig {
    fn default() -> Self {
        Self {
            show_edit: true,
            show_delete: false,
            show_comments: true,
            show_metadata: true,
            show_relationships: true,
            metadata_default_open: false,
            comments_default_open: true,
        }
    }
}

impl DetailViewConfig {
    /// Read-only view (no edit/delete buttons)
    pub fn read_only() -> Self {
        Self {
            show_edit: false,
            show_delete: false,
            ..Default::default()
        }
    }

    /// Full view with all actions
    pub fn full() -> Self {
        Self {
            show_edit: true,
            show_delete: true,
            ..Default::default()
        }
    }

    /// Compact view (no metadata, no relationships)
    pub fn compact() -> Self {
        Self {
            show_metadata: false,
            show_relationships: false,
            ..Default::default()
        }
    }
}

/// Render the requirement detail view
///
/// Returns any action triggered by the user
pub fn requirement_detail_view(
    ui: &mut Ui,
    req: &Requirement,
    comment_input_text: &mut String,
    config: &DetailViewConfig,
) -> Option<DetailViewAction> {
    let mut action = None;

    ScrollArea::vertical().show(ui, |ui| {
        // Header with spec_id and action buttons
        ui.horizontal(|ui| {
            ui.heading(RichText::new(&req.spec_id).monospace());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if config.show_delete {
                    if ui.button("🗑 Delete").clicked() {
                        action = Some(DetailViewAction::Delete);
                    }
                }
                if config.show_edit {
                    if ui.button("✏️ Edit").clicked() {
                        action = Some(DetailViewAction::Edit);
                    }
                }
            });
        });

        ui.separator();

        // Status badges
        ui.horizontal(|ui| {
            status_badge(ui, req.status);
            priority_badge(ui, req.priority);
            type_badge(ui, req.req_type);
        });

        ui.add_space(8.0);

        // Title
        ui.heading(&req.title);

        ui.add_space(8.0);

        // Description
        if !req.description.is_empty() {
            ui.label(&req.description);
        } else {
            ui.label(RichText::new("No description").weak().italics());
        }

        // Tags
        if !req.tags.is_empty() {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                for tag in &req.tags {
                    ui.label(
                        RichText::new(format!("#{}", tag))
                            .color(Color32::from_rgb(100, 150, 200))
                    );
                }
            });
        }

        ui.add_space(16.0);
        ui.separator();

        // Metadata section
        if config.show_metadata {
            egui::CollapsingHeader::new("Details")
                .default_open(config.metadata_default_open)
                .show(ui, |ui| {
                    metadata_grid(ui, req);
                });
        }

        // Relationships section
        if config.show_relationships && !req.relationships.is_empty() {
            egui::CollapsingHeader::new(format!("Relationships ({})", req.relationships.len()))
                .default_open(false)
                .show(ui, |ui| {
                    relationships_view(ui, req);
                });
        }

        // Comments section
        if config.show_comments {
            ui.add_space(16.0);
            egui::CollapsingHeader::new(format!("Comments ({})", req.comments.len()))
                .default_open(config.comments_default_open)
                .show(ui, |ui| {
                    comment_list(ui, &req.comments);
                });
        }
    });

    // Comment input outside ScrollArea to avoid borrow conflicts
    if config.show_comments {
        ui.separator();
        if let Some(content) = comment_input(ui, comment_input_text, &CommentInputConfig::default()) {
            action = Some(DetailViewAction::AddComment(content));
        }
    }

    action
}

/// Render the metadata grid
fn metadata_grid(ui: &mut Ui, req: &Requirement) {
    egui::Grid::new("detail_metadata_grid")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("ID:");
            ui.label(RichText::new(&req.id).monospace().small());
            ui.end_row();

            if !req.owner.is_empty() {
                ui.label("Owner:");
                ui.label(&req.owner);
                ui.end_row();
            }

            if !req.feature.is_empty() {
                ui.label("Feature:");
                ui.label(&req.feature);
                ui.end_row();
            }

            if let Some(ref ts) = req.created_at {
                ui.label("Created:");
                ui.label(format_timestamp(ts));
                ui.end_row();
            }

            if let Some(ref ts) = req.modified_at {
                ui.label("Modified:");
                ui.label(format_timestamp(ts));
                ui.end_row();
            }

            if !req.created_by.is_empty() {
                ui.label("Created by:");
                ui.label(&req.created_by);
                ui.end_row();
            }

            if req.archived {
                ui.label("Status:");
                ui.label(RichText::new("Archived").color(Color32::from_rgb(200, 150, 100)));
                ui.end_row();
            }
        });
}

/// Render the relationships view
fn relationships_view(ui: &mut Ui, req: &Requirement) {
    use crate::storage::proto::RelationshipType;

    for rel in &req.relationships {
        ui.horizontal(|ui| {
            let type_text = match RelationshipType::try_from(rel.rel_type) {
                Ok(RelationshipType::Parent) => "Parent",
                Ok(RelationshipType::Child) => "Child",
                Ok(RelationshipType::Verifies) => "Verifies",
                Ok(RelationshipType::VerifiedBy) => "Verified by",
                Ok(RelationshipType::References) => "References",
                Ok(RelationshipType::Duplicate) => "Duplicate of",
                Ok(RelationshipType::Custom) => "Custom",
                _ => "Related to",
            };
            ui.label(format!("{}:", type_text));
            ui.label(RichText::new(&rel.target_spec_id).monospace());
        });
    }
}

/// Render a placeholder for when no requirement is selected
pub fn no_selection_placeholder(ui: &mut Ui) {
    ui.centered_and_justified(|ui| {
        ui.label(RichText::new("Select a requirement from the list").weak());
    });
}

/// Render a loading state
pub fn loading_state(ui: &mut Ui) {
    ui.centered_and_justified(|ui| {
        ui.spinner();
        ui.label("Loading...");
    });
}

/// Render an error state
pub fn error_state(ui: &mut Ui, message: &str) {
    ui.centered_and_justified(|ui| {
        ui.label(RichText::new(format!("Error: {}", message)).color(Color32::RED));
    });
}
