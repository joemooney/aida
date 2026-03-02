// trace:FR-0273 | ai:claude:high
//! Requirement list item rendering
//!
//! Components for rendering requirements in list views (sidebar, search results, etc.)

use super::badges::{status_dot, status_dot_color};
use crate::storage::proto::Requirement;
use egui::{Color32, Response, RichText, Ui};

/// Configuration for list item rendering
#[derive(Clone, Default)]
pub struct ListItemConfig {
    /// Show the title on a separate line
    pub show_title: bool,
    /// Show tags
    pub show_tags: bool,
    /// Show feature badge
    pub show_feature: bool,
    /// Maximum title length (0 for unlimited)
    pub max_title_len: usize,
    /// Compact mode (tighter spacing)
    pub compact: bool,
}

impl ListItemConfig {
    /// Default configuration with title
    pub fn with_title() -> Self {
        Self {
            show_title: true,
            show_tags: false,
            show_feature: false,
            max_title_len: 40,
            compact: false,
        }
    }

    /// Compact configuration (status dot + spec_id only)
    pub fn compact() -> Self {
        Self {
            show_title: false,
            show_tags: false,
            show_feature: false,
            max_title_len: 0,
            compact: true,
        }
    }

    /// Full configuration with all details
    pub fn full() -> Self {
        Self {
            show_title: true,
            show_tags: true,
            show_feature: true,
            max_title_len: 60,
            compact: false,
        }
    }
}

/// Render a requirement list item
///
/// Returns true if the item was clicked (for selection handling)
pub fn requirement_list_item(
    ui: &mut Ui,
    req: &Requirement,
    selected: bool,
    config: &ListItemConfig,
) -> bool {
    let mut clicked = false;

    ui.horizontal(|ui| {
        // Status indicator dot
        status_dot(ui, req.status);

        // Spec ID (clickable)
        let response = ui.selectable_label(selected, RichText::new(&req.spec_id).monospace());

        if response.clicked() {
            clicked = true;
        }

        // Feature badge (inline)
        if config.show_feature && !req.feature.is_empty() {
            ui.label(RichText::new(format!("[{}]", &req.feature)).small().weak());
        }
    });

    // Title on separate line
    if config.show_title && !req.title.is_empty() {
        let title = if config.max_title_len > 0 && req.title.len() > config.max_title_len {
            format!(
                "{}...",
                &req.title
                    .chars()
                    .take(config.max_title_len - 3)
                    .collect::<String>()
            )
        } else {
            req.title.clone()
        };

        ui.label(RichText::new(&title).small().weak().color(if selected {
            Color32::WHITE
        } else {
            Color32::GRAY
        }));
    }

    // Tags
    if config.show_tags && !req.tags.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for tag in &req.tags {
                ui.label(
                    RichText::new(format!("#{}", tag))
                        .small()
                        .color(Color32::from_rgb(100, 150, 200)),
                );
            }
        });
    }

    if !config.compact {
        ui.add_space(4.0);
    }

    clicked
}

/// Render a simple list item (just dot + spec_id)
pub fn simple_list_item(ui: &mut Ui, req: &Requirement, selected: bool) -> Response {
    ui.horizontal(|ui| {
        status_dot(ui, req.status);
        ui.selectable_label(selected, RichText::new(&req.spec_id).monospace())
    })
    .inner
}

/// Render a compact list item suitable for KanBan cards
pub fn kanban_card_item(ui: &mut Ui, req: &Requirement) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let color = status_dot_color(req.status);
            ui.colored_label(color, "●");
            ui.label(RichText::new(&req.spec_id).monospace().small());
        });

        if !req.title.is_empty() {
            let title = if req.title.len() > 50 {
                format!("{}...", &req.title.chars().take(47).collect::<String>())
            } else {
                req.title.clone()
            };
            ui.label(RichText::new(title).small());
        }

        if !req.tags.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for tag in req.tags.iter().take(3) {
                    ui.label(RichText::new(format!("#{}", tag)).small().weak());
                }
                if req.tags.len() > 3 {
                    ui.label(
                        RichText::new(format!("+{}", req.tags.len() - 3))
                            .small()
                            .weak(),
                    );
                }
            });
        }
    });
}

/// Render a list of requirements with selection support
///
/// Returns the index of the clicked item, if any
pub fn requirement_list(
    ui: &mut Ui,
    requirements: &[Requirement],
    selected_idx: Option<usize>,
    config: &ListItemConfig,
) -> Option<usize> {
    let mut clicked_idx = None;

    for (idx, req) in requirements.iter().enumerate() {
        let is_selected = selected_idx == Some(idx);
        if requirement_list_item(ui, req, is_selected, config) {
            clicked_idx = Some(idx);
        }
    }

    clicked_idx
}

/// Render a search result list item (with match highlighting in the future)
pub fn search_result_item(
    ui: &mut Ui,
    req: &Requirement,
    selected: bool,
    _query: &str, // For future highlighting
) -> bool {
    // For now, same as regular list item with title
    requirement_list_item(ui, req, selected, &ListItemConfig::with_title())
}
