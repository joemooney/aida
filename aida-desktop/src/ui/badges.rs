// trace:FR-0273 | ai:claude:high
//! Badge rendering for status, priority, and type indicators
//!
//! These functions render colored badges/labels for requirement metadata.

use egui::{Color32, RichText, Ui, Response};
use crate::storage::proto::{RequirementPriority, RequirementStatus, RequirementType};
use super::formatters::{format_status, format_priority, format_type};

/// Get the color for a status value
pub fn status_color(status: i32) -> Color32 {
    match RequirementStatus::try_from(status) {
        Ok(RequirementStatus::Draft) => Color32::from_rgb(100, 100, 100),
        Ok(RequirementStatus::Approved) => Color32::from_rgb(50, 120, 50),
        Ok(RequirementStatus::Planned) => Color32::from_rgb(50, 100, 150),
        Ok(RequirementStatus::InProgress) => Color32::from_rgb(180, 140, 50),
        Ok(RequirementStatus::Completed) => Color32::from_rgb(50, 150, 50),
        Ok(RequirementStatus::Rejected) => Color32::from_rgb(150, 50, 50),
        _ => Color32::GRAY,
    }
}

/// Get the status indicator dot color (for list views)
pub fn status_dot_color(status: i32) -> Color32 {
    match RequirementStatus::try_from(status) {
        Ok(RequirementStatus::Draft) => Color32::GRAY,
        Ok(RequirementStatus::Approved) => Color32::from_rgb(100, 200, 100),
        Ok(RequirementStatus::Planned) => Color32::from_rgb(100, 150, 200),
        Ok(RequirementStatus::InProgress) => Color32::from_rgb(200, 200, 100),
        Ok(RequirementStatus::Completed) => Color32::from_rgb(100, 255, 100),
        Ok(RequirementStatus::Rejected) => Color32::from_rgb(200, 100, 100),
        _ => Color32::GRAY,
    }
}

/// Get the color for a priority value
pub fn priority_color(priority: i32) -> Color32 {
    match RequirementPriority::try_from(priority) {
        Ok(RequirementPriority::High) => Color32::from_rgb(180, 50, 50),
        Ok(RequirementPriority::Medium) => Color32::from_rgb(180, 150, 50),
        Ok(RequirementPriority::Low) => Color32::from_rgb(50, 150, 180),
        _ => Color32::GRAY,
    }
}

/// Get the color for a requirement type
pub fn type_color(req_type: i32) -> Color32 {
    match RequirementType::try_from(req_type) {
        Ok(RequirementType::Bug) => Color32::from_rgb(200, 80, 80),
        Ok(RequirementType::Epic) => Color32::from_rgb(130, 80, 180),
        Ok(RequirementType::Story) => Color32::from_rgb(80, 150, 80),
        Ok(RequirementType::Task) => Color32::from_rgb(80, 130, 180),
        Ok(RequirementType::Spike) => Color32::from_rgb(180, 130, 80),
        Ok(RequirementType::Sprint) => Color32::from_rgb(80, 180, 180),
        _ => Color32::from_rgb(120, 120, 120),
    }
}

/// Render a status badge with background color
pub fn status_badge(ui: &mut Ui, status: i32) -> Response {
    let text = format_status(status);
    let color = status_color(status);
    ui.label(RichText::new(text).background_color(color).strong())
}

/// Render a priority badge with background color
pub fn priority_badge(ui: &mut Ui, priority: i32) -> Response {
    let text = format_priority(priority);
    let color = priority_color(priority);
    ui.label(RichText::new(text).background_color(color))
}

/// Render a type badge (lighter styling)
pub fn type_badge(ui: &mut Ui, req_type: i32) -> Response {
    let text = format_type(req_type);
    ui.label(RichText::new(text).weak())
}

/// Render a type badge with color
pub fn type_badge_colored(ui: &mut Ui, req_type: i32) -> Response {
    let text = format_type(req_type);
    let color = type_color(req_type);
    ui.label(RichText::new(text).background_color(color))
}

/// Render a status dot indicator (for list items)
pub fn status_dot(ui: &mut Ui, status: i32) -> Response {
    let color = status_dot_color(status);
    ui.colored_label(color, "●")
}

/// Render all badges in a horizontal row
pub fn all_badges(ui: &mut Ui, status: i32, priority: i32, req_type: i32) {
    ui.horizontal(|ui| {
        status_badge(ui, status);
        priority_badge(ui, priority);
        type_badge(ui, req_type);
    });
}

/// Render a compact badge row (for tight spaces)
pub fn compact_badges(ui: &mut Ui, status: i32, priority: i32) {
    ui.horizontal(|ui| {
        status_dot(ui, status);
        ui.label(RichText::new(format_priority(priority)).small());
    });
}

/// Render a tag list
pub fn tag_list(ui: &mut Ui, tags: &[String]) {
    if tags.is_empty() {
        return;
    }

    ui.horizontal_wrapped(|ui| {
        for tag in tags {
            ui.label(
                RichText::new(format!("#{}", tag))
                    .small()
                    .color(Color32::from_rgb(100, 150, 200))
            );
        }
    });
}

/// Render a feature badge (if feature is set)
pub fn feature_badge(ui: &mut Ui, feature: &str) {
    if !feature.is_empty() {
        ui.label(
            RichText::new(format!("[{}]", feature))
                .small()
                .color(Color32::from_rgb(150, 120, 180))
        );
    }
}
