// trace:FR-0273 | ai:claude:high
//! Comment list rendering
//!
//! Components for displaying comments on requirements.

use super::formatters::format_relative_time;
use crate::storage::proto::Comment;
use egui::{Color32, RichText, TextEdit, Ui};

/// Render a single comment
pub fn comment_item(ui: &mut Ui, comment: &Comment) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&comment.author).strong());
            ui.label(RichText::new("•").weak());
            if let Some(ref ts) = comment.created_at {
                ui.label(RichText::new(format_relative_time(ts)).small().weak());
            }
        });
        ui.label(&comment.content);
    });
    ui.add_space(4.0);
}

/// Render a list of comments
pub fn comment_list(ui: &mut Ui, comments: &[Comment]) {
    if comments.is_empty() {
        ui.label(RichText::new("No comments yet").weak().italics());
        return;
    }

    for comment in comments {
        comment_item(ui, comment);
    }
}

/// Configuration for the comment input field
#[derive(Clone)]
pub struct CommentInputConfig {
    /// Placeholder text
    pub placeholder: &'static str,
    /// Width of the input field (relative to available width)
    pub width_subtract: f32,
    /// Button text
    pub button_text: &'static str,
}

impl Default for CommentInputConfig {
    fn default() -> Self {
        Self {
            placeholder: "Add a comment...",
            width_subtract: 80.0,
            button_text: "Add",
        }
    }
}

/// Render a comment input field with add button
///
/// Returns Some(content) if the add button was clicked and input is non-empty
pub fn comment_input(
    ui: &mut Ui,
    input: &mut String,
    config: &CommentInputConfig,
) -> Option<String> {
    let mut result = None;

    ui.horizontal(|ui| {
        let response = ui.add(
            TextEdit::singleline(input)
                .hint_text(config.placeholder)
                .desired_width(ui.available_width() - config.width_subtract),
        );

        let enter_pressed = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let button_clicked = ui.button(config.button_text).clicked();

        if (enter_pressed || button_clicked) && !input.trim().is_empty() {
            result = Some(input.trim().to_string());
            input.clear();
        }
    });

    result
}

/// Render a comment section (collapsible header with list and input)
///
/// Returns Some(content) if a new comment should be added
pub fn comment_section(
    ui: &mut Ui,
    comments: &[Comment],
    input: &mut String,
    default_open: bool,
) -> Option<String> {
    let mut new_comment = None;

    egui::CollapsingHeader::new(format!("Comments ({})", comments.len()))
        .default_open(default_open)
        .show(ui, |ui| {
            comment_list(ui, comments);
            ui.separator();
            new_comment = comment_input(ui, input, &CommentInputConfig::default());
        });

    new_comment
}

/// Render a compact comment indicator (for list views)
pub fn comment_indicator(ui: &mut Ui, count: usize) {
    if count > 0 {
        ui.label(
            RichText::new(format!("💬 {}", count))
                .small()
                .color(Color32::from_rgb(150, 150, 200)),
        );
    }
}

/// Render the comment author with timestamp in a compact format
pub fn comment_header_compact(ui: &mut Ui, comment: &Comment) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(&comment.author).small().strong());
        if let Some(ref ts) = comment.created_at {
            ui.label(RichText::new(format_relative_time(ts)).small().weak());
        }
    });
}
