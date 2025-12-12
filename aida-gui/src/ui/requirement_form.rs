// trace:FR-0273 | ai:claude:high
//! Requirement form rendering
//!
//! Reusable form components for creating and editing requirements.

use egui::{TextEdit, Ui};
use crate::storage::proto::{RequirementPriority, RequirementStatus, RequirementType};
use super::formatters::{format_status, format_priority, format_type};

/// Form data for creating/editing requirements
#[derive(Default, Clone)]
pub struct RequirementFormData {
    pub title: String,
    pub description: String,
    pub status: i32,
    pub priority: i32,
    pub req_type: i32,
    pub owner: String,
    pub feature: String,
    pub tags: Vec<String>,
    /// Temporary string for editing tags
    pub tags_input: String,
}

impl RequirementFormData {
    /// Create form data with default values for a new requirement
    pub fn new_requirement() -> Self {
        Self {
            title: String::new(),
            description: String::new(),
            status: RequirementStatus::Draft.into(),
            priority: RequirementPriority::Medium.into(),
            req_type: RequirementType::Functional.into(),
            owner: String::new(),
            feature: String::new(),
            tags: Vec::new(),
            tags_input: String::new(),
        }
    }

    /// Create form data from an existing requirement
    pub fn from_requirement(req: &crate::storage::proto::Requirement) -> Self {
        Self {
            title: req.title.clone(),
            description: req.description.clone(),
            status: req.status,
            priority: req.priority,
            req_type: req.req_type,
            owner: req.owner.clone(),
            feature: req.feature.clone(),
            tags: req.tags.clone(),
            tags_input: req.tags.join(", "),
        }
    }

    /// Clear all form fields
    pub fn clear(&mut self) {
        *self = Self::new_requirement();
    }

    /// Validate the form data
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.title.trim().is_empty() {
            return Err("Title is required");
        }
        Ok(())
    }

    /// Parse tags from the input string
    pub fn parse_tags(&mut self) {
        self.tags = self.tags_input
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
}

/// Configuration for the requirement form
#[derive(Clone)]
pub struct RequirementFormConfig {
    /// Width for text input fields
    pub field_width: f32,
    /// Number of rows for description
    pub description_rows: usize,
    /// Show owner field
    pub show_owner: bool,
    /// Show feature field
    pub show_feature: bool,
    /// Show tags field
    pub show_tags: bool,
}

impl Default for RequirementFormConfig {
    fn default() -> Self {
        Self {
            field_width: 400.0,
            description_rows: 6,
            show_owner: false,
            show_feature: false,
            show_tags: false,
        }
    }
}

impl RequirementFormConfig {
    /// Basic form (title, description, status, priority, type)
    pub fn basic() -> Self {
        Self::default()
    }

    /// Full form with all fields
    pub fn full() -> Self {
        Self {
            show_owner: true,
            show_feature: true,
            show_tags: true,
            ..Default::default()
        }
    }

    /// Compact form (smaller field width)
    pub fn compact() -> Self {
        Self {
            field_width: 280.0,
            description_rows: 4,
            ..Default::default()
        }
    }
}

/// Render the requirement form
///
/// Returns true if any field was modified
pub fn requirement_form(
    ui: &mut Ui,
    data: &mut RequirementFormData,
    config: &RequirementFormConfig,
) -> bool {
    let mut modified = false;

    egui::Grid::new("requirement_form_grid")
        .num_columns(2)
        .spacing([8.0, 8.0])
        .show(ui, |ui| {
            // Title
            ui.label("Title:");
            let response = ui.add(
                TextEdit::singleline(&mut data.title)
                    .hint_text("Requirement title")
                    .desired_width(config.field_width),
            );
            if response.changed() {
                modified = true;
            }
            ui.end_row();

            // Description
            ui.label("Description:");
            let response = ui.add(
                TextEdit::multiline(&mut data.description)
                    .hint_text("Detailed description...")
                    .desired_width(config.field_width)
                    .desired_rows(config.description_rows),
            );
            if response.changed() {
                modified = true;
            }
            ui.end_row();

            // Status
            ui.label("Status:");
            if status_combo(ui, &mut data.status) {
                modified = true;
            }
            ui.end_row();

            // Priority
            ui.label("Priority:");
            if priority_combo(ui, &mut data.priority) {
                modified = true;
            }
            ui.end_row();

            // Type
            ui.label("Type:");
            if type_combo(ui, &mut data.req_type) {
                modified = true;
            }
            ui.end_row();

            // Owner (optional)
            if config.show_owner {
                ui.label("Owner:");
                let response = ui.add(
                    TextEdit::singleline(&mut data.owner)
                        .hint_text("Owner name")
                        .desired_width(config.field_width),
                );
                if response.changed() {
                    modified = true;
                }
                ui.end_row();
            }

            // Feature (optional)
            if config.show_feature {
                ui.label("Feature:");
                let response = ui.add(
                    TextEdit::singleline(&mut data.feature)
                        .hint_text("Feature name")
                        .desired_width(config.field_width),
                );
                if response.changed() {
                    modified = true;
                }
                ui.end_row();
            }

            // Tags (optional)
            if config.show_tags {
                ui.label("Tags:");
                let response = ui.add(
                    TextEdit::singleline(&mut data.tags_input)
                        .hint_text("tag1, tag2, tag3")
                        .desired_width(config.field_width),
                );
                if response.changed() {
                    data.parse_tags();
                    modified = true;
                }
                ui.end_row();
            }
        });

    modified
}

/// Render a status combo box
///
/// Returns true if the value changed
pub fn status_combo(ui: &mut Ui, status: &mut i32) -> bool {
    let mut changed = false;
    let current_text = format_status(*status);

    egui::ComboBox::from_id_salt("status_combo")
        .selected_text(current_text)
        .show_ui(ui, |ui| {
            if ui.selectable_value(status, RequirementStatus::Draft.into(), "Draft").changed() {
                changed = true;
            }
            if ui.selectable_value(status, RequirementStatus::Approved.into(), "Approved").changed() {
                changed = true;
            }
            if ui.selectable_value(status, RequirementStatus::Planned.into(), "Planned").changed() {
                changed = true;
            }
            if ui.selectable_value(status, RequirementStatus::InProgress.into(), "In Progress").changed() {
                changed = true;
            }
            if ui.selectable_value(status, RequirementStatus::Completed.into(), "Completed").changed() {
                changed = true;
            }
            if ui.selectable_value(status, RequirementStatus::Rejected.into(), "Rejected").changed() {
                changed = true;
            }
        });

    changed
}

/// Render a priority combo box
///
/// Returns true if the value changed
pub fn priority_combo(ui: &mut Ui, priority: &mut i32) -> bool {
    let mut changed = false;
    let current_text = format_priority(*priority);

    egui::ComboBox::from_id_salt("priority_combo")
        .selected_text(current_text)
        .show_ui(ui, |ui| {
            if ui.selectable_value(priority, RequirementPriority::High.into(), "High").changed() {
                changed = true;
            }
            if ui.selectable_value(priority, RequirementPriority::Medium.into(), "Medium").changed() {
                changed = true;
            }
            if ui.selectable_value(priority, RequirementPriority::Low.into(), "Low").changed() {
                changed = true;
            }
        });

    changed
}

/// Render a type combo box
///
/// Returns true if the value changed
pub fn type_combo(ui: &mut Ui, req_type: &mut i32) -> bool {
    let mut changed = false;
    let current_text = format_type(*req_type);

    egui::ComboBox::from_id_salt("type_combo")
        .selected_text(current_text)
        .show_ui(ui, |ui| {
            if ui.selectable_value(req_type, RequirementType::Functional.into(), "Functional").changed() {
                changed = true;
            }
            if ui.selectable_value(req_type, RequirementType::NonFunctional.into(), "Non-Functional").changed() {
                changed = true;
            }
            if ui.selectable_value(req_type, RequirementType::Bug.into(), "Bug").changed() {
                changed = true;
            }
            if ui.selectable_value(req_type, RequirementType::Epic.into(), "Epic").changed() {
                changed = true;
            }
            if ui.selectable_value(req_type, RequirementType::Story.into(), "Story").changed() {
                changed = true;
            }
            if ui.selectable_value(req_type, RequirementType::Task.into(), "Task").changed() {
                changed = true;
            }
            if ui.selectable_value(req_type, RequirementType::Spike.into(), "Spike").changed() {
                changed = true;
            }
        });

    changed
}
