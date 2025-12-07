use aida_core::{
    ai::AiClient,
    determine_requirements_path, AddResult, Cardinality, Comment, ConflictInfo, ConflictResolution,
    CustomFieldDefinition, CustomFieldType, EditLock, EvaluationResponse, FieldChange, IdFormat,
    LockFileInfo, NumberingStrategy, RelationshipDefinition, RelationshipType, Requirement,
    RequirementPriority, RequirementStatus, RequirementType, RequirementsStore, SaveResult,
    SessionInfo, Storage, StoredAiEvaluation, UrlLink,
};
use eframe::egui;
use similar::{ChangeTag, TextDiff};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;
use uuid::Uuid;

/// Commands sent to the heartbeat thread
enum HeartbeatCommand {
    /// Update the edit lock for a requirement
    SetEditLock(Option<EditLock>),
    /// Shutdown the heartbeat thread
    Shutdown,
}

// =============================================================================
// TEXT EDIT CONTEXT MENU IMPLEMENTATION
// =============================================================================
//
// BACKGROUND:
// egui's TextEdit widget clears the text selection when a right-click occurs,
// before any user code can intercept it. This is a known limitation.
// See: https://github.com/emilk/egui/issues/5852
//
// WORKAROUND:
// We continuously capture the selection state while the TextEdit has focus.
// This way, when the user right-clicks, we already have the selection stored
// from the previous frame, before the right-click event cleared it.
//
// LIMITATIONS:
// 1. The visual selection highlight disappears when the context menu opens
//    (we show the selected text in the menu as a workaround)
// 2. Selection is not preserved in the TextEdit after menu closes
//
// FUTURE IMPROVEMENTS (check these egui issues/features):
// - Issue #5852: Add support for X11/Wayland PRIMARY clipboard
//   https://github.com/emilk/egui/issues/5852
// - Issue #7273: TextSelectionChanged events for all selectable text
//   https://github.com/emilk/egui/issues/7273
// - If egui adds a way to prevent right-click from clearing selection,
//   or provides a "selection changed" callback, this code can be simplified
// - If egui adds native context menu support for TextEdit with Copy/Paste,
//   this entire workaround may become unnecessary
//
// TO UPDATE WHEN EGUI IMPROVES:
// 1. If egui preserves selection on right-click: remove continuous capture,
//    just read selection directly in context_menu callback
// 2. If egui adds TextSelectionChanged event: use that instead of polling
// 3. If egui adds native context menu: remove this entirely
// =============================================================================

/// Helper to get selection range from CCursorRange (handles primary/secondary ordering)
fn get_sorted_range(range: &egui::text::CCursorRange) -> (usize, usize) {
    let start = range.primary.index.min(range.secondary.index);
    let end = range.primary.index.max(range.secondary.index);
    (start, end)
}

/// Capture current text selection from a TextEdit widget.
///
/// This reads the TextEdit's internal state to get the current cursor/selection range.
/// Returns None if there's no selection (cursor is just a point, not a range).
fn capture_text_selection(ctx: &egui::Context, text: &str, id: egui::Id) -> Option<TextSelection> {
    let state = egui::TextEdit::load_state(ctx, id)?;
    let range = state.cursor.char_range()?;
    let (start, end) = get_sorted_range(&range);
    if start == end {
        return None; // No selection (just cursor position)
    }
    let selected: String = text.chars().skip(start).take(end - start).collect();
    Some(TextSelection {
        text: selected,
        start,
        end,
        widget_id: Some(id),
    })
}

/// Show context menu for TextEdit with Cut/Copy/Paste/Select All.
///
/// # Workaround for egui selection clearing
///
/// egui's TextEdit clears the selection when right-click occurs. To work around this,
/// we continuously capture the selection while the TextEdit has focus. The selection
/// is stored in `stored_selection` and used when the context menu opens.
///
/// # Arguments
/// * `ui` - The egui UI context
/// * `response` - The Response from the TextEdit widget
/// * `text` - Mutable reference to the text buffer
/// * `id` - The widget ID (use `response.id`)
/// * `stored_selection` - Mutable reference to stored selection state (persists across frames)
///
/// # Linux Primary Selection
/// On Linux, Copy and Cut also write to the X11/Wayland PRIMARY selection,
/// enabling middle-click paste in other applications.
fn show_text_context_menu(
    ui: &mut egui::Ui,
    response: &egui::Response,
    text: &mut String,
    id: egui::Id,
    stored_selection: &mut Option<TextSelection>,
) {
    // WORKAROUND: Continuously capture selection while TextEdit has focus.
    // This ensures we have the selection stored BEFORE right-click clears it.
    // See module-level documentation for details on this workaround.
    if response.has_focus() {
        if let Some(selection) = capture_text_selection(ui.ctx(), text, id) {
            // Only update if this is for the same widget and selection changed
            let should_update = stored_selection
                .as_ref()
                .map(|s| s.widget_id != Some(id) || s.text != selection.text)
                .unwrap_or(true);
            if should_update {
                *stored_selection = Some(selection);
            }
        }
    }

    response.context_menu(|ui| {
        // Use stored selection (captured continuously while focused, before right-click cleared it)
        let selection = stored_selection.clone().filter(|s| s.widget_id == Some(id));
        let has_selection = selection
            .as_ref()
            .map(|s| !s.text.is_empty())
            .unwrap_or(false);

        // WORKAROUND: Show selected text in menu since visual selection is cleared
        // This lets users see what they're about to copy/cut
        if let Some(ref sel) = selection {
            let display_text = if sel.text.len() > 50 {
                format!("\"{}...\"", &sel.text.chars().take(47).collect::<String>())
            } else {
                format!("\"{}\"", &sel.text)
            };
            // Replace newlines with visible indicator
            let display_text = display_text.replace('\n', "↵");
            ui.label(egui::RichText::new(display_text).italics().weak());
            ui.separator();
        }

        // Cut
        if ui
            .add_enabled(has_selection, egui::Button::new("✂ Cut"))
            .clicked()
        {
            if let Some(ref sel) = selection {
                // Copy to clipboard (both regular and primary on Linux)
                ui.ctx().copy_text(sel.text.clone());
                copy_to_primary_selection(&sel.text);

                // Remove the selected text
                let before: String = text.chars().take(sel.start).collect();
                let after: String = text.chars().skip(sel.end).collect();
                *text = before + &after;
                *stored_selection = None;
            }
            ui.close_menu();
        }

        // Copy
        if ui
            .add_enabled(has_selection, egui::Button::new("📋 Copy"))
            .clicked()
        {
            if let Some(ref sel) = selection {
                // Copy to clipboard (both regular and primary on Linux)
                ui.ctx().copy_text(sel.text.clone());
                copy_to_primary_selection(&sel.text);
            }
            ui.close_menu();
        }

        // Paste
        let can_paste = get_clipboard_text().is_some();

        if ui
            .add_enabled(can_paste, egui::Button::new("📥 Paste"))
            .clicked()
        {
            if let Some(paste_text) = get_clipboard_text() {
                if let Some(ref sel) = selection {
                    // Replace selection with pasted text
                    let before: String = text.chars().take(sel.start).collect();
                    let after: String = text.chars().skip(sel.end).collect();
                    *text = before + &paste_text + &after;
                } else {
                    // No selection - get cursor position from state
                    if let Some(state) = egui::TextEdit::load_state(ui.ctx(), id) {
                        if let Some(range) = state.cursor.char_range() {
                            let pos = range.primary.index;
                            let before: String = text.chars().take(pos).collect();
                            let after: String = text.chars().skip(pos).collect();
                            *text = before + &paste_text + &after;
                        } else {
                            text.push_str(&paste_text);
                        }
                    } else {
                        text.push_str(&paste_text);
                    }
                }
                *stored_selection = None;
            }
            ui.close_menu();
        }

        ui.separator();

        // Select All
        if ui.button("Select All").clicked() {
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                let len = text.chars().count();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(len),
                    )));
                state.store(ui.ctx(), id);
            }
            ui.close_menu();
        }
    });
}

/// Copy text to the X11/Wayland primary selection (middle-click paste buffer).
///
/// On Linux/X11, there are two clipboard mechanisms:
/// 1. CLIPBOARD - Used by Ctrl+C/Ctrl+V (handled by egui's ctx.copy_text())
/// 2. PRIMARY - Filled by selecting text, pasted with middle-click
///
/// egui only handles CLIPBOARD, so we use arboard to also copy to PRIMARY.
/// This enables middle-click paste in other applications after copying from our app.
///
/// This is a workaround until egui adds native PRIMARY selection support.
/// See: https://github.com/emilk/egui/issues/5852
#[cfg(target_os = "linux")]
fn copy_to_primary_selection(text: &str) {
    use arboard::SetExtLinux;
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        // Set to primary selection for middle-click paste
        let _ = clipboard
            .set()
            .clipboard(arboard::LinuxClipboardKind::Primary)
            .text(text.to_string());
    }
}

/// No-op on non-Linux platforms (PRIMARY selection is X11/Wayland specific)
#[cfg(not(target_os = "linux"))]
fn copy_to_primary_selection(_text: &str) {}

/// Get text from the system clipboard (CLIPBOARD, not PRIMARY)
fn get_clipboard_text() -> Option<String> {
    arboard::Clipboard::new()
        .ok()
        .and_then(|mut c| c.get_text().ok())
}

/// Format a requirement as a prompt for Claude Code implementation
fn format_requirement_for_claude_code(req: &Requirement) -> String {
    let mut prompt = String::new();

    // Header with requirement ID
    let req_id = req.spec_id.as_deref().unwrap_or("(no ID)");
    prompt.push_str(&format!("# Implement Requirement: {} - {}\n\n", req_id, req.title));

    // Requirement type and priority
    prompt.push_str(&format!("**Type:** {}  \n", req.req_type));
    prompt.push_str(&format!("**Priority:** {}  \n", req.priority));
    if !req.feature.is_empty() {
        prompt.push_str(&format!("**Feature:** {}  \n", req.feature));
    }
    prompt.push_str("\n");

    // Description
    prompt.push_str("## Description\n\n");
    prompt.push_str(&req.description);
    prompt.push_str("\n\n");

    // Tags if present
    if !req.tags.is_empty() {
        prompt.push_str("## Tags\n\n");
        for tag in &req.tags {
            prompt.push_str(&format!("- {}\n", tag));
        }
        prompt.push_str("\n");
    }

    // Implementation instructions
    prompt.push_str("## Implementation Task\n\n");
    prompt.push_str("Please implement this requirement in the codebase. ");
    prompt.push_str("After implementation:\n");
    prompt.push_str("1. Ensure the code follows the project's existing patterns and style\n");
    prompt.push_str("2. Add appropriate tests if applicable\n");
    prompt.push_str("3. Update the requirement status to 'Completed' when done\n");

    prompt
}

/// Default base font size in points
const DEFAULT_FONT_SIZE: f32 = 14.0;
/// Minimum font size
const MIN_FONT_SIZE: f32 = 8.0;
/// Maximum font size
const MAX_FONT_SIZE: f32 = 32.0;
/// Font size step for zoom in/out
const FONT_SIZE_STEP: f32 = 1.0;

/// Modal window size constraints (as percentage of screen size)
const MODAL_MAX_WIDTH_PERCENT: f32 = 0.90;
const MODAL_MAX_HEIGHT_PERCENT: f32 = 0.85;

/// Calculate modal window max size based on screen dimensions
fn modal_max_size(ctx: &egui::Context) -> egui::Vec2 {
    let screen_rect = ctx.screen_rect();
    egui::vec2(
        screen_rect.width() * MODAL_MAX_WIDTH_PERCENT,
        screen_rect.height() * MODAL_MAX_HEIGHT_PERCENT,
    )
}

/// Calculate constrained modal size - returns (width, height) clamped to max
fn constrained_modal_size(ctx: &egui::Context, desired_width: f32, desired_height: f32) -> egui::Vec2 {
    let max = modal_max_size(ctx);
    egui::vec2(
        desired_width.min(max.x),
        desired_height.min(max.y),
    )
}

/// Truncate a string to a maximum length, adding "..." if truncated
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Render an inline word diff between two strings in the UI
/// Shows deletions in red/strikethrough and insertions in green
fn render_inline_diff(ui: &mut egui::Ui, old_text: &str, new_text: &str) {
    let diff = TextDiff::from_words(old_text, new_text);

    let delete_color = egui::Color32::from_rgb(255, 100, 100);
    let insert_color = egui::Color32::from_rgb(100, 255, 100);
    let unchanged_color = ui.visuals().text_color();

    // Build a LayoutJob for inline rendering
    let mut job = egui::text::LayoutJob::default();
    job.wrap = egui::text::TextWrapping {
        max_width: ui.available_width(),
        ..Default::default()
    };

    for change in diff.iter_all_changes() {
        let text = change.value();
        match change.tag() {
            ChangeTag::Delete => {
                job.append(
                    text,
                    0.0,
                    egui::TextFormat {
                        color: delete_color,
                        strikethrough: egui::Stroke::new(1.0, delete_color),
                        ..Default::default()
                    },
                );
            }
            ChangeTag::Insert => {
                job.append(
                    text,
                    0.0,
                    egui::TextFormat {
                        color: insert_color,
                        ..Default::default()
                    },
                );
            }
            ChangeTag::Equal => {
                job.append(
                    text,
                    0.0,
                    egui::TextFormat {
                        color: unchanged_color,
                        ..Default::default()
                    },
                );
            }
        }
    }

    ui.label(job);
}

/// Serializable color wrapper for themes
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThemeColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl ThemeColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn with_alpha(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn to_egui(self) -> egui::Color32 {
        egui::Color32::from_rgba_unmultiplied(self.r, self.g, self.b, self.a)
    }

    pub fn from_egui(color: egui::Color32) -> Self {
        Self {
            r: color.r(),
            g: color.g(),
            b: color.b(),
            a: color.a(),
        }
    }
}

impl Default for ThemeColor {
    fn default() -> Self {
        Self::new(128, 128, 128)
    }
}

/// Helper to show a color picker widget
fn color_picker_widget(ui: &mut egui::Ui, color: &mut ThemeColor) {
    let mut color32 = color.to_egui();

    egui::color_picker::color_edit_button_srgba(
        ui,
        &mut color32,
        egui::color_picker::Alpha::Opaque,
    );

    *color = ThemeColor::from_egui(color32);
}

/// Custom theme with full visual customization
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomTheme {
    /// Name of this custom theme
    pub name: String,
    /// Base theme to extend from
    pub base: BaseTheme,

    // === Background Colors ===
    /// Main window background
    pub window_fill: ThemeColor,
    /// Panel background color
    pub panel_fill: ThemeColor,
    /// Extreme background (e.g., text edit background)
    pub extreme_bg: ThemeColor,
    /// Faint background (subtle separators)
    pub faint_bg: ThemeColor,

    // === Text Colors ===
    /// Primary text color (None = use widget defaults)
    pub text_color: Option<ThemeColor>,
    /// Hyperlink color
    pub hyperlink_color: ThemeColor,
    /// Warning text color
    pub warn_fg: ThemeColor,
    /// Error text color
    pub error_fg: ThemeColor,

    // === Widget Colors - Noninteractive ===
    /// Non-interactive widget background
    pub widget_bg: ThemeColor,
    /// Non-interactive widget text/stroke color
    pub widget_fg: ThemeColor,

    // === Widget Colors - Interactive States ===
    /// Inactive/default widget background
    pub widget_inactive_bg: ThemeColor,
    /// Hovered widget background
    pub widget_hovered_bg: ThemeColor,
    /// Active/pressed widget background
    pub widget_active_bg: ThemeColor,
    /// Open (expanded) widget background
    pub widget_open_bg: ThemeColor,

    // === Selection ===
    /// Selection background
    pub selection_bg: ThemeColor,
    /// Selection text color
    pub selection_fg: ThemeColor,

    // === Strokes & Borders ===
    /// Widget border width
    pub widget_stroke_width: f32,
    /// Widget border color (inactive)
    pub widget_stroke_color: ThemeColor,
    /// Widget border color (hovered)
    pub widget_hovered_stroke_color: ThemeColor,
    /// Widget border color (active)
    pub widget_active_stroke_color: ThemeColor,

    // === Rounding ===
    /// Widget corner rounding
    pub widget_rounding: f32,
    /// Window corner rounding
    pub window_rounding: f32,

    // === Shadows ===
    /// Window shadow enabled
    pub window_shadow: bool,
    /// Popup shadow enabled
    pub popup_shadow: bool,

    // === Spacing ===
    /// Item spacing (horizontal, vertical)
    pub item_spacing: (f32, f32),
    /// Button padding (horizontal, vertical)
    pub button_padding: (f32, f32),
    /// Window padding
    pub window_padding: (f32, f32),

    // === Miscellaneous ===
    /// Use dark mode icons/decorations
    pub dark_mode: bool,
    /// Scroll bar width
    pub scroll_bar_width: f32,
    /// Indent amount for nested items
    pub indent: f32,

    // === Detail View Title Bar ===
    /// Title bar background color
    pub title_bar_bg: ThemeColor,
    /// Title bar text color (None = use default heading color)
    pub title_bar_text: Option<ThemeColor>,
    /// Title bar font size multiplier (1.0 = default heading size)
    pub title_bar_font_size: f32,
}

impl Default for CustomTheme {
    fn default() -> Self {
        Self::from_base(BaseTheme::Dark, "Custom Dark".to_string())
    }
}

impl CustomTheme {
    /// Create a custom theme based on a built-in base theme
    pub fn from_base(base: BaseTheme, name: String) -> Self {
        match base {
            BaseTheme::Dark => Self::dark_defaults(name),
            BaseTheme::Light => Self::light_defaults(name),
        }
    }

    fn dark_defaults(name: String) -> Self {
        Self {
            name,
            base: BaseTheme::Dark,
            // Background
            window_fill: ThemeColor::new(27, 27, 27),
            panel_fill: ThemeColor::new(27, 27, 27),
            extreme_bg: ThemeColor::new(10, 10, 10),
            faint_bg: ThemeColor::new(5, 5, 5),
            // Text
            text_color: None,
            hyperlink_color: ThemeColor::new(90, 170, 255),
            warn_fg: ThemeColor::new(255, 143, 0),
            error_fg: ThemeColor::new(255, 0, 0),
            // Widgets - noninteractive
            widget_bg: ThemeColor::new(27, 27, 27),
            widget_fg: ThemeColor::new(140, 140, 140),
            // Widgets - interactive
            widget_inactive_bg: ThemeColor::new(60, 60, 60),
            widget_hovered_bg: ThemeColor::new(70, 70, 70),
            widget_active_bg: ThemeColor::new(55, 55, 55),
            widget_open_bg: ThemeColor::new(27, 27, 27),
            // Selection
            selection_bg: ThemeColor::new(0, 92, 128),
            selection_fg: ThemeColor::new(255, 255, 255),
            // Strokes
            widget_stroke_width: 1.0,
            widget_stroke_color: ThemeColor::new(60, 60, 60),
            widget_hovered_stroke_color: ThemeColor::new(150, 150, 150),
            widget_active_stroke_color: ThemeColor::new(255, 255, 255),
            // Rounding
            widget_rounding: 2.0,
            window_rounding: 6.0,
            // Shadows
            window_shadow: true,
            popup_shadow: true,
            // Spacing
            item_spacing: (8.0, 3.0),
            button_padding: (4.0, 1.0),
            window_padding: (6.0, 6.0),
            // Misc
            dark_mode: true,
            scroll_bar_width: 8.0,
            indent: 18.0,
            // Title bar - slightly lighter than panel for dark theme
            title_bar_bg: ThemeColor::new(45, 45, 50),
            title_bar_text: None,
            title_bar_font_size: 1.0,
        }
    }

    fn light_defaults(name: String) -> Self {
        Self {
            name,
            base: BaseTheme::Light,
            // Background
            window_fill: ThemeColor::new(248, 248, 248),
            panel_fill: ThemeColor::new(248, 248, 248),
            extreme_bg: ThemeColor::new(255, 255, 255),
            faint_bg: ThemeColor::new(245, 245, 245),
            // Text
            text_color: None,
            hyperlink_color: ThemeColor::new(0, 102, 204),
            warn_fg: ThemeColor::new(255, 100, 0),
            error_fg: ThemeColor::new(220, 0, 0),
            // Widgets - noninteractive
            widget_bg: ThemeColor::new(248, 248, 248),
            widget_fg: ThemeColor::new(100, 100, 100),
            // Widgets - interactive
            widget_inactive_bg: ThemeColor::new(230, 230, 230),
            widget_hovered_bg: ThemeColor::new(210, 210, 210),
            widget_active_bg: ThemeColor::new(195, 195, 195),
            widget_open_bg: ThemeColor::new(235, 235, 235),
            // Selection
            selection_bg: ThemeColor::new(144, 209, 255),
            selection_fg: ThemeColor::new(0, 0, 0),
            // Strokes
            widget_stroke_width: 1.0,
            widget_stroke_color: ThemeColor::new(180, 180, 180),
            widget_hovered_stroke_color: ThemeColor::new(105, 105, 105),
            widget_active_stroke_color: ThemeColor::new(0, 0, 0),
            // Rounding
            widget_rounding: 2.0,
            window_rounding: 6.0,
            // Shadows
            window_shadow: false,
            popup_shadow: true,
            // Spacing
            item_spacing: (8.0, 3.0),
            button_padding: (4.0, 1.0),
            window_padding: (6.0, 6.0),
            // Misc
            dark_mode: false,
            scroll_bar_width: 8.0,
            indent: 18.0,
            // Title bar - slightly darker than panel for light theme
            title_bar_bg: ThemeColor::new(220, 220, 225),
            title_bar_text: None,
            title_bar_font_size: 1.0,
        }
    }

    /// Apply this custom theme to the egui context
    pub fn apply(&self, ctx: &egui::Context) {
        let mut visuals = if self.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        // Background colors
        visuals.window_fill = self.window_fill.to_egui();
        visuals.panel_fill = self.panel_fill.to_egui();
        visuals.extreme_bg_color = self.extreme_bg.to_egui();
        visuals.faint_bg_color = self.faint_bg.to_egui();

        // Text colors
        visuals.override_text_color = self.text_color.map(|c| c.to_egui());
        visuals.hyperlink_color = self.hyperlink_color.to_egui();
        visuals.warn_fg_color = self.warn_fg.to_egui();
        visuals.error_fg_color = self.error_fg.to_egui();

        // Widgets - noninteractive
        visuals.widgets.noninteractive.bg_fill = self.widget_bg.to_egui();
        visuals.widgets.noninteractive.fg_stroke =
            egui::Stroke::new(1.0, self.widget_fg.to_egui());

        // Widgets - interactive states
        visuals.widgets.inactive.bg_fill = self.widget_inactive_bg.to_egui();
        visuals.widgets.inactive.bg_stroke =
            egui::Stroke::new(self.widget_stroke_width, self.widget_stroke_color.to_egui());
        visuals.widgets.inactive.rounding = egui::Rounding::same(self.widget_rounding);

        visuals.widgets.hovered.bg_fill = self.widget_hovered_bg.to_egui();
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(
            self.widget_stroke_width,
            self.widget_hovered_stroke_color.to_egui(),
        );
        visuals.widgets.hovered.rounding = egui::Rounding::same(self.widget_rounding);

        visuals.widgets.active.bg_fill = self.widget_active_bg.to_egui();
        visuals.widgets.active.bg_stroke = egui::Stroke::new(
            self.widget_stroke_width,
            self.widget_active_stroke_color.to_egui(),
        );
        visuals.widgets.active.rounding = egui::Rounding::same(self.widget_rounding);

        visuals.widgets.open.bg_fill = self.widget_open_bg.to_egui();
        visuals.widgets.open.rounding = egui::Rounding::same(self.widget_rounding);

        // Selection
        visuals.selection.bg_fill = self.selection_bg.to_egui();
        visuals.selection.stroke = egui::Stroke::new(1.0, self.selection_fg.to_egui());

        // Window rounding
        visuals.window_rounding = egui::Rounding::same(self.window_rounding);

        // Shadows
        if !self.window_shadow {
            visuals.window_shadow = egui::Shadow::NONE;
        }
        if !self.popup_shadow {
            visuals.popup_shadow = egui::Shadow::NONE;
        }

        ctx.set_visuals(visuals);

        // Apply spacing settings
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(self.item_spacing.0, self.item_spacing.1);
        style.spacing.button_padding = egui::vec2(self.button_padding.0, self.button_padding.1);
        style.spacing.window_margin = egui::Margin::same(self.window_padding.0);
        style.spacing.scroll.bar_width = self.scroll_bar_width;
        style.spacing.indent = self.indent;
        ctx.set_style(style);
    }
}

/// Base theme to extend from
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BaseTheme {
    #[default]
    Dark,
    Light,
}

impl BaseTheme {
    fn label(&self) -> &'static str {
        match self {
            BaseTheme::Dark => "Dark",
            BaseTheme::Light => "Light",
        }
    }
}

/// Available color themes
#[derive(Debug, Default, PartialEq, Clone, Serialize, Deserialize)]
pub enum Theme {
    /// Dark theme (default egui dark)
    #[default]
    Dark,
    /// Light theme
    Light,
    /// Vibrant Light - colorful light theme with modern accents
    VibrantLight,
    /// High contrast dark
    HighContrastDark,
    /// Solarized dark
    SolarizedDark,
    /// Nord theme
    Nord,
    /// Custom theme with user-defined colors
    Custom(Box<CustomTheme>),
}

impl Theme {
    fn label(&self) -> String {
        match self {
            Theme::Dark => "Dark".to_string(),
            Theme::Light => "Light".to_string(),
            Theme::VibrantLight => "Vibrant Light".to_string(),
            Theme::HighContrastDark => "High Contrast Dark".to_string(),
            Theme::SolarizedDark => "Solarized Dark".to_string(),
            Theme::Nord => "Nord".to_string(),
            Theme::Custom(theme) => format!("Custom: {}", theme.name),
        }
    }

    fn next(&self) -> Theme {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::VibrantLight,
            Theme::VibrantLight => Theme::HighContrastDark,
            Theme::HighContrastDark => Theme::SolarizedDark,
            Theme::SolarizedDark => Theme::Nord,
            Theme::Nord => Theme::Dark,
            Theme::Custom(_) => Theme::Dark, // Cycle back to Dark from Custom
        }
    }

    fn apply(&self, ctx: &egui::Context) {
        match self {
            Theme::Dark => {
                ctx.set_visuals(egui::Visuals::dark());
            }
            Theme::Light => {
                ctx.set_visuals(egui::Visuals::light());
            }
            Theme::VibrantLight => {
                let mut visuals = egui::Visuals::light();

                // Vibrant, colorful accent colors
                let accent_primary = egui::Color32::from_rgb(99, 102, 241);   // Indigo/purple
                let accent_hover = egui::Color32::from_rgb(129, 140, 248);    // Lighter indigo
                let accent_active = egui::Color32::from_rgb(79, 70, 229);     // Deeper indigo
                let selection_bg = egui::Color32::from_rgb(199, 210, 254);    // Light indigo
                let hyperlink = egui::Color32::from_rgb(37, 99, 235);         // Bright blue

                // Warm off-white backgrounds with subtle color
                let bg_main = egui::Color32::from_rgb(250, 250, 252);         // Slightly cool white
                let bg_panel = egui::Color32::from_rgb(245, 245, 250);        // Subtle lavender tint
                let bg_widget = egui::Color32::from_rgb(240, 240, 248);       // Light lavender

                // Text colors with good contrast
                let text_primary = egui::Color32::from_rgb(30, 30, 60);       // Dark blue-gray
                let text_muted = egui::Color32::from_rgb(100, 100, 130);      // Muted blue-gray

                // Apply the colors
                visuals.override_text_color = Some(text_primary);
                visuals.widgets.noninteractive.bg_fill = bg_panel;
                visuals.widgets.noninteractive.fg_stroke.color = text_muted;
                visuals.widgets.inactive.bg_fill = bg_widget;
                visuals.widgets.inactive.fg_stroke.color = text_primary;
                visuals.widgets.hovered.bg_fill = accent_hover;
                visuals.widgets.hovered.fg_stroke.color = egui::Color32::WHITE;
                visuals.widgets.active.bg_fill = accent_active;
                visuals.widgets.active.fg_stroke.color = egui::Color32::WHITE;
                visuals.widgets.open.bg_fill = accent_primary;
                visuals.widgets.open.fg_stroke.color = egui::Color32::WHITE;
                visuals.selection.bg_fill = selection_bg;
                visuals.selection.stroke.color = accent_primary;
                visuals.hyperlink_color = hyperlink;
                visuals.extreme_bg_color = bg_main;
                visuals.faint_bg_color = bg_panel;
                visuals.window_fill = bg_main;
                visuals.panel_fill = bg_panel;

                // Colorful window shadow
                visuals.window_shadow = egui::epaint::Shadow {
                    offset: egui::vec2(0.0, 4.0),
                    blur: 12.0,
                    spread: 0.0,
                    color: egui::Color32::from_rgba_unmultiplied(99, 102, 241, 25),
                };

                ctx.set_visuals(visuals);
            }
            Theme::HighContrastDark => {
                let mut visuals = egui::Visuals::dark();
                visuals.override_text_color = Some(egui::Color32::WHITE);
                visuals.widgets.noninteractive.bg_fill = egui::Color32::from_gray(20);
                visuals.widgets.inactive.bg_fill = egui::Color32::from_gray(30);
                visuals.widgets.hovered.bg_fill = egui::Color32::from_gray(50);
                visuals.widgets.active.bg_fill = egui::Color32::from_gray(60);
                visuals.selection.bg_fill = egui::Color32::from_rgb(0, 100, 200);
                visuals.extreme_bg_color = egui::Color32::BLACK;
                ctx.set_visuals(visuals);
            }
            Theme::SolarizedDark => {
                let mut visuals = egui::Visuals::dark();
                // Solarized dark colors
                let base03 = egui::Color32::from_rgb(0, 43, 54);
                let base02 = egui::Color32::from_rgb(7, 54, 66);
                let base01 = egui::Color32::from_rgb(88, 110, 117);
                let base0 = egui::Color32::from_rgb(131, 148, 150);
                let base1 = egui::Color32::from_rgb(147, 161, 161);
                let cyan = egui::Color32::from_rgb(42, 161, 152);

                visuals.override_text_color = Some(base0);
                visuals.widgets.noninteractive.bg_fill = base03;
                visuals.widgets.noninteractive.fg_stroke.color = base1;
                visuals.widgets.inactive.bg_fill = base02;
                visuals.widgets.hovered.bg_fill = base01;
                visuals.widgets.active.bg_fill = cyan;
                visuals.selection.bg_fill = cyan;
                visuals.extreme_bg_color = base03;
                visuals.faint_bg_color = base02;
                ctx.set_visuals(visuals);
            }
            Theme::Nord => {
                let mut visuals = egui::Visuals::dark();
                // Nord colors
                let nord0 = egui::Color32::from_rgb(46, 52, 64); // Polar Night
                let nord1 = egui::Color32::from_rgb(59, 66, 82);
                let nord2 = egui::Color32::from_rgb(67, 76, 94);
                let nord3 = egui::Color32::from_rgb(76, 86, 106);
                let nord4 = egui::Color32::from_rgb(216, 222, 233); // Snow Storm
                let nord8 = egui::Color32::from_rgb(136, 192, 208); // Frost
                let nord10 = egui::Color32::from_rgb(94, 129, 172);

                visuals.override_text_color = Some(nord4);
                visuals.widgets.noninteractive.bg_fill = nord0;
                visuals.widgets.noninteractive.fg_stroke.color = nord4;
                visuals.widgets.inactive.bg_fill = nord1;
                visuals.widgets.hovered.bg_fill = nord2;
                visuals.widgets.active.bg_fill = nord3;
                visuals.selection.bg_fill = nord10;
                visuals.hyperlink_color = nord8;
                visuals.extreme_bg_color = nord0;
                visuals.faint_bg_color = nord1;
                ctx.set_visuals(visuals);
            }
            Theme::Custom(custom_theme) => {
                custom_theme.apply(ctx);
            }
        }
    }

    /// Check if this is a custom theme
    fn is_custom(&self) -> bool {
        matches!(self, Theme::Custom(_))
    }

    /// Get the custom theme if this is one
    fn as_custom(&self) -> Option<&CustomTheme> {
        match self {
            Theme::Custom(t) => Some(t),
            _ => None,
        }
    }

    /// Get mutable custom theme if this is one
    fn as_custom_mut(&mut self) -> Option<&mut CustomTheme> {
        match self {
            Theme::Custom(t) => Some(t),
            _ => None,
        }
    }

    /// Get the title bar background color for this theme
    fn title_bar_bg(&self) -> egui::Color32 {
        match self {
            Theme::Custom(t) => t.title_bar_bg.to_egui(),
            Theme::Light => egui::Color32::from_rgb(220, 220, 225),
            Theme::VibrantLight => egui::Color32::from_rgb(230, 230, 248), // Lavender tint
            // Dark themes
            Theme::Dark => egui::Color32::from_rgb(45, 45, 50),
            Theme::HighContrastDark => egui::Color32::from_rgb(35, 35, 40),
            Theme::SolarizedDark => egui::Color32::from_rgb(7, 54, 66), // base02
            Theme::Nord => egui::Color32::from_rgb(59, 66, 82), // nord1
        }
    }

    /// Get the title bar text color for this theme (None = use default)
    fn title_bar_text(&self) -> Option<egui::Color32> {
        match self {
            Theme::Custom(t) => t.title_bar_text.map(|c| c.to_egui()),
            _ => None, // Use default heading color
        }
    }

    /// Get the title bar font size multiplier for this theme
    fn title_bar_font_size(&self) -> f32 {
        match self {
            Theme::Custom(t) => t.title_bar_font_size,
            _ => 1.0, // Default size
        }
    }
}

/// Categories for the theme editor
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeEditorCategory {
    #[default]
    Backgrounds,
    Text,
    Widgets,
    Selection,
    Borders,
    Spacing,
}

impl ThemeEditorCategory {
    fn label(&self) -> &'static str {
        match self {
            ThemeEditorCategory::Backgrounds => "Backgrounds",
            ThemeEditorCategory::Text => "Text",
            ThemeEditorCategory::Widgets => "Widgets",
            ThemeEditorCategory::Selection => "Selection",
            ThemeEditorCategory::Borders => "Borders & Rounding",
            ThemeEditorCategory::Spacing => "Spacing & Layout",
        }
    }

    fn all() -> &'static [ThemeEditorCategory] {
        &[
            ThemeEditorCategory::Backgrounds,
            ThemeEditorCategory::Text,
            ThemeEditorCategory::Widgets,
            ThemeEditorCategory::Selection,
            ThemeEditorCategory::Borders,
            ThemeEditorCategory::Spacing,
        ]
    }
}

/// Context/scope where a keybinding is active
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum KeyContext {
    /// Works everywhere in the application
    #[default]
    Global,
    /// Only in the requirements list panel (not when editing text)
    RequirementsList,
    /// Only when viewing requirement details
    DetailView,
    /// Only when in add/edit form
    Form,
}

impl KeyContext {
    fn label(&self) -> &'static str {
        match self {
            KeyContext::Global => "Global",
            KeyContext::RequirementsList => "Requirements List",
            KeyContext::DetailView => "Detail View",
            KeyContext::Form => "Form",
        }
    }

    fn all() -> &'static [KeyContext] {
        &[
            KeyContext::Global,
            KeyContext::RequirementsList,
            KeyContext::DetailView,
            KeyContext::Form,
        ]
    }
}

/// Actions that can be bound to keys
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyAction {
    NavigateUp,
    NavigateDown,
    NavigateUpVim,      // 'k' key - vim-style up
    NavigateDownVim,    // 'j' key - vim-style down
    Edit,
    ToggleExpand,
    Save,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    CycleTheme,
    NewRequirement,
    NewSiblingRequirement,
    NewChildRequirement,
    // Quick change actions
    OpenOwnerPicker,    // 'o' - fuzzy finder for owner
    OpenFeaturePicker,  // 'f' - fuzzy finder for feature
    OpenStatusPicker,   // 's' - status picker (existing, now formal)
    OpenPriorityPicker, // 'p' - priority picker (existing, now formal)
    OpenSprintPicker,   // 'S' (shift+s) - sprint assignment picker
    AddComment,         // 'c' - add comment
    ToggleLinksPanel,   // 'L' - show/toggle links
    // Deletion actions
    DeleteWithConfirm,  // 'd' - delete with confirmation
    DeleteImmediate,    // 'D' - delete immediately
    ArchiveToggle,      // 'a' - archive/unarchive
    // Search navigation
    NextSearchMatch,    // 'n' - next search match
    PrevSearchMatch,    // 'N' - previous search match
    ClearSearch,        // Esc - clear search
    SwitchToFilterMode, // '/' - switch search to filter mode
}

impl KeyAction {
    fn label(&self) -> &'static str {
        match self {
            KeyAction::NavigateUp => "Navigate Up (Arrow)",
            KeyAction::NavigateDown => "Navigate Down (Arrow)",
            KeyAction::NavigateUpVim => "Navigate Up (k)",
            KeyAction::NavigateDownVim => "Navigate Down (j)",
            KeyAction::Edit => "Edit Requirement",
            KeyAction::ToggleExpand => "Toggle Expand/Collapse",
            KeyAction::Save => "Save",
            KeyAction::ZoomIn => "Zoom In",
            KeyAction::ZoomOut => "Zoom Out",
            KeyAction::ZoomReset => "Reset Zoom",
            KeyAction::CycleTheme => "Cycle Theme",
            KeyAction::NewRequirement => "New Requirement (Smart)",
            KeyAction::NewSiblingRequirement => "New Sibling Requirement",
            KeyAction::NewChildRequirement => "New Child Requirement",
            KeyAction::OpenOwnerPicker => "Open Owner Picker",
            KeyAction::OpenFeaturePicker => "Open Feature Picker",
            KeyAction::OpenStatusPicker => "Open Status Picker",
            KeyAction::OpenPriorityPicker => "Open Priority Picker",
            KeyAction::OpenSprintPicker => "Assign to Sprint",
            KeyAction::AddComment => "Add Comment",
            KeyAction::ToggleLinksPanel => "Toggle Links Panel",
            KeyAction::DeleteWithConfirm => "Delete (with confirm)",
            KeyAction::DeleteImmediate => "Delete (immediate)",
            KeyAction::ArchiveToggle => "Archive/Unarchive",
            KeyAction::NextSearchMatch => "Next Search Match",
            KeyAction::PrevSearchMatch => "Previous Search Match",
            KeyAction::ClearSearch => "Clear Search",
            KeyAction::SwitchToFilterMode => "Switch to Filter Mode",
        }
    }

    /// Returns the default context for this action
    fn default_context(&self) -> KeyContext {
        match self {
            KeyAction::NavigateUp => KeyContext::RequirementsList,
            KeyAction::NavigateDown => KeyContext::RequirementsList,
            KeyAction::NavigateUpVim => KeyContext::RequirementsList,
            KeyAction::NavigateDownVim => KeyContext::RequirementsList,
            KeyAction::Edit => KeyContext::RequirementsList,
            KeyAction::ToggleExpand => KeyContext::RequirementsList,
            KeyAction::Save => KeyContext::Form,
            KeyAction::ZoomIn => KeyContext::Global,
            KeyAction::ZoomOut => KeyContext::Global,
            KeyAction::ZoomReset => KeyContext::Global,
            KeyAction::CycleTheme => KeyContext::Global,
            KeyAction::NewRequirement => KeyContext::Global,
            KeyAction::NewSiblingRequirement => KeyContext::RequirementsList,
            KeyAction::NewChildRequirement => KeyContext::RequirementsList,
            KeyAction::OpenOwnerPicker => KeyContext::RequirementsList,
            KeyAction::OpenFeaturePicker => KeyContext::RequirementsList,
            KeyAction::OpenStatusPicker => KeyContext::RequirementsList,
            KeyAction::OpenPriorityPicker => KeyContext::RequirementsList,
            KeyAction::OpenSprintPicker => KeyContext::RequirementsList,
            KeyAction::AddComment => KeyContext::RequirementsList,
            KeyAction::ToggleLinksPanel => KeyContext::RequirementsList,
            KeyAction::DeleteWithConfirm => KeyContext::RequirementsList,
            KeyAction::DeleteImmediate => KeyContext::RequirementsList,
            KeyAction::ArchiveToggle => KeyContext::RequirementsList,
            KeyAction::NextSearchMatch => KeyContext::RequirementsList,
            KeyAction::PrevSearchMatch => KeyContext::RequirementsList,
            KeyAction::ClearSearch => KeyContext::RequirementsList,
            KeyAction::SwitchToFilterMode => KeyContext::RequirementsList,
        }
    }

    fn all() -> &'static [KeyAction] {
        &[
            KeyAction::NavigateUp,
            KeyAction::NavigateDown,
            KeyAction::NavigateUpVim,
            KeyAction::NavigateDownVim,
            KeyAction::Edit,
            KeyAction::ToggleExpand,
            KeyAction::Save,
            KeyAction::ZoomIn,
            KeyAction::ZoomOut,
            KeyAction::ZoomReset,
            KeyAction::CycleTheme,
            KeyAction::NewRequirement,
            KeyAction::NewSiblingRequirement,
            KeyAction::NewChildRequirement,
            KeyAction::OpenOwnerPicker,
            KeyAction::OpenFeaturePicker,
            KeyAction::OpenStatusPicker,
            KeyAction::OpenPriorityPicker,
            KeyAction::OpenSprintPicker,
            KeyAction::AddComment,
            KeyAction::ToggleLinksPanel,
            KeyAction::DeleteWithConfirm,
            KeyAction::DeleteImmediate,
            KeyAction::ArchiveToggle,
            KeyAction::NextSearchMatch,
            KeyAction::PrevSearchMatch,
            KeyAction::ClearSearch,
            KeyAction::SwitchToFilterMode,
        ]
    }
}

/// A key binding with optional modifiers and context
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key_name: String, // Store as string for serialization
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    #[serde(default)]
    pub context: KeyContext,
}

impl KeyBinding {
    fn new(key: egui::Key, context: KeyContext) -> Self {
        Self {
            key_name: key_to_string(key).to_string(),
            ctrl: false,
            shift: false,
            alt: false,
            context,
        }
    }

    fn with_ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

    fn key(&self) -> Option<egui::Key> {
        string_to_key(&self.key_name)
    }

    fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        parts.push(&self.key_name);
        parts.join("+")
    }

    /// Check if the key combination is pressed (does not check context)
    fn key_matches(&self, ctx: &egui::Context) -> bool {
        let Some(key) = self.key() else { return false };
        ctx.input(|i| {
            let modifiers_match = i.modifiers.ctrl == self.ctrl
                && i.modifiers.shift == self.shift
                && i.modifiers.alt == self.alt;
            modifiers_match && i.key_pressed(key)
        })
    }

    /// Check if the binding matches given the current app context
    fn matches(&self, egui_ctx: &egui::Context, current_context: KeyContext) -> bool {
        // Global bindings work everywhere
        // RequirementsList bindings also work in DetailView since both show the list panel
        // Otherwise, context must match
        let context_matches = self.context == KeyContext::Global
            || self.context == current_context
            || (self.context == KeyContext::RequirementsList
                && current_context == KeyContext::DetailView);
        context_matches && self.key_matches(egui_ctx)
    }
}

fn key_to_string(key: egui::Key) -> &'static str {
    match key {
        egui::Key::ArrowUp => "Up",
        egui::Key::ArrowDown => "Down",
        egui::Key::ArrowLeft => "Left",
        egui::Key::ArrowRight => "Right",
        egui::Key::Enter => "Enter",
        egui::Key::Space => "Space",
        egui::Key::Tab => "Tab",
        egui::Key::Escape => "Escape",
        egui::Key::Backspace => "Backspace",
        egui::Key::Delete => "Delete",
        egui::Key::Home => "Home",
        egui::Key::End => "End",
        egui::Key::PageUp => "PageUp",
        egui::Key::PageDown => "PageDown",
        egui::Key::Plus => "Plus",
        egui::Key::Minus => "Minus",
        egui::Key::Equals => "Equals",
        egui::Key::Num0 => "0",
        egui::Key::Num1 => "1",
        egui::Key::Num2 => "2",
        egui::Key::Num3 => "3",
        egui::Key::Num4 => "4",
        egui::Key::Num5 => "5",
        egui::Key::Num6 => "6",
        egui::Key::Num7 => "7",
        egui::Key::Num8 => "8",
        egui::Key::Num9 => "9",
        egui::Key::A => "A",
        egui::Key::B => "B",
        egui::Key::C => "C",
        egui::Key::D => "D",
        egui::Key::E => "E",
        egui::Key::F => "F",
        egui::Key::G => "G",
        egui::Key::H => "H",
        egui::Key::I => "I",
        egui::Key::J => "J",
        egui::Key::K => "K",
        egui::Key::L => "L",
        egui::Key::M => "M",
        egui::Key::N => "N",
        egui::Key::O => "O",
        egui::Key::P => "P",
        egui::Key::Q => "Q",
        egui::Key::R => "R",
        egui::Key::S => "S",
        egui::Key::T => "T",
        egui::Key::U => "U",
        egui::Key::V => "V",
        egui::Key::W => "W",
        egui::Key::X => "X",
        egui::Key::Y => "Y",
        egui::Key::Z => "Z",
        egui::Key::F1 => "F1",
        egui::Key::F2 => "F2",
        egui::Key::F3 => "F3",
        egui::Key::F4 => "F4",
        egui::Key::F5 => "F5",
        egui::Key::F6 => "F6",
        egui::Key::F7 => "F7",
        egui::Key::F8 => "F8",
        egui::Key::F9 => "F9",
        egui::Key::F10 => "F10",
        egui::Key::F11 => "F11",
        egui::Key::F12 => "F12",
        _ => "?",
    }
}

fn string_to_key(s: &str) -> Option<egui::Key> {
    match s {
        "Up" => Some(egui::Key::ArrowUp),
        "Down" => Some(egui::Key::ArrowDown),
        "Left" => Some(egui::Key::ArrowLeft),
        "Right" => Some(egui::Key::ArrowRight),
        "Enter" => Some(egui::Key::Enter),
        "Space" => Some(egui::Key::Space),
        "Tab" => Some(egui::Key::Tab),
        "Escape" => Some(egui::Key::Escape),
        "Backspace" => Some(egui::Key::Backspace),
        "Delete" => Some(egui::Key::Delete),
        "Home" => Some(egui::Key::Home),
        "End" => Some(egui::Key::End),
        "PageUp" => Some(egui::Key::PageUp),
        "PageDown" => Some(egui::Key::PageDown),
        "Plus" => Some(egui::Key::Plus),
        "Minus" => Some(egui::Key::Minus),
        "Equals" => Some(egui::Key::Equals),
        "0" => Some(egui::Key::Num0),
        "1" => Some(egui::Key::Num1),
        "2" => Some(egui::Key::Num2),
        "3" => Some(egui::Key::Num3),
        "4" => Some(egui::Key::Num4),
        "5" => Some(egui::Key::Num5),
        "6" => Some(egui::Key::Num6),
        "7" => Some(egui::Key::Num7),
        "8" => Some(egui::Key::Num8),
        "9" => Some(egui::Key::Num9),
        "A" => Some(egui::Key::A),
        "B" => Some(egui::Key::B),
        "C" => Some(egui::Key::C),
        "D" => Some(egui::Key::D),
        "E" => Some(egui::Key::E),
        "F" => Some(egui::Key::F),
        "G" => Some(egui::Key::G),
        "H" => Some(egui::Key::H),
        "I" => Some(egui::Key::I),
        "J" => Some(egui::Key::J),
        "K" => Some(egui::Key::K),
        "L" => Some(egui::Key::L),
        "M" => Some(egui::Key::M),
        "N" => Some(egui::Key::N),
        "O" => Some(egui::Key::O),
        "P" => Some(egui::Key::P),
        "Q" => Some(egui::Key::Q),
        "R" => Some(egui::Key::R),
        "S" => Some(egui::Key::S),
        "T" => Some(egui::Key::T),
        "U" => Some(egui::Key::U),
        "V" => Some(egui::Key::V),
        "W" => Some(egui::Key::W),
        "X" => Some(egui::Key::X),
        "Y" => Some(egui::Key::Y),
        "Z" => Some(egui::Key::Z),
        "F1" => Some(egui::Key::F1),
        "F2" => Some(egui::Key::F2),
        "F3" => Some(egui::Key::F3),
        "F4" => Some(egui::Key::F4),
        "F5" => Some(egui::Key::F5),
        "F6" => Some(egui::Key::F6),
        "F7" => Some(egui::Key::F7),
        "F8" => Some(egui::Key::F8),
        "F9" => Some(egui::Key::F9),
        "F10" => Some(egui::Key::F10),
        "F11" => Some(egui::Key::F11),
        "F12" => Some(egui::Key::F12),
        _ => None,
    }
}

/// Collection of key bindings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBindings {
    pub bindings: HashMap<KeyAction, KeyBinding>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        let mut bindings = HashMap::new();
        // Use default_context() for each action
        bindings.insert(
            KeyAction::NavigateUp,
            KeyBinding::new(egui::Key::ArrowUp, KeyAction::NavigateUp.default_context()),
        );
        bindings.insert(
            KeyAction::NavigateDown,
            KeyBinding::new(
                egui::Key::ArrowDown,
                KeyAction::NavigateDown.default_context(),
            ),
        );
        bindings.insert(
            KeyAction::Edit,
            KeyBinding::new(egui::Key::Enter, KeyAction::Edit.default_context()),
        );
        bindings.insert(
            KeyAction::ToggleExpand,
            KeyBinding::new(egui::Key::Space, KeyAction::ToggleExpand.default_context()),
        );
        bindings.insert(
            KeyAction::Save,
            KeyBinding::new(egui::Key::S, KeyAction::Save.default_context()).with_ctrl(),
        );
        bindings.insert(
            KeyAction::ZoomIn,
            KeyBinding::new(egui::Key::Plus, KeyAction::ZoomIn.default_context())
                .with_ctrl()
                .with_shift(),
        );
        bindings.insert(
            KeyAction::ZoomOut,
            KeyBinding::new(egui::Key::Minus, KeyAction::ZoomOut.default_context()).with_ctrl(),
        );
        bindings.insert(
            KeyAction::ZoomReset,
            KeyBinding::new(egui::Key::Num0, KeyAction::ZoomReset.default_context()).with_ctrl(),
        );
        bindings.insert(
            KeyAction::CycleTheme,
            KeyBinding::new(egui::Key::T, KeyAction::CycleTheme.default_context()).with_ctrl(),
        );
        // Ctrl+N for smart new requirement (uses heuristic for parent selection)
        bindings.insert(
            KeyAction::NewRequirement,
            KeyBinding::new(egui::Key::N, KeyAction::NewRequirement.default_context()).with_ctrl(),
        );
        // 'n' in reqlist for new sibling requirement
        bindings.insert(
            KeyAction::NewSiblingRequirement,
            KeyBinding::new(egui::Key::N, KeyAction::NewSiblingRequirement.default_context()),
        );
        // 'N' (Shift+N) in reqlist for new child requirement
        bindings.insert(
            KeyAction::NewChildRequirement,
            KeyBinding::new(egui::Key::N, KeyAction::NewChildRequirement.default_context())
                .with_shift(),
        );
        // Vim-style navigation: 'j' for down, 'k' for up
        bindings.insert(
            KeyAction::NavigateDownVim,
            KeyBinding::new(egui::Key::J, KeyAction::NavigateDownVim.default_context()),
        );
        bindings.insert(
            KeyAction::NavigateUpVim,
            KeyBinding::new(egui::Key::K, KeyAction::NavigateUpVim.default_context()),
        );
        // Quick change actions
        bindings.insert(
            KeyAction::OpenOwnerPicker,
            KeyBinding::new(egui::Key::O, KeyAction::OpenOwnerPicker.default_context()),
        );
        bindings.insert(
            KeyAction::OpenFeaturePicker,
            KeyBinding::new(egui::Key::F, KeyAction::OpenFeaturePicker.default_context()),
        );
        bindings.insert(
            KeyAction::OpenStatusPicker,
            KeyBinding::new(egui::Key::S, KeyAction::OpenStatusPicker.default_context()),
        );
        bindings.insert(
            KeyAction::OpenPriorityPicker,
            KeyBinding::new(egui::Key::P, KeyAction::OpenPriorityPicker.default_context()),
        );
        // 'S' (Shift+S) for sprint assignment picker
        bindings.insert(
            KeyAction::OpenSprintPicker,
            KeyBinding::new(egui::Key::S, KeyAction::OpenSprintPicker.default_context())
                .with_shift(),
        );
        bindings.insert(
            KeyAction::AddComment,
            KeyBinding::new(egui::Key::C, KeyAction::AddComment.default_context()),
        );
        // 'L' (Shift+L) for toggle links panel
        bindings.insert(
            KeyAction::ToggleLinksPanel,
            KeyBinding::new(egui::Key::L, KeyAction::ToggleLinksPanel.default_context())
                .with_shift(),
        );
        // Deletion actions
        bindings.insert(
            KeyAction::DeleteWithConfirm,
            KeyBinding::new(egui::Key::D, KeyAction::DeleteWithConfirm.default_context()),
        );
        // 'D' (Shift+D) for immediate delete
        bindings.insert(
            KeyAction::DeleteImmediate,
            KeyBinding::new(egui::Key::D, KeyAction::DeleteImmediate.default_context())
                .with_shift(),
        );
        bindings.insert(
            KeyAction::ArchiveToggle,
            KeyBinding::new(egui::Key::A, KeyAction::ArchiveToggle.default_context()),
        );
        // Search navigation: n/N for next/prev match (vim-style)
        bindings.insert(
            KeyAction::NextSearchMatch,
            KeyBinding::new(egui::Key::N, KeyAction::NextSearchMatch.default_context()),
        );
        bindings.insert(
            KeyAction::PrevSearchMatch,
            KeyBinding::new(egui::Key::N, KeyAction::PrevSearchMatch.default_context())
                .with_shift(),
        );
        // Esc to clear search
        bindings.insert(
            KeyAction::ClearSearch,
            KeyBinding::new(egui::Key::Escape, KeyAction::ClearSearch.default_context()),
        );
        // '/' to switch to filter mode (vim-style search)
        bindings.insert(
            KeyAction::SwitchToFilterMode,
            KeyBinding::new(egui::Key::Slash, KeyAction::SwitchToFilterMode.default_context()),
        );
        Self { bindings }
    }
}

impl KeyBindings {
    #[allow(dead_code)]
    fn get(&self, action: KeyAction) -> Option<&KeyBinding> {
        self.bindings.get(&action)
    }

    /// Check if an action's keybinding is pressed in the given context
    fn is_pressed(
        &self,
        action: KeyAction,
        egui_ctx: &egui::Context,
        current_context: KeyContext,
    ) -> bool {
        self.bindings
            .get(&action)
            .map(|binding| binding.matches(egui_ctx, current_context))
            .unwrap_or(false)
    }

    /// Check if an action's keybinding is pressed (ignores context - for key capture)
    #[allow(dead_code)]
    fn is_key_pressed(&self, action: KeyAction, egui_ctx: &egui::Context) -> bool {
        self.bindings
            .get(&action)
            .map(|binding| binding.key_matches(egui_ctx))
            .unwrap_or(false)
    }
}

/// Icon configuration for status indicators
/// Maps status keywords to display icons
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusIconConfig {
    /// Map of status keyword (lowercase) to icon string
    pub icons: std::collections::HashMap<String, String>,
    /// Default icon for unknown statuses
    pub default_icon: String,
}

impl Default for StatusIconConfig {
    fn default() -> Self {
        let mut icons = std::collections::HashMap::new();
        // Unicode symbol markers
        icons.insert("draft".to_string(), "? ".to_string());
        icons.insert("approved".to_string(), "☆".to_string());
        icons.insert("confirmed".to_string(), "★".to_string());
        icons.insert("ready".to_string(), "◑".to_string());
        icons.insert("progress".to_string(), "⇒".to_string());
        icons.insert("implement".to_string(), "※".to_string());
        icons.insert("review".to_string(), "†".to_string());
        icons.insert("verified".to_string(), "‡".to_string());
        icons.insert("completed".to_string(), "✔".to_string());
        icons.insert("fixed".to_string(), "◐".to_string());
        icons.insert("done".to_string(), "☒".to_string());
        icons.insert("rejected".to_string(), "☒".to_string());
        icons.insert("closed".to_string(), "☒".to_string());
        icons.insert("backlog".to_string(), "☐".to_string());
        icons.insert("open".to_string(), "○".to_string());
        icons.insert("ai".to_string(), "✦".to_string());
        Self {
            icons,
            default_icon: "? ".to_string(),
        }
    }
}

impl StatusIconConfig {
    /// Get the icon for a status string
    pub fn get_icon(&self, status: &str) -> &str {
        let status_lower = status.to_lowercase();
        // Check for exact match first
        if let Some(icon) = self.icons.get(&status_lower) {
            return icon;
        }
        // Check if any key is contained in the status
        for (key, icon) in &self.icons {
            if status_lower.contains(key) {
                return icon;
            }
        }
        &self.default_icon
    }
}

/// Icon configuration for priority indicators
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorityIconConfig {
    /// Map of priority keyword (lowercase) to icon string
    pub icons: std::collections::HashMap<String, String>,
    /// Default icon for unknown priorities
    pub default_icon: String,
}

impl Default for PriorityIconConfig {
    fn default() -> Self {
        let mut icons = std::collections::HashMap::new();
        icons.insert("high".to_string(), "!!!".to_string());
        icons.insert("critical".to_string(), "!!!".to_string());
        icons.insert("medium".to_string(), "!!".to_string());
        icons.insert("low".to_string(), "!".to_string());
        icons.insert("trivial".to_string(), ".".to_string());
        Self {
            icons,
            default_icon: "".to_string(),
        }
    }
}

impl PriorityIconConfig {
    /// Get the icon for a priority string
    pub fn get_icon(&self, priority: &str) -> &str {
        let priority_lower = priority.to_lowercase();
        if let Some(icon) = self.icons.get(&priority_lower) {
            return icon;
        }
        for (key, icon) in &self.icons {
            if priority_lower.contains(key) {
                return icon;
            }
        }
        &self.default_icon
    }
}

/// KanBan card click action for opening detail modal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum KanBanClickAction {
    /// Right-click opens detail modal (default)
    #[default]
    RightClick,
    /// Single click opens detail modal
    SingleClick,
}

impl KanBanClickAction {
    pub fn label(&self) -> &'static str {
        match self {
            KanBanClickAction::RightClick => "Right Click",
            KanBanClickAction::SingleClick => "Single Click",
        }
    }
}

/// User settings for the GUI application
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    /// User's full name
    pub name: String,
    /// User's email address
    pub email: String,
    /// User's nickname/handle for @mentions in comments
    pub handle: String,
    /// Base font size in points
    #[serde(default = "default_font_size")]
    pub base_font_size: f32,
    /// UI title heading level (1-6, default 3)
    #[serde(default = "default_ui_heading_level")]
    pub ui_heading_level: u8,
    /// Preferred view perspective
    #[serde(default)]
    pub preferred_perspective: Perspective,
    /// Color theme
    #[serde(default)]
    pub theme: Theme,
    /// Key bindings
    #[serde(default)]
    pub keybindings: KeyBindings,
    /// Saved view presets
    #[serde(default)]
    pub view_presets: Vec<ViewPreset>,
    /// Saved custom themes
    #[serde(default)]
    pub custom_themes: Vec<CustomTheme>,
    /// Show status icons in requirements list
    #[serde(default)]
    pub show_status_icons: bool,
    /// Status icon configuration
    #[serde(default)]
    pub status_icons: StatusIconConfig,
    /// Priority icon configuration
    #[serde(default)]
    pub priority_icons: PriorityIconConfig,
    /// KanBan card click action for opening detail modal
    #[serde(default)]
    pub kanban_click_action: KanBanClickAction,
    /// Search behavior mode (highlight vs filter)
    #[serde(default)]
    pub search_mode: SearchMode,
    /// Whether search navigation wraps around
    #[serde(default = "default_wrap_search")]
    pub wrap_search: bool,
}

fn default_font_size() -> f32 {
    DEFAULT_FONT_SIZE
}

fn default_ui_heading_level() -> u8 {
    5 // H5 by default
}

fn default_wrap_search() -> bool {
    true // Wrap search by default
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            email: String::new(),
            handle: String::new(),
            base_font_size: DEFAULT_FONT_SIZE,
            ui_heading_level: default_ui_heading_level(),
            preferred_perspective: Perspective::default(),
            theme: Theme::default(),
            keybindings: KeyBindings::default(),
            view_presets: Vec::new(),
            custom_themes: Vec::new(),
            show_status_icons: false,
            status_icons: StatusIconConfig::default(),
            priority_icons: PriorityIconConfig::default(),
            kanban_click_action: KanBanClickAction::default(),
            search_mode: SearchMode::default(),
            wrap_search: default_wrap_search(),
        }
    }
}

impl UserSettings {
    /// Get the default settings file path
    fn settings_path() -> PathBuf {
        let config_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("aida");

        // Ensure the directory exists
        let _ = std::fs::create_dir_all(&config_dir);

        config_dir.join("aida_gui_settings.yaml")
    }

    /// Get the themes directory path
    pub fn themes_dir() -> PathBuf {
        let themes_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("aida")
            .join("themes");

        // Ensure the directory exists
        let _ = std::fs::create_dir_all(&themes_dir);

        themes_dir
    }

    /// Load themes from the themes directory
    fn load_file_themes() -> Vec<CustomTheme> {
        let themes_dir = Self::themes_dir();
        let mut themes = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&themes_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "yaml" || ext == "yml") {
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        if let Ok(theme) = serde_yaml::from_str::<CustomTheme>(&contents) {
                            themes.push(theme);
                        }
                    }
                }
            }
        }

        // Sort themes by name for consistent ordering
        themes.sort_by(|a, b| a.name.cmp(&b.name));
        themes
    }

    /// Save a theme to a file in the themes directory
    pub fn save_theme_to_file(theme: &CustomTheme) -> Result<PathBuf, String> {
        let themes_dir = Self::themes_dir();

        // Create a filename from the theme name (sanitize for filesystem)
        let filename = theme.name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect::<String>()
            .to_lowercase();

        let path = themes_dir.join(format!("{}.yaml", filename));

        let yaml = serde_yaml::to_string(theme)
            .map_err(|e| format!("Failed to serialize theme: {}", e))?;
        std::fs::write(&path, yaml)
            .map_err(|e| format!("Failed to write theme file: {}", e))?;

        Ok(path)
    }

    /// Delete a theme file from the themes directory
    pub fn delete_theme_file(theme_name: &str) -> Result<(), String> {
        let themes_dir = Self::themes_dir();

        // Try to find and delete the file
        if let Ok(entries) = std::fs::read_dir(&themes_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "yaml" || ext == "yml") {
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        if let Ok(theme) = serde_yaml::from_str::<CustomTheme>(&contents) {
                            if theme.name == theme_name {
                                std::fs::remove_file(&path)
                                    .map_err(|e| format!("Failed to delete theme file: {}", e))?;
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }

        Err(format!("Theme file for '{}' not found", theme_name))
    }

    /// Load settings from file, or return defaults if not found
    /// Also loads themes from the themes directory
    /// For new users, attempts to populate name/email from git config and environment
    pub fn load() -> Self {
        let path = Self::settings_path();
        let mut settings = if path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                serde_yaml::from_str(&contents).unwrap_or_default()
            } else {
                Self::default()
            }
        } else {
            Self::default()
        };

        // Migrate keybindings: ensure all KeyAction variants have bindings
        // This handles new actions added in updates - they get their default binding
        let default_bindings = KeyBindings::default();
        for action in KeyAction::all() {
            if !settings.keybindings.bindings.contains_key(action) {
                if let Some(binding) = default_bindings.bindings.get(action) {
                    settings.keybindings.bindings.insert(*action, binding.clone());
                }
            }
        }

        // For new users, try to get name/email from git config and environment
        if settings.name.is_empty() || settings.email.is_empty() {
            Self::populate_from_git_and_env(&mut settings);
        }

        // Load themes from files and merge with embedded custom themes
        let file_themes = Self::load_file_themes();
        for file_theme in file_themes {
            // Only add if not already present (embedded themes take precedence by name)
            if !settings.custom_themes.iter().any(|t| t.name == file_theme.name) {
                settings.custom_themes.push(file_theme);
            }
        }

        settings
    }

    /// Populate name and email from git config and environment variables
    fn populate_from_git_and_env(settings: &mut Self) {
        // Try to read from ~/.gitconfig
        if let Some(home) = dirs::home_dir() {
            let gitconfig_path = home.join(".gitconfig");
            if gitconfig_path.exists() {
                if let Ok(contents) = std::fs::read_to_string(&gitconfig_path) {
                    // Parse git config for user.name and user.email
                    let mut in_user_section = false;
                    for line in contents.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with('[') {
                            in_user_section = trimmed.to_lowercase().starts_with("[user");
                        } else if in_user_section {
                            if let Some(value) = trimmed.strip_prefix("name") {
                                if settings.name.is_empty() {
                                    let value = value.trim_start_matches(|c| c == ' ' || c == '=').trim();
                                    if !value.is_empty() {
                                        settings.name = value.to_string();
                                    }
                                }
                            } else if let Some(value) = trimmed.strip_prefix("email") {
                                if settings.email.is_empty() {
                                    let value = value.trim_start_matches(|c| c == ' ' || c == '=').trim();
                                    if !value.is_empty() {
                                        settings.email = value.to_string();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // If name is still empty, try USER or USERNAME environment variable
        if settings.name.is_empty() {
            if let Ok(user) = std::env::var("USER") {
                settings.name = user;
            } else if let Ok(username) = std::env::var("USERNAME") {
                settings.name = username;
            }
        }
    }

    /// Save settings to file
    pub fn save(&self) -> Result<(), String> {
        let path = Self::settings_path();
        let yaml = serde_yaml::to_string(self)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;
        std::fs::write(&path, yaml).map_err(|e| format!("Failed to write settings file: {}", e))?;
        Ok(())
    }

    /// Get the display name for use in comments/history
    /// Returns handle if set, otherwise name, otherwise "Unknown User"
    pub fn display_name(&self) -> String {
        if !self.handle.is_empty() {
            self.handle.clone()
        } else if !self.name.is_empty() {
            self.name.clone()
        } else {
            "Unknown User".to_string()
        }
    }
}

#[derive(Default, PartialEq, Clone)]
enum DetailTab {
    Ai,
    #[default]
    Description,
    Comments,
    Links,
    History,
}

#[derive(Default, PartialEq, Clone)]
enum SettingsTab {
    #[default]
    User,
    Appearance,
    Keybindings,
    IDs,
    Relationships,
    Reactions,
    TypeDefinitions,
    Members, // Replaced Users with Members (contains Users and Teams sub-tabs)
    Database,
    AiPrompts,
}

#[derive(Default, PartialEq, Clone, Copy)]
enum MembersSubTab {
    #[default]
    Users,
    Teams,
}

#[derive(Default, PartialEq, Clone, Copy)]
enum IdsSubTab {
    #[default]
    Format,
    Prefixes,
}

#[derive(Default, PartialEq, Clone, Copy)]
enum AiSubTab {
    #[default]
    Prompts,
    Skills,
}

#[derive(Default, PartialEq, Clone, Copy)]
enum FilterTab {
    #[default]
    Root,
    Children,
}

/// Field types for quick-change popup (triggered by keyboard shortcuts in list view)
#[derive(Debug, Clone, Copy, PartialEq)]
enum QuickChangeField {
    Status,
    Priority,
    Owner,
    Sprint,
    Feature,
}

/// AI action types - what kind of AI analysis to perform
#[derive(Debug, Clone)]
enum AiAction {
    /// Evaluate quality of a requirement
    Evaluate(Uuid),
    /// Find potential duplicate requirements
    FindDuplicates(Uuid),
    /// Suggest missing relationships
    SuggestRelationships(Uuid),
    /// Improve requirement description
    ImproveDescription(Uuid),
    /// Generate child requirements
    GenerateChildren(Uuid),
}

impl AiAction {
    fn name(&self) -> &'static str {
        match self {
            AiAction::Evaluate(_) => "Evaluate",
            AiAction::FindDuplicates(_) => "Find Duplicates",
            AiAction::SuggestRelationships(_) => "Suggest Relationships",
            AiAction::ImproveDescription(_) => "Improve Description",
            AiAction::GenerateChildren(_) => "Generate Children",
        }
    }

    fn requirement_id(&self) -> Uuid {
        match self {
            AiAction::Evaluate(id) => *id,
            AiAction::FindDuplicates(id) => *id,
            AiAction::SuggestRelationships(id) => *id,
            AiAction::ImproveDescription(id) => *id,
            AiAction::GenerateChildren(id) => *id,
        }
    }
}

/// AI result - stores the outcome of an AI action
#[derive(Debug, Clone)]
struct AiResult {
    action_name: String,
    success: bool,
    message: String,
    #[allow(dead_code)]
    details: Option<String>,
}

impl AiResult {
    fn placeholder(action: &AiAction) -> Self {
        Self {
            action_name: action.name().to_string(),
            success: true,
            message: format!(
                "AI {} is not yet implemented. See docs/AI_INTEGRATION_DESIGN.md for the roadmap.",
                action.name()
            ),
            details: None,
        }
    }
}

/// Search behavior mode
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SearchMode {
    /// Highlight matching requirements but show all (default)
    #[default]
    Highlight,
    /// Filter to show only matching requirements (current behavior)
    Filter,
}

impl SearchMode {
    fn label(&self) -> &'static str {
        match self {
            SearchMode::Highlight => "Highlight",
            SearchMode::Filter => "Filter",
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            SearchMode::Highlight => "☀", // sun for highlight (show all, highlight matches)
            SearchMode::Filter => "⋈",    // bowtie for filter (show only matches)
        }
    }
}

/// What fields to include in text search
#[derive(Default, PartialEq, Clone, Copy)]
struct SearchScope {
    title: bool,
    description: bool,
    comments: bool,
    spec_id: bool,
}

impl SearchScope {
    /// Default scope searches everything
    fn all() -> Self {
        Self {
            title: true,
            description: true,
            comments: true,
            spec_id: true,
        }
    }

    /// Check if all scopes are enabled (for "Everything" display)
    fn is_all(&self) -> bool {
        self.title && self.description && self.comments && self.spec_id
    }

    /// Check if no scopes are enabled
    fn is_none(&self) -> bool {
        !self.title && !self.description && !self.comments && !self.spec_id
    }
}

#[derive(Default, Debug, PartialEq, Clone)]
enum View {
    #[default]
    List,
    Detail,
    Add,
    Edit,
    OrgChart,  // Organization chart view for teams
    KanBan,    // KanBan board view
    Baselines, // Baseline management view
    Timeline,  // Timeline view showing requirement changes over time
    Planning,  // Sprint planning view for assigning items to Sprints
}

/// Layout mode defines the panel arrangement (cycles through 5 predefined layouts)
#[derive(Default, Debug, PartialEq, Clone, Copy)]
enum LayoutMode {
    /// List on left, Details on right (side-by-side) - default
    #[default]
    ListDetailsSide,
    /// List on top, Details on bottom (stacked)
    ListDetailsStacked,
    /// Two lists side-by-side on top, Details on bottom
    SplitListDetails,
    /// Two lists side-by-side, no details
    SplitListOnly,
    /// Single list only, no details
    ListOnly,
}

impl LayoutMode {
    /// Cycle to the next layout in sequence
    fn next(&self) -> Self {
        match self {
            LayoutMode::ListDetailsSide => LayoutMode::ListDetailsStacked,
            LayoutMode::ListDetailsStacked => LayoutMode::SplitListDetails,
            LayoutMode::SplitListDetails => LayoutMode::SplitListOnly,
            LayoutMode::SplitListOnly => LayoutMode::ListDetailsSide,
            LayoutMode::ListOnly => LayoutMode::ListDetailsSide, // Back to default
        }
    }

    fn label(&self) -> &'static str {
        match self {
            LayoutMode::ListDetailsSide => "List | Details",
            LayoutMode::ListDetailsStacked => "List / Details",
            LayoutMode::SplitListDetails => "List | List / Details",
            LayoutMode::SplitListOnly => "List | List",
            LayoutMode::ListOnly => "List",
        }
    }

    fn has_details(&self) -> bool {
        matches!(self, LayoutMode::ListDetailsSide | LayoutMode::ListDetailsStacked | LayoutMode::SplitListDetails)
    }

    fn has_split(&self) -> bool {
        matches!(self, LayoutMode::SplitListDetails | LayoutMode::SplitListOnly)
    }

    /// Get the layout to transition to when closing the details panel
    fn without_details(&self) -> Self {
        match self {
            LayoutMode::ListDetailsSide | LayoutMode::ListDetailsStacked => LayoutMode::ListOnly,
            LayoutMode::SplitListDetails => LayoutMode::SplitListOnly,
            LayoutMode::SplitListOnly => LayoutMode::SplitListOnly, // Already no details
            LayoutMode::ListOnly => LayoutMode::ListOnly, // Already no details
        }
    }

    /// Get the layout to transition to when closing the split panel
    fn without_split(&self) -> Self {
        match self {
            LayoutMode::SplitListDetails => LayoutMode::ListDetailsStacked,
            LayoutMode::SplitListOnly => LayoutMode::ListOnly,
            _ => *self, // Already no split
        }
    }

    /// Get all available layout modes for the menu
    fn all_modes() -> &'static [LayoutMode] {
        &[
            LayoutMode::ListDetailsSide,
            LayoutMode::ListDetailsStacked,
            LayoutMode::SplitListDetails,
            LayoutMode::SplitListOnly,
            LayoutMode::ListOnly,
        ]
    }

    /// Returns true if the details panel is displayed at the bottom (stacked),
    /// false if displayed on the side (right)
    fn is_details_stacked(&self) -> bool {
        matches!(self, LayoutMode::ListDetailsStacked | LayoutMode::SplitListDetails)
    }
}

/// Which list panel currently has focus for keyboard navigation
#[derive(Default, Debug, PartialEq, Clone, Copy)]
enum FocusedList {
    #[default]
    List1,  // Main requirements list
    List2,  // Split list (second list)
}

/// Perspective defines how requirements are organized in the list
#[derive(Debug, Default, PartialEq, Clone, Serialize, Deserialize)]
pub(crate) enum Perspective {
    /// Simple flat list of all requirements
    Flat,
    /// Tree view based on Parent/Child relationships (default)
    #[default]
    ParentChild,
    /// Tree view based on Verifies/VerifiedBy relationships
    Verification,
    /// Tree view based on References relationships
    References,
    /// Tree view with Users as roots, owned requirements as children
    ByOwner,
}

/// Sub-hierarchy mode for ByOwner perspective
#[derive(Debug, Default, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub(crate) enum OwnerViewMode {
    /// Flat: All owned requirements shown directly under user (2-level)
    #[default]
    Flat,
    /// Nested: Owned requirements preserve their parent/child hierarchy
    Nested,
}

/// KanBan swimlane field - what field determines the columns
#[derive(Debug, Default, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub(crate) enum KanBanField {
    #[default]
    Status,
    Priority,
    // Custom(String) - for custom enum fields, handled separately
}

impl KanBanField {
    fn label(&self) -> &'static str {
        match self {
            KanBanField::Status => "Status",
            KanBanField::Priority => "Priority",
        }
    }
}

/// KanBan column configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KanBanColumn {
    /// The value this column represents (e.g., "Draft", "In Progress")
    pub value: String,
    /// Display name for the column header
    pub label: String,
    /// Work-in-progress limit (0 = unlimited)
    pub wip_limit: usize,
}

impl Perspective {
    fn label(&self) -> &'static str {
        match self {
            Perspective::Flat => "Flat List",
            Perspective::ParentChild => "Parent/Child",
            Perspective::Verification => "Verification",
            Perspective::References => "References",
            Perspective::ByOwner => "By Owner",
        }
    }

    /// Get the relationship types used for this perspective
    fn relationship_types(&self) -> Option<(RelationshipType, RelationshipType)> {
        match self {
            Perspective::Flat => None,
            Perspective::ParentChild => Some((RelationshipType::Parent, RelationshipType::Child)),
            Perspective::Verification => {
                Some((RelationshipType::Verifies, RelationshipType::VerifiedBy))
            }
            Perspective::References => {
                Some((RelationshipType::References, RelationshipType::References))
            }
            Perspective::ByOwner => None, // ByOwner uses a different grouping mechanism
        }
    }

    /// Returns true if this perspective uses users as root nodes
    fn uses_user_roots(&self) -> bool {
        matches!(self, Perspective::ByOwner)
    }
}

/// Direction for viewing relationship hierarchies
#[derive(Debug, Default, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub(crate) enum PerspectiveDirection {
    /// View from parent/source to children/targets
    #[default]
    TopDown,
    /// View from children/targets to parents/sources
    BottomUp,
}

impl PerspectiveDirection {
    fn label(&self) -> &'static str {
        match self {
            PerspectiveDirection::TopDown => "Top-down",
            PerspectiveDirection::BottomUp => "Bottom-up",
        }
    }
}

/// A saved view preset combining perspective, direction, and filters
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewPreset {
    /// User-defined name for the preset
    pub name: String,
    /// The perspective (Flat, ParentChild, etc.)
    pub perspective: Perspective,
    /// Direction for tree views
    pub direction: PerspectiveDirection,
    /// Root filter by requirement types (empty = all)
    pub filter_types: Vec<String>,
    /// Root filter by features (empty = all)
    pub filter_features: Vec<String>,
    /// Root filter by ID prefixes (empty = all)
    #[serde(default)]
    pub filter_prefixes: Vec<String>,
    /// Child filter by requirement types (empty = all)
    #[serde(default)]
    pub child_filter_types: Vec<String>,
    /// Child filter by features (empty = all)
    #[serde(default)]
    pub child_filter_features: Vec<String>,
    /// Child filter by ID prefixes (empty = all)
    #[serde(default)]
    pub child_filter_prefixes: Vec<String>,
    /// Whether children use same filters as root
    #[serde(default = "default_true")]
    pub children_same_as_root: bool,
}

fn default_true() -> bool {
    true
}

impl ViewPreset {
    /// Create a new preset with the given name and current settings
    fn new(
        name: String,
        perspective: Perspective,
        direction: PerspectiveDirection,
        filter_types: &HashSet<RequirementType>,
        filter_features: &HashSet<String>,
        filter_prefixes: &HashSet<String>,
        child_filter_types: &HashSet<RequirementType>,
        child_filter_features: &HashSet<String>,
        child_filter_prefixes: &HashSet<String>,
        children_same_as_root: bool,
    ) -> Self {
        Self {
            name,
            perspective,
            direction,
            filter_types: filter_types.iter().map(|t| format!("{:?}", t)).collect(),
            filter_features: filter_features.iter().cloned().collect(),
            filter_prefixes: filter_prefixes.iter().cloned().collect(),
            child_filter_types: child_filter_types
                .iter()
                .map(|t| format!("{:?}", t))
                .collect(),
            child_filter_features: child_filter_features.iter().cloned().collect(),
            child_filter_prefixes: child_filter_prefixes.iter().cloned().collect(),
            children_same_as_root,
        }
    }

    /// Get filter_types as HashSet<RequirementType>
    fn get_filter_types(&self) -> HashSet<RequirementType> {
        Self::parse_types(&self.filter_types)
    }

    /// Get child_filter_types as HashSet<RequirementType>
    fn get_child_filter_types(&self) -> HashSet<RequirementType> {
        Self::parse_types(&self.child_filter_types)
    }

    fn parse_types(types: &[String]) -> HashSet<RequirementType> {
        types
            .iter()
            .filter_map(|s| match s.as_str() {
                "Functional" => Some(RequirementType::Functional),
                "NonFunctional" => Some(RequirementType::NonFunctional),
                "System" => Some(RequirementType::System),
                "User" => Some(RequirementType::User),
                "ChangeRequest" => Some(RequirementType::ChangeRequest),
                _ => None,
            })
            .collect()
    }

    /// Get filter_features as HashSet<String>
    fn get_filter_features(&self) -> HashSet<String> {
        self.filter_features.iter().cloned().collect()
    }

    /// Get child_filter_features as HashSet<String>
    fn get_child_filter_features(&self) -> HashSet<String> {
        self.child_filter_features.iter().cloned().collect()
    }

    /// Get filter_prefixes as HashSet<String>
    fn get_filter_prefixes(&self) -> HashSet<String> {
        self.filter_prefixes.iter().cloned().collect()
    }

    /// Get child_filter_prefixes as HashSet<String>
    fn get_child_filter_prefixes(&self) -> HashSet<String> {
        self.child_filter_prefixes.iter().cloned().collect()
    }
}

pub struct RequirementsApp {
    storage: Storage,
    store: RequirementsStore,
    current_view: View,
    selected_idx: Option<usize>,
    filter_text: String,
    search_scope: SearchScope,
    search_match_indices: Vec<usize>,      // Indices of requirements matching current search
    search_current_match: Option<usize>,   // Current position in search_match_indices
    active_tab: DetailTab,

    // Form state
    form_title: String,
    form_description: String,
    form_status: RequirementStatus,
    form_status_string: String, // Status as string (for custom type statuses)
    form_custom_fields: HashMap<String, String>, // Custom field values
    form_priority: RequirementPriority,
    form_priority_string: String, // Priority as string (for custom type priorities)
    form_type: RequirementType,
    form_owner: String,
    form_feature: String,
    form_tags: String,
    form_prefix: String, // Optional prefix override (uppercase letters only)
    form_parent_id: Option<Uuid>, // Parent to link new requirement to
    focus_description: bool, // Request focus on description field when entering Edit view
    form_title_auto_synced: bool, // True if title is being auto-synced from description (Add mode only)
    form_last_description: String, // Track previous description to detect changes

    // Messages
    message: Option<(String, bool)>, // (message, is_error)

    // Comment state
    comment_author: String,
    comment_content: String,
    show_add_comment: bool,
    reply_to_comment: Option<Uuid>, // Parent comment ID for replies
    collapsed_comments: HashMap<Uuid, bool>, // Track which comments are collapsed
    #[allow(dead_code)]
    edit_comment_id: Option<Uuid>,

    // Pending operations (to avoid borrow checker issues)
    pending_delete: Option<usize>,
    pending_view_change: Option<View>,
    pending_save: bool, // Save triggered by keybinding
    pending_comment_add: Option<(String, String, Option<Uuid>)>, // (author, content, parent_id)
    pending_comment_delete: Option<Uuid>,
    pending_reaction_toggle: Option<(Uuid, String)>, // (comment_id, reaction_name)
    show_reaction_picker: Option<Uuid>,              // Comment ID to show reaction picker for
    scroll_to_requirement: Option<Uuid>,             // Requirement ID to scroll into view (List 1)
    split_scroll_to_requirement: Option<Uuid>,       // Requirement ID to scroll into view (List 2)

    // Settings
    user_settings: UserSettings,
    show_settings_dialog: bool,
    settings_tab: SettingsTab,
    ids_subtab: IdsSubTab,
    ai_subtab: AiSubTab,
    // Skills loaded from .claude/skills/ directory
    loaded_skills: Vec<(String, String)>, // (filename, content)
    // Prefix rename dialog
    rename_prefix_from: String,
    rename_prefix_to: String,
    show_rename_prefix_dialog: bool,
    settings_form_name: String,
    settings_form_email: String,
    settings_form_handle: String,
    settings_form_font_size: f32,
    settings_form_ui_heading_level: u8,
    settings_form_perspective: Perspective,
    settings_form_theme: Theme,
    settings_form_keybindings: KeyBindings,
    capturing_key_for: Option<KeyAction>, // Which action we're capturing a key for
    settings_form_show_status_icons: bool,
    settings_form_status_icons: StatusIconConfig,
    settings_form_priority_icons: PriorityIconConfig,
    show_icon_editor: bool, // Whether to show the icon editor dialog
    icon_editor_new_keyword: String, // For adding new status/priority keywords
    icon_editor_new_icon: String, // Icon for new keyword
    show_symbol_picker: bool, // Whether to show the symbol picker popup
    symbol_picker_target: Option<String>, // Which field the symbol picker is targeting

    // Original appearance settings for Cancel reversion (live preview support)
    original_appearance_theme: Theme,
    original_appearance_font_size: f32,
    original_appearance_ui_heading_level: u8,
    original_appearance_show_status_icons: bool,
    original_appearance_status_icons: StatusIconConfig,
    original_appearance_priority_icons: PriorityIconConfig,

    // Project settings form fields
    settings_form_id_format: IdFormat,
    settings_form_numbering: NumberingStrategy,
    settings_form_digits: u8,
    show_migration_dialog: bool,
    pending_migration: Option<(IdFormat, NumberingStrategy, u8)>, // Format, Numbering, Digits

    // Theme editor
    show_theme_editor: bool,
    theme_editor_theme: CustomTheme,   // Working copy being edited
    theme_editor_category: ThemeEditorCategory, // Currently selected category
    theme_editor_original_theme: Theme, // Original theme before editing (for Cancel)

    // User management
    show_user_form: bool,
    editing_user_id: Option<Uuid>,
    user_form_name: String,
    user_form_email: String,
    user_form_handle: String,
    show_archived_users: bool,

    // Team management
    members_sub_tab: MembersSubTab,
    show_team_form: bool,
    editing_team_id: Option<Uuid>,
    team_form_name: String,
    team_form_description: String,
    team_form_parent_id: Option<Uuid>,
    show_archived_teams: bool,
    show_team_members_dialog: Option<Uuid>, // Team ID for member management dialog

    // Relationships view
    show_recursive_relationships: bool, // Toggle for recursive tree view
    relationship_tree_collapsed: HashMap<(Uuid, Uuid), bool>, // Track collapsed relationship tree nodes (source_id, target_id)

    // Font size (runtime, can differ from saved base)
    current_font_size: f32,

    // Perspective and filtering
    perspective: Perspective,
    perspective_direction: PerspectiveDirection,
    owner_view_mode: OwnerViewMode, // Sub-hierarchy mode for ByOwner perspective

    // KanBan view state
    kanban_field: KanBanField,                      // What field to use for columns
    kanban_filter_type: Option<RequirementType>,    // Filter to single type (None = all)
    kanban_wip_limits: HashMap<String, usize>,      // WIP limits per column value
    kanban_drag_card: Option<Uuid>,                 // Card being dragged
    kanban_drop_column: Option<String>,             // Column being hovered for drop
    kanban_detail_modal: Option<usize>,             // Index of requirement to show in detail modal

    filter_types: HashSet<RequirementType>, // Root filter: Empty = show all
    filter_features: HashSet<String>,       // Root filter: Empty = show all
    filter_prefixes: HashSet<String>,       // Root filter: Empty = show all
    filter_statuses: HashSet<RequirementStatus>, // Root filter: Empty = show all
    filter_priorities: HashSet<RequirementPriority>, // Root filter: Empty = show all
    // Child filters (when children_same_as_root is false)
    child_filter_types: HashSet<RequirementType>,
    child_filter_features: HashSet<String>,
    child_filter_prefixes: HashSet<String>,
    child_filter_statuses: HashSet<RequirementStatus>,
    child_filter_priorities: HashSet<RequirementPriority>,
    children_same_as_root: bool, // When true, children use same filters as root
    filter_tab: FilterTab,       // Which filter tab is active (Root or Children)
    tree_collapsed: HashMap<Uuid, bool>, // Track collapsed tree nodes
    show_filter_panel: bool,     // Toggle filter panel visibility
    show_archived: bool,         // Whether to show archived requirements
    show_filtered_parents: bool, // Show greyed-out parents of filtered items in tree view

    // Drag and drop for relationships
    drag_source: Option<usize>, // Index of requirement being dragged
    drop_target: Option<usize>, // Index of requirement being hovered over
    pending_relationship: Option<(usize, usize)>, // (source_idx, target_idx) to create relationship
    pending_apply_ai_description: Option<(Uuid, String)>, // (req_id, new_description) to apply from AI suggestion
    drag_scroll_delta: f32,     // Accumulated scroll delta during drag (for auto-scroll)

    // Markdown rendering
    markdown_cache: CommonMarkCache,
    show_description_preview: bool, // Toggle preview mode in edit form

    // Layout state
    left_panel_collapsed: bool,  // Whether left panel is manually collapsed (in form view)
    layout_mode: LayoutMode,     // Current layout mode (cycles through 5 layouts)
    layout_button_press_start: Option<std::time::Instant>,  // When layout button was pressed
    show_layout_menu: bool,      // Whether to show the layout selection menu
    layout_button_rect: Option<egui::Rect>,  // Position of layout button for popup menu

    // Quick change popup (triggered by keyboard shortcuts in list view: 's' for status, 'p' for priority, 'o' for owner)
    quick_change_field: Option<QuickChangeField>, // Which field is being changed (None = popup closed)
    quick_change_selected: usize,                 // Currently highlighted option index in popup
    quick_change_target_id: Option<Uuid>,         // Requirement being modified
    quick_change_consumed_action: bool,           // True if popup consumed a key this frame (prevents pass-through)
    quick_change_owner_search: String,            // Search text for owner fuzzy finder
    quick_change_feature_search: String,          // Search text for feature fuzzy finder

    // Delete confirmation state
    pending_delete_confirm: Option<usize>,        // Index of requirement awaiting delete confirmation

    // Split panel (second requirements list) - used in split layouts
    split_perspective: Perspective,
    split_perspective_direction: PerspectiveDirection,
    split_filter_text: String,
    split_filter_types: HashSet<RequirementType>,
    split_filter_features: HashSet<String>,
    split_filter_prefixes: HashSet<String>,
    split_filter_statuses: HashSet<RequirementStatus>,
    split_filter_priorities: HashSet<RequirementPriority>,
    split_show_filter_panel: bool,
    split_tree_collapsed: HashMap<Uuid, bool>,
    split_selected_idx: Option<usize>,
    split_active_preset: Option<String>,
    focused_list: FocusedList,  // Which list has focus for keyboard navigation
    navigation_locked: bool,    // Lock navigation between both lists (sync selection)
    scroll_to_center: bool,     // When true, scroll to center the selected item

    // Filter dialogs (modal)
    show_filter_dialog_list1: bool,
    show_filter_dialog_list2: bool,

    // Relationship definition editing
    editing_rel_def: Option<String>, // Name of relationship def being edited (None = adding new)
    rel_def_form_name: String,
    rel_def_form_display_name: String,
    rel_def_form_description: String,
    rel_def_form_inverse: String,
    rel_def_form_symmetric: bool,
    rel_def_form_cardinality: Cardinality,
    rel_def_form_source_types: String, // Comma-separated
    rel_def_form_target_types: String, // Comma-separated
    rel_def_form_color: String,
    show_rel_def_form: bool,

    // View presets
    active_preset: Option<String>, // Name of currently active preset (None = custom/unsaved)
    show_save_preset_dialog: bool, // Show the save preset dialog
    show_split_save_preset_dialog: bool, // Show the save preset dialog for List 2
    preset_name_input: String,     // Name input for new preset
    show_delete_preset_confirm: Option<String>, // Name of preset to confirm deletion

    // Keybinding context
    current_key_context: KeyContext, // Current context for keybinding checks

    // Reaction definition editing
    editing_reaction_def: Option<String>, // Name of reaction def being edited (None = adding new)
    reaction_def_form_name: String,
    reaction_def_form_emoji: String,
    reaction_def_form_label: String,
    reaction_def_form_description: String,
    show_reaction_def_form: bool,

    // Prefix management
    new_prefix_input: String, // Input for adding new allowed prefix

    // Type definition editing
    editing_type_def: Option<String>, // Name of type def being edited (None = adding new)
    type_def_form_name: String,
    type_def_form_display_name: String,
    type_def_form_description: String,
    type_def_form_prefix: String,
    type_def_form_statuses: Vec<String>, // Editable list of statuses
    type_def_form_priorities: Vec<String>, // Editable list of priorities
    type_def_form_fields: Vec<CustomFieldDefinition>, // Editable list of custom fields
    show_type_def_form: bool,
    new_status_input: String, // Input for adding new status
    new_priority_input: String, // Input for adding new priority
    // Custom field form (for adding/editing fields within type)
    editing_field_idx: Option<usize>, // Index of field being edited (None = adding new)
    field_form_name: String,
    field_form_label: String,
    field_form_type: CustomFieldType,
    field_form_required: bool,
    field_form_options: String, // Comma-separated for Select type
    field_form_default: String,
    show_field_form: bool,

    // URL link editing
    show_url_form: bool,
    editing_url_id: Option<Uuid>, // None = adding new, Some = editing existing
    url_form_url: String,
    url_form_title: String,
    url_form_description: String,
    url_verification_status: Option<(bool, String)>, // (success, message)
    url_verification_in_progress: bool,

    // Markdown help modal
    show_markdown_help: bool,

    // Reference picker modal (for adding References relationships)
    show_reference_picker: bool,
    reference_picker_search: String,
    reference_picker_selected: Option<Uuid>, // Selected requirement in the picker

    // Context menu state - stores selection before right-click clears it
    last_text_selection: Option<TextSelection>,

    // Project management
    show_new_project_dialog: bool,
    new_project_dir: String,
    new_project_name: String,
    new_project_title: String,
    new_project_description: String,
    new_project_template: String, // "current" or template name
    new_project_include_users: bool,
    show_switch_project_dialog: bool,
    available_projects: Vec<(String, String, String)>, // (name, path, description)

    // Form cancel confirmation
    show_cancel_confirm_dialog: bool, // Show confirmation dialog when canceling with unsaved changes
    original_form_title: String,      // Original values when entering Add/Edit mode
    original_form_description: String,
    original_form_status_string: String,
    original_form_priority: RequirementPriority,
    original_form_priority_string: String,
    original_form_type: RequirementType,
    original_form_owner: String,
    original_form_feature: String,
    form_feature_selected_idx: usize,  // Selected index in feature dropdown
    original_form_tags: String,
    original_form_prefix: String,
    original_form_custom_fields: HashMap<String, String>,

    // AI integration state
    ai_client: AiClient,                          // AI client for Claude CLI integration
    ai_pending_action: Option<AiAction>,          // Currently running AI action
    ai_last_result: Option<AiResult>,             // Result from last AI action
    show_ai_results_panel: bool,                  // Whether to show AI results panel
    ai_loading: bool,                             // True while AI request is in progress

    // Background AI evaluation state (for non-blocking evaluation)
    ai_eval_receiver: Option<mpsc::Receiver<BackgroundAiResult>>, // Channel to receive results
    ai_eval_in_progress: Option<(Uuid, String)>,  // (req_id, spec_id) of req being evaluated

    // Background Find Duplicates state (for non-blocking duplicate detection)
    find_duplicates_receiver: Option<mpsc::Receiver<BackgroundFindDuplicatesResult>>,
    find_duplicates_in_progress: Option<(Uuid, String)>,  // (req_id, spec_id) of req being checked

    // Toast notification state
    toast_message: Option<ToastNotification>,     // Current toast message to display

    // Project scaffolding state (FR-0152)
    show_scaffold_dialog: bool,                   // Whether to show the scaffolding dialog
    scaffold_config: aida_core::ScaffoldConfig,   // Scaffolding configuration
    scaffold_preview: Option<aida_core::ScaffoldPreview>, // Preview of artifacts to generate
    scaffold_tech_stack_input: String,            // Input for adding tech stack items

    // Conflict detection state (FR-0153)
    original_timestamps: HashMap<Uuid, DateTime<Utc>>,  // Requirement timestamps when loaded
    modified_requirement_ids: HashSet<Uuid>,            // IDs of requirements modified since load
    show_conflict_dialog: bool,                         // Whether to show conflict resolution dialog
    current_conflict: Option<ConflictInfo>,             // Current conflict being resolved

    // Session tracking state (collaborative awareness)
    session_id: String,                                 // Unique session ID for this instance
    heartbeat_sender: Option<std::sync::mpsc::Sender<HeartbeatCommand>>,  // Channel to heartbeat thread
    last_lock_info: Option<LockFileInfo>,               // Last known lock file state
    other_sessions_warning_shown: bool,                 // Whether we've shown the concurrent users warning

    // Baseline management state
    show_create_baseline_dialog: bool,
    baseline_form_name: String,
    baseline_form_description: String,
    selected_baseline_id: Option<Uuid>,                 // Currently selected baseline for viewing
    baseline_compare_source: Option<Uuid>,              // Source baseline for comparison
    baseline_compare_target: Option<Uuid>,              // Target baseline (None = current state)
    show_baseline_comparison: bool,                     // Whether to show comparison view

    // Clone requirement state
    show_clone_dialog: bool,
    clone_source_idx: Option<usize>,                    // Index of requirement being cloned
    clone_include_tags: bool,
    clone_include_relationships: bool,
    clone_include_comments: bool,
    clone_include_history: bool,
    clone_include_urls: bool,
    clone_include_custom_fields: bool,

    // Timeline view state
    timeline_selected_date: Option<chrono::DateTime<chrono::Utc>>,  // Currently selected point in timeline
    timeline_events: Vec<TimelineEvent>,                            // Cached timeline events
    timeline_filter_author: String,                                  // Filter by author
    timeline_filter_field: String,                                   // Filter by field name (e.g., "status")
    timeline_selected_event_idx: Option<usize>,                     // Currently selected event
    timeline_suppress_hover: bool,                                   // Suppress hover highlight after click/keyboard nav
    timeline_suppress_hover_pos: Option<egui::Pos2>,                // Mouse position when hover was suppressed

    // Planning view state
    planning_collapsed_sprints: std::collections::HashSet<Uuid>,     // Collapsed sprint sections
    planning_show_completed_sprints: bool,                           // Show archived/completed sprints
    planning_selected_item: Option<Uuid>,                            // Currently selected item for details
    planning_drag_source: Option<Uuid>,                              // Item being dragged
    planning_drag_target_sprint: Option<Option<Uuid>>,               // Target sprint (None = Backlog)
}

/// A timeline event representing a change to a requirement
#[derive(Clone, Debug)]
struct TimelineEvent {
    /// When the event occurred
    timestamp: chrono::DateTime<chrono::Utc>,
    /// Type of event (Created, Modified, Comment, etc.)
    event_type: TimelineEventType,
    /// ID of the requirement this event belongs to
    req_id: Uuid,
    /// Spec ID of the requirement (for display)
    spec_id: String,
    /// Title of the requirement at time of event
    req_title: String,
    /// Author of the change
    author: String,
    /// Description of what changed
    description: String,
    /// Field changes (for modification events)
    changes: Vec<aida_core::FieldChange>,
}

/// Types of timeline events
#[derive(Clone, Debug, PartialEq, Eq)]
enum TimelineEventType {
    /// Requirement was created
    Created,
    /// Requirement was modified
    Modified,
    /// Comment was added
    CommentAdded,
    /// Baseline was created
    BaselineCreated,
}

impl TimelineEventType {
    fn icon(&self) -> &'static str {
        match self {
            TimelineEventType::Created => "✨",
            TimelineEventType::Modified => "✏️",
            TimelineEventType::CommentAdded => "💬",
            TimelineEventType::BaselineCreated => "🏷",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            TimelineEventType::Created => "Created",
            TimelineEventType::Modified => "Modified",
            TimelineEventType::CommentAdded => "Comment",
            TimelineEventType::BaselineCreated => "Baseline",
        }
    }
}

/// Result from background AI evaluation thread
struct BackgroundAiResult {
    req_id: Uuid,
    spec_id: String,
    content_hash: String,
    result: Result<EvaluationResponse, String>,
}

/// Result from background Find Duplicates thread
struct BackgroundFindDuplicatesResult {
    req_id: Uuid,
    spec_id: String,
    result: Result<Vec<DuplicateInfo>, String>,
}

/// Info about a potential duplicate (simplified for display)
#[derive(Clone)]
struct DuplicateInfo {
    spec_id: String,
    similarity: f64,
    reason: String,
    recommendation: String,
}

/// Toast notification for displaying temporary messages
struct ToastNotification {
    message: String,
    is_success: bool,
    show_until: Instant,
}

/// Stores text selection state for context menu operations.
///
/// This is part of the workaround for egui clearing selection on right-click.
/// See the TEXT EDIT CONTEXT MENU IMPLEMENTATION documentation at the top of this file.
///
/// The selection is captured continuously while a TextEdit has focus, and stored
/// here so it's available when the context menu opens (after egui has cleared the
/// actual selection).
#[derive(Clone, Default)]
struct TextSelection {
    /// The selected text content
    text: String,
    /// Start index (in chars, not bytes)
    start: usize,
    /// End index (in chars, not bytes)
    end: usize,
    /// Which widget this selection came from (to avoid using stale selections)
    widget_id: Option<egui::Id>,
}

impl RequirementsApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::new_with_file(cc, None)
    }

    pub fn new_with_file(cc: &eframe::CreationContext<'_>, file_path: Option<String>) -> Self {
        // Configure fonts with better Unicode support
        Self::configure_fonts(&cc.egui_ctx);

        // Configure heading styles for markdown rendering
        // egui_commonmark uses named text styles: "Heading", "Heading2", "Heading3", etc.
        {
            let mut style = (*cc.egui_ctx.style()).clone();
            let base_size = style
                .text_styles
                .get(&egui::TextStyle::Body)
                .map(|f| f.size)
                .unwrap_or(14.0);

            // Set distinct sizes for each heading level
            style.text_styles.insert(
                egui::TextStyle::Name("Heading".into()),
                egui::FontId::new(base_size * 1.8, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Name("Heading2".into()),
                egui::FontId::new(base_size * 1.5, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Name("Heading3".into()),
                egui::FontId::new(base_size * 1.25, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Name("Heading4".into()),
                egui::FontId::new(base_size * 1.1, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Name("Heading5".into()),
                egui::FontId::new(base_size * 1.0, egui::FontFamily::Proportional),
            );
            style.text_styles.insert(
                egui::TextStyle::Name("Heading6".into()),
                egui::FontId::new(base_size * 0.9, egui::FontFamily::Proportional),
            );
            cc.egui_ctx.set_style(style);
        }

        // Use provided file path, or determine from environment/current directory
        let requirements_path = if let Some(fp) = file_path {
            std::path::PathBuf::from(fp)
        } else {
            determine_requirements_path(None)
                .unwrap_or_else(|_| std::path::PathBuf::from("requirements.yaml"))
        };

        let storage = Storage::new(&requirements_path);
        let store = match storage.load() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("ERROR: Failed to load requirements from {:?}: {}", requirements_path, e);
                eprintln!("Creating empty store. Your data may still be in the file.");
                RequirementsStore::new()
            }
        };
        let user_settings = UserSettings::load();

        // Extract project settings before store is moved
        let initial_id_format = store.id_config.format.clone();
        let initial_numbering = store.id_config.numbering.clone();
        let initial_digits = store.id_config.digits;

        // Capture timestamps for conflict detection (FR-0153)
        let initial_timestamps = Storage::get_requirement_timestamps(&store);

        // Apply saved preferences
        let initial_font_size = user_settings.base_font_size;
        let initial_perspective = user_settings.preferred_perspective.clone();

        // Generate unique session ID and register session (collaborative awareness)
        let session_id = format!("{}-{}", std::process::id(), Uuid::new_v4());
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let session_info = SessionInfo {
            session_id: session_id.clone(),
            user_name: user_settings.display_name(),
            hostname,
            pid: std::process::id(),
            started_at: Utc::now(),
            last_heartbeat: Utc::now(),
            editing_requirement: None,
        };

        // Register session and get initial lock info
        let initial_lock_info = storage.register_session(session_info).ok();

        // Start heartbeat thread
        let (heartbeat_sender, heartbeat_receiver) = mpsc::channel::<HeartbeatCommand>();
        let heartbeat_storage = Storage::new(storage.path());
        let heartbeat_session_id = session_id.clone();

        thread::spawn(move || {
            let mut current_edit_lock: Option<EditLock> = None;

            loop {
                // Check for commands (non-blocking with timeout)
                match heartbeat_receiver.recv_timeout(std::time::Duration::from_secs(5)) {
                    Ok(HeartbeatCommand::SetEditLock(lock)) => {
                        current_edit_lock = lock;
                    }
                    Ok(HeartbeatCommand::Shutdown) => {
                        // Unregister session on shutdown
                        let _ = heartbeat_storage.unregister_session(&heartbeat_session_id);
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Normal timeout - update heartbeat
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        // Channel closed - cleanup and exit
                        let _ = heartbeat_storage.unregister_session(&heartbeat_session_id);
                        break;
                    }
                }

                // Update heartbeat
                let _ = heartbeat_storage.update_heartbeat(
                    &heartbeat_session_id,
                    current_edit_lock.clone(),
                );
            }
        });

        Self {
            storage,
            store,
            current_view: View::List,
            selected_idx: None,
            filter_text: String::new(),
            search_scope: SearchScope::all(),
            search_match_indices: Vec::new(),
            search_current_match: None,
            active_tab: DetailTab::Description,
            form_title: String::new(),
            form_description: String::new(),
            form_status: RequirementStatus::Draft,
            form_status_string: String::from("Draft"),
            form_custom_fields: HashMap::new(),
            form_priority: RequirementPriority::Medium,
            form_priority_string: String::from("Medium"),
            form_type: RequirementType::Functional,
            form_owner: String::new(),
            form_feature: String::from("Uncategorized"),
            form_tags: String::new(),
            form_prefix: String::new(),
            form_parent_id: None,
            focus_description: false,
            form_title_auto_synced: true, // Start with auto-sync enabled for new requirements
            form_last_description: String::new(),
            message: None,
            comment_author: String::new(),
            comment_content: String::new(),
            show_add_comment: false,
            reply_to_comment: None,
            collapsed_comments: HashMap::new(),
            edit_comment_id: None,
            pending_delete: None,
            pending_view_change: None,
            pending_save: false,
            pending_comment_add: None,
            pending_comment_delete: None,
            pending_reaction_toggle: None,
            show_reaction_picker: None,
            scroll_to_requirement: None,
            split_scroll_to_requirement: None,
            current_font_size: initial_font_size,
            user_settings,
            show_settings_dialog: false,
            settings_tab: SettingsTab::default(),
            ids_subtab: IdsSubTab::default(),
            ai_subtab: AiSubTab::default(),
            loaded_skills: Vec::new(),
            rename_prefix_from: String::new(),
            rename_prefix_to: String::new(),
            show_rename_prefix_dialog: false,
            settings_form_name: String::new(),
            settings_form_email: String::new(),
            settings_form_handle: String::new(),
            settings_form_font_size: DEFAULT_FONT_SIZE,
            settings_form_ui_heading_level: default_ui_heading_level(),
            settings_form_perspective: Perspective::default(),
            settings_form_theme: Theme::default(),
            settings_form_keybindings: KeyBindings::default(),
            capturing_key_for: None,
            settings_form_show_status_icons: false,
            settings_form_status_icons: StatusIconConfig::default(),
            settings_form_priority_icons: PriorityIconConfig::default(),
            show_icon_editor: false,
            icon_editor_new_keyword: String::new(),
            icon_editor_new_icon: String::new(),
            show_symbol_picker: false,
            symbol_picker_target: None,
            original_appearance_theme: Theme::default(),
            original_appearance_font_size: DEFAULT_FONT_SIZE,
            original_appearance_ui_heading_level: default_ui_heading_level(),
            original_appearance_show_status_icons: false,
            original_appearance_status_icons: StatusIconConfig::default(),
            original_appearance_priority_icons: PriorityIconConfig::default(),
            settings_form_id_format: initial_id_format,
            settings_form_numbering: initial_numbering,
            settings_form_digits: initial_digits,
            show_migration_dialog: false,
            pending_migration: None,
            show_theme_editor: false,
            theme_editor_theme: CustomTheme::default(),
            theme_editor_category: ThemeEditorCategory::default(),
            theme_editor_original_theme: Theme::default(),
            show_user_form: false,
            editing_user_id: None,
            user_form_name: String::new(),
            user_form_email: String::new(),
            user_form_handle: String::new(),
            show_archived_users: false,
            members_sub_tab: MembersSubTab::default(),
            show_team_form: false,
            editing_team_id: None,
            team_form_name: String::new(),
            team_form_description: String::new(),
            team_form_parent_id: None,
            show_archived_teams: false,
            show_team_members_dialog: None,
            show_recursive_relationships: false,
            relationship_tree_collapsed: HashMap::new(),
            perspective: initial_perspective,
            perspective_direction: PerspectiveDirection::default(),
            owner_view_mode: OwnerViewMode::default(),
            kanban_field: KanBanField::default(),
            kanban_filter_type: None,
            kanban_wip_limits: HashMap::new(),
            kanban_drag_card: None,
            kanban_drop_column: None,
            kanban_detail_modal: None,
            filter_types: HashSet::new(),
            filter_features: HashSet::new(),
            filter_prefixes: HashSet::new(),
            filter_statuses: HashSet::new(),
            filter_priorities: HashSet::new(),
            child_filter_types: HashSet::new(),
            child_filter_features: HashSet::new(),
            child_filter_prefixes: HashSet::new(),
            child_filter_statuses: HashSet::new(),
            child_filter_priorities: HashSet::new(),
            children_same_as_root: true,
            filter_tab: FilterTab::Root,
            tree_collapsed: HashMap::new(),
            show_filter_panel: false,
            show_archived: false,
            show_filtered_parents: true, // Default to showing parents
            drag_source: None,
            drop_target: None,
            pending_relationship: None,
            pending_apply_ai_description: None,
            drag_scroll_delta: 0.0,
            markdown_cache: CommonMarkCache::default(),
            show_description_preview: false,
            left_panel_collapsed: false,
            layout_mode: LayoutMode::ListDetailsSide,
            layout_button_press_start: None,
            show_layout_menu: false,
            layout_button_rect: None,
            quick_change_field: None,
            quick_change_selected: 0,
            quick_change_target_id: None,
            quick_change_consumed_action: false,
            quick_change_owner_search: String::new(),
            quick_change_feature_search: String::new(),
            pending_delete_confirm: None,
            split_perspective: Perspective::default(),
            split_perspective_direction: PerspectiveDirection::default(),
            split_filter_text: String::new(),
            split_filter_types: HashSet::new(),
            split_filter_features: HashSet::new(),
            split_filter_prefixes: HashSet::new(),
            split_filter_statuses: HashSet::new(),
            split_filter_priorities: HashSet::new(),
            split_show_filter_panel: false,
            split_tree_collapsed: HashMap::new(),
            split_selected_idx: None,
            split_active_preset: None,
            focused_list: FocusedList::default(),
            navigation_locked: false,
            scroll_to_center: false,
            show_filter_dialog_list1: false,
            show_filter_dialog_list2: false,
            editing_rel_def: None,
            rel_def_form_name: String::new(),
            rel_def_form_display_name: String::new(),
            rel_def_form_description: String::new(),
            rel_def_form_inverse: String::new(),
            rel_def_form_symmetric: false,
            rel_def_form_cardinality: Cardinality::default(),
            rel_def_form_source_types: String::new(),
            rel_def_form_target_types: String::new(),
            rel_def_form_color: String::new(),
            show_rel_def_form: false,
            active_preset: None,
            show_save_preset_dialog: false,
            show_split_save_preset_dialog: false,
            preset_name_input: String::new(),
            show_delete_preset_confirm: None,
            current_key_context: KeyContext::RequirementsList,
            editing_reaction_def: None,
            reaction_def_form_name: String::new(),
            reaction_def_form_emoji: String::new(),
            reaction_def_form_label: String::new(),
            reaction_def_form_description: String::new(),
            show_reaction_def_form: false,
            new_prefix_input: String::new(),
            // Type definition editing
            editing_type_def: None,
            type_def_form_name: String::new(),
            type_def_form_display_name: String::new(),
            type_def_form_description: String::new(),
            type_def_form_prefix: String::new(),
            type_def_form_statuses: Vec::new(),
            type_def_form_priorities: Vec::new(),
            type_def_form_fields: Vec::new(),
            show_type_def_form: false,
            new_status_input: String::new(),
            new_priority_input: String::new(),
            editing_field_idx: None,
            field_form_name: String::new(),
            field_form_label: String::new(),
            field_form_type: CustomFieldType::Text,
            field_form_required: false,
            field_form_options: String::new(),
            field_form_default: String::new(),
            show_field_form: false,
            // URL form
            show_url_form: false,
            editing_url_id: None,
            url_form_url: String::new(),
            url_form_title: String::new(),
            url_form_description: String::new(),
            url_verification_status: None,
            url_verification_in_progress: false,
            // Markdown help
            show_markdown_help: false,
            // Reference picker
            show_reference_picker: false,
            reference_picker_search: String::new(),
            reference_picker_selected: None,
            // Context menu
            last_text_selection: None,
            // Project management
            show_new_project_dialog: false,
            new_project_dir: String::new(),
            new_project_name: String::new(),
            new_project_title: String::new(),
            new_project_description: String::new(),
            new_project_template: "current".to_string(),
            new_project_include_users: false,
            show_switch_project_dialog: false,
            available_projects: Vec::new(),
            // Form cancel confirmation
            show_cancel_confirm_dialog: false,
            original_form_title: String::new(),
            original_form_description: String::new(),
            original_form_status_string: String::new(),
            original_form_priority: RequirementPriority::Medium,
            original_form_priority_string: String::new(),
            original_form_type: RequirementType::Functional,
            original_form_owner: String::new(),
            original_form_feature: String::new(),
            form_feature_selected_idx: 0,
            original_form_tags: String::new(),
            original_form_prefix: String::new(),
            original_form_custom_fields: HashMap::new(),

            // AI integration state
            ai_client: AiClient::new(),
            ai_pending_action: None,
            ai_last_result: None,
            show_ai_results_panel: false,
            ai_loading: false,

            // Background AI evaluation state
            ai_eval_receiver: None,
            ai_eval_in_progress: None,

            // Background Find Duplicates state
            find_duplicates_receiver: None,
            find_duplicates_in_progress: None,

            // Toast notification state
            toast_message: None,

            // Project scaffolding state (FR-0152)
            show_scaffold_dialog: false,
            scaffold_config: aida_core::ScaffoldConfig::default(),
            scaffold_preview: None,
            scaffold_tech_stack_input: String::new(),

            // Conflict detection state (FR-0153)
            original_timestamps: initial_timestamps,
            modified_requirement_ids: HashSet::new(),
            show_conflict_dialog: false,
            current_conflict: None,

            // Session tracking state (collaborative awareness)
            session_id: session_id.clone(),
            heartbeat_sender: Some(heartbeat_sender),
            last_lock_info: initial_lock_info,
            other_sessions_warning_shown: false,

            // Baseline management state
            show_create_baseline_dialog: false,
            baseline_form_name: String::new(),
            baseline_form_description: String::new(),
            selected_baseline_id: None,
            baseline_compare_source: None,
            baseline_compare_target: None,
            show_baseline_comparison: false,

            // Clone requirement state
            show_clone_dialog: false,
            clone_source_idx: None,
            clone_include_tags: true,
            clone_include_relationships: false,
            clone_include_comments: false,
            clone_include_history: false,
            clone_include_urls: true,
            clone_include_custom_fields: true,

            // Timeline view state
            timeline_selected_date: None,
            timeline_events: Vec::new(),
            timeline_filter_author: String::new(),
            timeline_filter_field: String::new(),
            timeline_selected_event_idx: None,
            timeline_suppress_hover: false,
            timeline_suppress_hover_pos: None,

            // Planning view state
            planning_collapsed_sprints: std::collections::HashSet::new(),
            planning_show_completed_sprints: false,
            planning_selected_item: None,
            planning_drag_source: None,
            planning_drag_target_sprint: None,
        }
    }

    /// Configure fonts with better Unicode symbol support
    fn configure_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();

        // Embed DejaVu Sans font at compile time for cross-platform Unicode support
        // DejaVu Sans is licensed under a free license (similar to MIT)
        // See: https://dejavu-fonts.github.io/License.html
        const DEJAVU_SANS: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

        fonts.font_data.insert(
            "dejavu_sans".to_owned(),
            egui::FontData::from_static(DEJAVU_SANS).into(),
        );

        // Add DejaVu Sans as highest priority for proportional text
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "dejavu_sans".to_owned());

        ctx.set_fonts(fonts);
    }

    /// Increase font size by one step
    fn zoom_in(&mut self) {
        self.current_font_size = (self.current_font_size + FONT_SIZE_STEP).min(MAX_FONT_SIZE);
    }

    /// Decrease font size by one step
    fn zoom_out(&mut self) {
        self.current_font_size = (self.current_font_size - FONT_SIZE_STEP).max(MIN_FONT_SIZE);
    }

    /// Reset font size to base setting
    fn reset_zoom(&mut self) {
        self.current_font_size = self.user_settings.base_font_size;
    }

    /// Check if the current view settings match the active preset
    fn current_view_matches_active_preset(&self) -> bool {
        if let Some(ref preset_name) = self.active_preset {
            if let Some(preset) = self
                .user_settings
                .view_presets
                .iter()
                .find(|p| &p.name == preset_name)
            {
                return self.perspective == preset.perspective
                    && self.perspective_direction == preset.direction
                    && self.filter_types == preset.get_filter_types()
                    && self.filter_features == preset.get_filter_features()
                    && self.filter_prefixes == preset.get_filter_prefixes()
                    && self.child_filter_types == preset.get_child_filter_types()
                    && self.child_filter_features == preset.get_child_filter_features()
                    && self.child_filter_prefixes == preset.get_child_filter_prefixes()
                    && self.children_same_as_root == preset.children_same_as_root;
            }
        }
        false
    }

    /// Check if there's an unsaved view (view differs from active preset or no preset active but has non-default settings)
    fn has_unsaved_view(&self) -> bool {
        if self.active_preset.is_some() {
            // If we have an active preset, check if current view differs
            !self.current_view_matches_active_preset()
        } else {
            // If no active preset, check if we have non-default settings
            self.perspective != Perspective::Flat
                || self.perspective_direction != PerspectiveDirection::TopDown
                || !self.filter_types.is_empty()
                || !self.filter_features.is_empty()
                || !self.filter_prefixes.is_empty()
                || !self.children_same_as_root
                || !self.child_filter_types.is_empty()
                || !self.child_filter_features.is_empty()
                || !self.child_filter_prefixes.is_empty()
        }
    }

    /// Apply a preset to the current view
    fn apply_preset(&mut self, preset: &ViewPreset) {
        self.perspective = preset.perspective.clone();
        self.perspective_direction = preset.direction;
        self.filter_types = preset.get_filter_types();
        self.filter_features = preset.get_filter_features();
        self.filter_prefixes = preset.get_filter_prefixes();
        self.child_filter_types = preset.get_child_filter_types();
        self.child_filter_features = preset.get_child_filter_features();
        self.child_filter_prefixes = preset.get_child_filter_prefixes();
        self.children_same_as_root = preset.children_same_as_root;
        self.active_preset = Some(preset.name.clone());
    }

    /// Save the current view as a new preset
    fn save_current_view_as_preset(&mut self, name: String) {
        let preset = ViewPreset::new(
            name.clone(),
            self.perspective.clone(),
            self.perspective_direction,
            &self.filter_types,
            &self.filter_features,
            &self.filter_prefixes,
            &self.child_filter_types,
            &self.child_filter_features,
            &self.child_filter_prefixes,
            self.children_same_as_root,
        );

        // Check if preset with this name already exists
        if let Some(existing) = self
            .user_settings
            .view_presets
            .iter_mut()
            .find(|p| p.name == name)
        {
            // Update existing preset
            *existing = preset;
        } else {
            // Add new preset
            self.user_settings.view_presets.push(preset);
        }

        self.active_preset = Some(name);

        // Save settings
        if let Err(e) = self.user_settings.save() {
            self.message = Some((format!("Failed to save preset: {}", e), true));
        } else {
            self.message = Some(("View preset saved".to_string(), false));
        }
    }

    /// Save current List 2 (split) view as a preset
    fn save_split_view_as_preset(&mut self, name: String) {
        let preset = ViewPreset::new(
            name.clone(),
            self.split_perspective.clone(),
            self.split_perspective_direction,
            &self.split_filter_types,
            &self.split_filter_features,
            &self.split_filter_prefixes,
            &HashSet::new(), // No child filters for split view
            &HashSet::new(),
            &HashSet::new(),
            true,
        );

        // Check if preset with this name already exists
        if let Some(existing) = self
            .user_settings
            .view_presets
            .iter_mut()
            .find(|p| p.name == name)
        {
            // Update existing preset
            *existing = preset;
        } else {
            // Add new preset
            self.user_settings.view_presets.push(preset);
        }

        self.split_active_preset = Some(name);

        // Save settings
        if let Err(e) = self.user_settings.save() {
            self.message = Some((format!("Failed to save preset: {}", e), true));
        } else {
            self.message = Some(("View preset saved".to_string(), false));
        }
    }

    /// Delete a preset by name
    fn delete_preset(&mut self, name: &str) {
        self.user_settings.view_presets.retain(|p| p.name != name);

        // If the deleted preset was active, clear it
        if self.active_preset.as_deref() == Some(name) {
            self.active_preset = None;
        }

        // Save settings
        if let Err(e) = self.user_settings.save() {
            self.message = Some((format!("Failed to delete preset: {}", e), true));
        } else {
            self.message = Some(("View preset deleted".to_string(), false));
        }
    }

    /// Reset to default view (user's preferred perspective, TopDown, no filters, clear search)
    fn reset_to_default_view(&mut self) {
        self.perspective = self.user_settings.preferred_perspective.clone();
        self.perspective_direction = PerspectiveDirection::TopDown;
        self.filter_types.clear();
        self.filter_features.clear();
        self.filter_text.clear();
        self.search_scope = SearchScope::all();
        self.active_preset = None;
    }

    /// Open the user guide in the default browser
    fn open_user_guide() {
        // Get the path to the docs directory relative to the executable
        let exe_path = std::env::current_exe().ok();

        let possible_paths: Vec<PathBuf> = if let Some(ref exe) = exe_path {
            vec![
                // Relative to executable (for installed binaries)
                exe.parent().unwrap().join("../docs"),
                exe.parent().unwrap().join("../../docs"),
                // Development paths
                exe.parent().unwrap().join("../../../docs"),
                exe.parent().unwrap().join("../../../../docs"),
                // Current directory
                std::env::current_dir().unwrap_or_default().join("docs"),
                // Project root (when running from project directory)
                PathBuf::from("docs"),
            ]
        } else {
            vec![
                std::env::current_dir().unwrap_or_default().join("docs"),
                PathBuf::from("docs"),
            ]
        };

        let filename = "user-guide.html";

        // Find the first path that exists
        let doc_path = possible_paths
            .iter()
            .map(|p| p.join(filename))
            .find(|p| p.exists());

        if let Some(path) = doc_path {
            if let Ok(canonical) = path.canonicalize() {
                let url = format!("file://{}", canonical.to_string_lossy());

                // Try to open in browser using platform-specific commands
                #[cfg(target_os = "linux")]
                {
                    let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                }

                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("open").arg(&url).spawn();
                }

                #[cfg(target_os = "windows")]
                {
                    let _ = std::process::Command::new("cmd")
                        .args(["/C", "start", &url])
                        .spawn();
                }
            }
        }
    }

    /// Restart the application, detecting if running via cargo run
    fn restart_application() {
        let exe_path = std::env::current_exe().ok();
        let args: Vec<String> = std::env::args().collect();

        // Detect if we're running from a cargo target directory
        let is_cargo_run = exe_path
            .as_ref()
            .map(|p| {
                let path_str = p.to_string_lossy();
                path_str.contains("/target/debug/") || path_str.contains("/target/release/")
            })
            .unwrap_or(false);

        if is_cargo_run {
            // Running via cargo - use cargo run to potentially trigger recompile
            // Determine the binary name from the exe path
            let bin_name = exe_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("aida-gui");

            // Check if this is a release build
            let is_release = exe_path
                .as_ref()
                .map(|p| p.to_string_lossy().contains("/target/release/"))
                .unwrap_or(false);

            let mut cmd = std::process::Command::new("cargo");
            cmd.arg("run").arg("--bin").arg(bin_name);

            if is_release {
                cmd.arg("--release");
            }

            // Pass through any additional arguments (skip the exe name)
            if args.len() > 1 {
                cmd.arg("--").args(&args[1..]);
            }

            // Spawn the new process detached
            let _ = cmd.spawn();
        } else if let Some(exe) = exe_path {
            // Direct executable - just restart it
            let mut cmd = std::process::Command::new(&exe);

            // Pass through any arguments (skip the exe name)
            if args.len() > 1 {
                cmd.args(&args[1..]);
            }

            // Spawn the new process detached
            let _ = cmd.spawn();
        }
    }

    /// Opens a new window (spawns another instance of the application)
    fn open_new_window() {
        let exe_path = std::env::current_exe().ok();
        let args: Vec<String> = std::env::args().collect();

        // Detect if we're running from a cargo target directory
        let is_cargo_run = exe_path
            .as_ref()
            .map(|p| {
                let path_str = p.to_string_lossy();
                path_str.contains("/target/debug/") || path_str.contains("/target/release/")
            })
            .unwrap_or(false);

        if is_cargo_run {
            // Running via cargo - use cargo run
            let bin_name = exe_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("aida-gui");

            let is_release = exe_path
                .as_ref()
                .map(|p| p.to_string_lossy().contains("/target/release/"))
                .unwrap_or(false);

            let mut cmd = std::process::Command::new("cargo");
            cmd.arg("run").arg("--bin").arg(bin_name);

            if is_release {
                cmd.arg("--release");
            }

            // Pass through any additional arguments (skip the exe name)
            if args.len() > 1 {
                cmd.arg("--").args(&args[1..]);
            }

            // Spawn the new process detached
            let _ = cmd.spawn();
        } else if let Some(exe) = exe_path {
            // Direct executable - just launch it
            let mut cmd = std::process::Command::new(&exe);

            // Pass through any arguments (skip the exe name)
            if args.len() > 1 {
                cmd.args(&args[1..]);
            }

            // Spawn the new process detached
            let _ = cmd.spawn();
        }
    }

    /// Open a specific project file in a new window
    fn open_project_in_new_window(file_path: String) {
        let exe_path = std::env::current_exe().ok();

        // Detect if we're running from a cargo target directory
        let is_cargo_run = exe_path
            .as_ref()
            .map(|p| {
                let path_str = p.to_string_lossy();
                path_str.contains("/target/debug/") || path_str.contains("/target/release/")
            })
            .unwrap_or(false);

        if is_cargo_run {
            // Running via cargo - use cargo run
            let bin_name = exe_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("aida-gui");

            let is_release = exe_path
                .as_ref()
                .map(|p| p.to_string_lossy().contains("/target/release/"))
                .unwrap_or(false);

            let mut cmd = std::process::Command::new("cargo");
            cmd.arg("run").arg("--bin").arg(bin_name);

            if is_release {
                cmd.arg("--release");
            }

            // Pass the file path
            cmd.arg("--").arg("--file").arg(&file_path);

            // Spawn the new process detached
            let _ = cmd.spawn();
        } else if let Some(exe) = exe_path {
            // Direct executable - just launch it
            let mut cmd = std::process::Command::new(&exe);

            // Pass the file path
            cmd.arg("--file").arg(&file_path);

            // Spawn the new process detached
            let _ = cmd.spawn();
        }
    }

    fn reload(&mut self) {
        match self.storage.load() {
            Ok(store) => {
                // Update timestamps snapshot when reloading
                self.original_timestamps = Storage::get_requirement_timestamps(&store);
                self.modified_requirement_ids.clear();
                self.store = store;
                self.message = Some(("Reloaded successfully".to_string(), false));
            }
            Err(e) => {
                eprintln!("ERROR: Failed to reload: {}", e);
                self.message = Some((format!("Failed to reload: {}", e), true));
            }
        }
    }

    /// Save current requirements to a new file location
    fn save_as(&mut self) {
        // Use rfd for native file dialog
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("YAML files", &["yaml", "yml"])
            .set_file_name("requirements.yaml")
            .save_file()
        {
            // Serialize and save to the new path
            match serde_yaml::to_string(&self.store) {
                Ok(yaml) => {
                    match std::fs::write(&path, yaml) {
                        Ok(()) => {
                            self.message = Some((
                                format!("Saved to: {}", path.display()),
                                false,
                            ));
                        }
                        Err(e) => {
                            self.message = Some((
                                format!("Failed to write file: {}", e),
                                true,
                            ));
                        }
                    }
                }
                Err(e) => {
                    self.message = Some((
                        format!("Failed to serialize: {}", e),
                        true,
                    ));
                }
            }
        }
    }

    // trace:FR-0153 | ai:claude:high
    fn save(&mut self) {
        // If no requirements were modified, use standard save
        if self.modified_requirement_ids.is_empty() {
            if let Err(e) = self.storage.save(&self.store) {
                self.message = Some((format!("Error saving: {}", e), true));
            } else {
                self.message = Some(("Saved successfully".to_string(), false));
            }
            return;
        }

        // Use conflict-aware save
        let modified_ids: Vec<Uuid> = self.modified_requirement_ids.iter().cloned().collect();
        match self.storage.save_with_conflict_detection(
            &self.store,
            &self.original_timestamps,
            &modified_ids,
        ) {
            Ok(SaveResult::Success) => {
                // Update timestamps and clear modified set
                self.original_timestamps = Storage::get_requirement_timestamps(&self.store);
                self.modified_requirement_ids.clear();
                self.message = Some(("Saved successfully".to_string(), false));
            }
            Ok(SaveResult::Merged { merged_count }) => {
                // Reload to get merged changes
                if let Ok(store) = self.storage.load() {
                    self.store = store;
                    self.original_timestamps = Storage::get_requirement_timestamps(&self.store);
                    self.modified_requirement_ids.clear();
                }
                self.message = Some((
                    format!("Saved with {} merged external change(s)", merged_count),
                    false,
                ));
            }
            Ok(SaveResult::Conflict(conflict_info)) => {
                // Show conflict resolution dialog
                self.current_conflict = Some(conflict_info);
                self.show_conflict_dialog = true;
                // Don't clear modified_requirement_ids - user needs to resolve
            }
            Err(e) => {
                self.message = Some((format!("Error saving: {}", e), true));
            }
        }
    }

    /// Mark a requirement as modified (for conflict tracking)
    fn mark_requirement_modified(&mut self, req_id: Uuid) {
        self.modified_requirement_ids.insert(req_id);
    }

    /// Display toast notification overlay at the bottom of the screen
    fn show_toast_notification(&mut self, ctx: &egui::Context) {
        // Check if toast should still be displayed
        if let Some(toast) = &self.toast_message {
            if Instant::now() > toast.show_until {
                self.toast_message = None;
                return;
            }
        }

        if let Some(toast) = &self.toast_message {
            let screen_rect = ctx.screen_rect();
            let toast_width = (screen_rect.width() * 0.4).min(500.0);
            let toast_height = 50.0;
            let margin = 20.0;

            let toast_pos = egui::pos2(
                (screen_rect.width() - toast_width) / 2.0,
                screen_rect.height() - toast_height - margin,
            );

            let toast_rect = egui::Rect::from_min_size(toast_pos, egui::vec2(toast_width, toast_height));

            egui::Area::new(egui::Id::new("toast_notification"))
                .fixed_pos(toast_pos)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let bg_color = if toast.is_success {
                        egui::Color32::from_rgb(34, 139, 34) // Forest green
                    } else {
                        egui::Color32::from_rgb(178, 34, 34) // Firebrick red
                    };

                    egui::Frame::none()
                        .fill(bg_color)
                        .rounding(8.0)
                        .inner_margin(egui::Margin::same(12.0))
                        .show(ui, |ui| {
                            ui.set_min_width(toast_width - 24.0);
                            ui.set_max_width(toast_width - 24.0);
                            ui.horizontal(|ui| {
                                let icon = if toast.is_success { "✓" } else { "✗" };
                                ui.label(
                                    egui::RichText::new(icon)
                                        .color(egui::Color32::WHITE)
                                        .size(18.0),
                                );
                                ui.add_space(8.0);
                                ui.label(
                                    egui::RichText::new(&toast.message)
                                        .color(egui::Color32::WHITE)
                                        .size(14.0),
                                );
                            });
                        });
                });

            // Request repaint to keep checking the timer
            ctx.request_repaint();
        }
    }

    /// Cycle through all themes including custom ones
    fn cycle_theme(&mut self) {
        // Build list of all available themes: built-ins + custom themes
        let built_in_themes = [
            Theme::Dark,
            Theme::Light,
            Theme::HighContrastDark,
            Theme::SolarizedDark,
            Theme::Nord,
        ];

        // Find current theme index
        let current = &self.user_settings.theme;

        // Check if current is a built-in theme
        let mut current_idx: Option<usize> = None;
        for (i, theme) in built_in_themes.iter().enumerate() {
            if std::mem::discriminant(current) == std::mem::discriminant(theme)
                && !matches!(current, Theme::Custom(_))
            {
                if current == theme {
                    current_idx = Some(i);
                    break;
                }
            }
        }

        // If not found in built-ins, check custom themes
        let custom_themes = &self.user_settings.custom_themes;
        let mut current_custom_idx: Option<usize> = None;
        if current_idx.is_none() {
            if let Theme::Custom(ref current_custom) = current {
                for (i, custom) in custom_themes.iter().enumerate() {
                    if custom.name == current_custom.name {
                        current_custom_idx = Some(i);
                        break;
                    }
                }
            }
        }

        // Determine next theme
        let next_theme = if let Some(idx) = current_idx {
            // Currently on a built-in theme
            if idx + 1 < built_in_themes.len() {
                // Next built-in
                built_in_themes[idx + 1].clone()
            } else if !custom_themes.is_empty() {
                // First custom theme
                Theme::Custom(Box::new(custom_themes[0].clone()))
            } else {
                // Wrap to first built-in
                Theme::Dark
            }
        } else if let Some(idx) = current_custom_idx {
            // Currently on a custom theme
            if idx + 1 < custom_themes.len() {
                // Next custom theme
                Theme::Custom(Box::new(custom_themes[idx + 1].clone()))
            } else {
                // Wrap to first built-in
                Theme::Dark
            }
        } else {
            // Unknown state, reset to Dark
            Theme::Dark
        };

        self.user_settings.theme = next_theme;
    }

    /// Determine parent for new requirement using smart heuristic:
    /// - If selected has children: new req becomes child of selected (parent = selected)
    /// - If selected has no children but has parent: new req becomes sibling (parent = selected's parent)
    /// - If selected has no children and no parent: new req is root (no parent)
    fn determine_smart_parent(&self) -> Option<Uuid> {
        let selected_req = self
            .selected_idx
            .and_then(|idx| self.store.requirements.get(idx))?;

        // Check if selected requirement has children
        let has_children = self
            .store
            .requirements
            .iter()
            .any(|r| {
                r.relationships.iter().any(|rel| {
                    rel.rel_type == RelationshipType::Parent && rel.target_id == selected_req.id
                })
            });

        if has_children {
            // Selected has children: new req becomes child of selected
            Some(selected_req.id)
        } else {
            // Selected has no children: check if it has a parent
            let parent_rel = selected_req
                .relationships
                .iter()
                .find(|r| r.rel_type == RelationshipType::Parent);

            if let Some(rel) = parent_rel {
                // Selected has a parent: new req becomes sibling (same parent)
                Some(rel.target_id)
            } else {
                // Selected has no parent: new req is root
                None
            }
        }
    }

    /// Get parent for new sibling requirement (same parent as selected, or None for root)
    fn get_sibling_parent(&self) -> Option<Uuid> {
        self.selected_idx
            .and_then(|idx| self.store.requirements.get(idx))
            .and_then(|req| {
                req.relationships
                    .iter()
                    .find(|r| r.rel_type == RelationshipType::Parent)
                    .map(|r| r.target_id)
            })
    }

    /// Get parent for new child requirement (selected requirement becomes parent)
    fn get_child_parent(&self) -> Option<Uuid> {
        self.selected_idx
            .and_then(|idx| self.store.requirements.get(idx))
            .map(|req| req.id)
    }

    fn clear_form(&mut self) {
        self.clear_form_base();

        // Use smart heuristic for parent selection
        self.form_parent_id = self.determine_smart_parent();

        // Store original values for change detection
        self.store_original_form_values();
    }

    /// Clear form with explicit parent for sibling creation
    fn clear_form_for_sibling(&mut self) {
        self.clear_form_base();
        self.form_parent_id = self.get_sibling_parent();
        self.store_original_form_values();
    }

    /// Clear form with explicit parent for child creation
    fn clear_form_for_child(&mut self) {
        self.clear_form_base();
        self.form_parent_id = self.get_child_parent();
        self.store_original_form_values();
    }

    /// Base form clearing without parent logic
    fn clear_form_base(&mut self) {
        self.form_title.clear();
        self.form_description.clear();
        self.form_status = RequirementStatus::Draft;
        self.form_status_string = String::from("Draft");
        self.form_custom_fields.clear();
        self.form_priority = RequirementPriority::Medium;
        self.form_type = RequirementType::Functional;
        self.form_owner.clear();
        self.form_feature = self.get_uncategorized_feature();
        self.form_tags.clear();
        self.form_prefix.clear();
        self.show_description_preview = false;
        self.form_title_auto_synced = true;
        self.form_last_description.clear();
    }

    /// Get the existing "Uncategorized" feature name (with number prefix if it exists)
    /// This ensures we reuse the same Uncategorized feature instead of creating new ones
    fn get_uncategorized_feature(&self) -> String {
        // Look for an existing feature that ends with "Uncategorized" (with number prefix)
        for req in &self.store.requirements {
            if req.feature.ends_with("-Uncategorized") || req.feature == "Uncategorized" {
                // Return the first matching feature (prefer numbered ones)
                if req.feature.contains('-') {
                    return req.feature.clone();
                }
            }
        }
        // No existing Uncategorized feature found, use the base name
        // (migration will add a number on next load, but at least they'll all share it)
        String::from("Uncategorized")
    }

    fn load_form_from_requirement(&mut self, idx: usize) {
        if let Some(req) = self.store.requirements.get(idx) {
            self.form_title = req.title.clone();
            self.form_description = req.description.clone();
            self.form_status = req.status.clone();
            self.form_status_string = req.effective_status();
            self.form_custom_fields = req.custom_fields.clone();
            self.form_priority = req.priority.clone();
            self.form_priority_string = req.effective_priority();
            self.form_type = req.req_type.clone();
            self.form_owner = req.owner.clone();
            self.form_feature = req.feature.clone();
            let tags_vec: Vec<String> = req.tags.iter().cloned().collect();
            self.form_tags = tags_vec.join(", ");
            self.form_prefix = req.prefix_override.clone().unwrap_or_default();
            self.show_description_preview = false;
        }
        // Store original values for change detection
        self.store_original_form_values();
    }

    /// Store current form values as original values (for change detection on cancel)
    fn store_original_form_values(&mut self) {
        self.original_form_title = self.form_title.clone();
        self.original_form_description = self.form_description.clone();
        self.original_form_status_string = self.form_status_string.clone();
        self.original_form_priority = self.form_priority.clone();
        self.original_form_priority_string = self.form_priority_string.clone();
        self.original_form_type = self.form_type.clone();
        self.original_form_owner = self.form_owner.clone();
        self.original_form_feature = self.form_feature.clone();
        self.original_form_tags = self.form_tags.clone();
        self.original_form_prefix = self.form_prefix.clone();
        self.original_form_custom_fields = self.form_custom_fields.clone();
    }

    /// Check if form has unsaved changes compared to original values
    fn form_has_changes(&self) -> bool {
        self.form_title != self.original_form_title
            || self.form_description != self.original_form_description
            || self.form_status_string != self.original_form_status_string
            || self.form_priority_string != self.original_form_priority_string
            || self.form_type != self.original_form_type
            || self.form_owner != self.original_form_owner
            || self.form_feature != self.original_form_feature
            || self.form_tags != self.original_form_tags
            || self.form_prefix != self.original_form_prefix
            || self.form_custom_fields != self.original_form_custom_fields
    }

    /// Handle cancel request - either cancel immediately or show confirmation dialog
    fn request_form_cancel(&mut self, is_edit: bool) {
        if self.form_has_changes() {
            self.show_cancel_confirm_dialog = true;
        } else {
            self.cancel_form(is_edit);
        }
    }

    /// Actually cancel the form and return to previous view
    fn cancel_form(&mut self, is_edit: bool) {
        self.clear_form();
        self.show_cancel_confirm_dialog = false;
        self.pending_view_change = Some(if is_edit { View::Detail } else { View::List });
    }

    // trace:FR-0183 | ai:claude:high
    fn add_requirement(&mut self) {
        let tags: HashSet<String> = self
            .form_tags
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut req = Requirement::new(self.form_title.clone(), self.form_description.clone());
        // Set status from string (handles both standard and custom statuses)
        req.set_status_from_str(&self.form_status_string);
        // Set priority from string (handles both standard and custom priorities)
        req.set_priority_from_str(&self.form_priority_string);
        req.req_type = self.form_type.clone();
        req.owner = self.form_owner.clone();
        req.feature = self.form_feature.clone();
        req.tags = tags;
        // Copy custom field values
        req.custom_fields = self.form_custom_fields.clone();
        // Set created_by to current user
        req.created_by = Some(self.user_settings.display_name());

        // Set prefix override if specified
        let prefix_trimmed = self.form_prefix.trim();
        if !prefix_trimmed.is_empty() {
            if let Err(e) = req.set_prefix_override(prefix_trimmed) {
                self.message = Some((e, true));
                return;
            }
            // Auto-add new prefix to allowed list (if not restricted)
            if !self.store.restrict_prefixes {
                self.store.add_allowed_prefix(prefix_trimmed);
            }
        }

        // Store parent ID before clearing form
        let parent_id = self.form_parent_id;

        // Get prefixes for ID generation
        let feature_prefix = self
            .store
            .get_feature_by_name(&req.feature)
            .map(|f| f.prefix.clone());
        let type_prefix = self.store.get_type_prefix(&req.req_type);

        // Capture the new requirement's ID before adding
        let new_req_id = req.id;

        // Use atomic add which reloads fresh data before adding (FR-0183)
        // This prevents duplicate SPEC-IDs and preserves concurrent changes
        match self.storage.add_requirement_atomic(
            &self.store,
            req,
            feature_prefix.as_deref(),
            type_prefix.as_deref(),
        ) {
            Ok(result) => {
                // Update our store with the merged result
                self.store = result.store;

                // Update timestamps for conflict detection
                self.original_timestamps = Storage::get_requirement_timestamps(&self.store);
                self.modified_requirement_ids.clear();

                // Create parent relationship if specified
                if let Some(parent_id) = parent_id {
                    // New requirement (child) stores Parent relationship pointing to parent
                    let _ = self.store.add_relationship_with_creator(
                        &new_req_id,
                        RelationshipType::Parent,
                        &parent_id,
                        true, // bidirectional
                        Some(self.user_settings.display_name()),
                    );
                    // Save the relationship
                    if let Err(e) = self.storage.save(&self.store) {
                        self.message = Some((format!("Error saving relationship: {}", e), true));
                        return;
                    }
                }

                self.form_parent_id = None;
                self.clear_form();

                // Find the index of the newly added requirement and select it
                if let Some(idx) = self
                    .store
                    .requirements
                    .iter()
                    .position(|r| r.id == new_req_id)
                {
                    self.selected_idx = Some(idx);
                    self.scroll_to_requirement = Some(new_req_id);
                    self.current_view = View::Detail;
                } else {
                    self.current_view = View::List;
                }

                // Show appropriate message
                if result.external_changes_merged > 0 {
                    self.message = Some((
                        format!(
                            "Requirement {} added (merged {} external change(s))",
                            result.spec_id, result.external_changes_merged
                        ),
                        false,
                    ));
                } else {
                    self.message = Some((
                        format!("Requirement {} added successfully", result.spec_id),
                        false,
                    ));
                }
            }
            Err(e) => {
                self.message = Some((format!("Error adding requirement: {}", e), true));
            }
        }
    }

    fn update_requirement(&mut self, idx: usize) {
        // Gather data we need before mutable borrows
        let (req_uuid, old_prefix_override, old_feature, old_req_type) = {
            if let Some(req) = self.store.requirements.get(idx) {
                (
                    req.id,
                    req.prefix_override.clone(),
                    req.feature.clone(),
                    req.req_type.clone(),
                )
            } else {
                return;
            }
        };

        // Determine new prefix
        let new_prefix = if self.form_prefix.trim().is_empty() {
            None
        } else {
            // Auto-add new prefix to allowed list (if not restricted)
            if !self.store.restrict_prefixes {
                self.store.add_allowed_prefix(self.form_prefix.trim());
            }
            Requirement::validate_prefix(&self.form_prefix)
        };

        // Check if we need to regenerate the spec_id
        let prefix_changed = new_prefix != old_prefix_override;
        let feature_changed = self.form_feature != old_feature;
        let type_changed = self.form_type != old_req_type;
        let needs_new_spec_id =
            prefix_changed || (new_prefix.is_none() && (feature_changed || type_changed));

        // Generate new spec_id if needed
        let new_spec_id_result = if needs_new_spec_id {
            let feature_prefix = self
                .store
                .get_feature_by_name(&self.form_feature)
                .map(|f| f.prefix.clone());
            let type_prefix = self.store.get_type_prefix(&self.form_type);

            Some(self.store.regenerate_spec_id_for_prefix_change(
                &req_uuid,
                new_prefix.as_deref(),
                feature_prefix.as_deref(),
                type_prefix.as_deref(),
            ))
        } else {
            None
        };

        // Check for ID conflict
        if let Some(Err(ref error_msg)) = new_spec_id_result {
            self.message = Some((error_msg.clone(), true));
            return;
        }

        // Now perform the actual updates
        if let Some(req) = self.store.requirements.get_mut(idx) {
            let mut changes: Vec<FieldChange> = Vec::new();

            // Track title change
            if self.form_title != req.title {
                changes.push(Requirement::field_change(
                    "title",
                    req.title.clone(),
                    self.form_title.clone(),
                ));
                req.title = self.form_title.clone();
            }

            // Track description change
            if self.form_description != req.description {
                changes.push(Requirement::field_change(
                    "description",
                    req.description.clone(),
                    self.form_description.clone(),
                ));
                req.description = self.form_description.clone();
            }

            // Track status change (use effective_status for comparison)
            let old_status = req.effective_status();
            if self.form_status_string != old_status {
                changes.push(Requirement::field_change(
                    "status",
                    old_status,
                    self.form_status_string.clone(),
                ));
                req.set_status_from_str(&self.form_status_string);
            }

            // Track custom fields changes
            for (key, new_value) in &self.form_custom_fields {
                let old_value = req.custom_fields.get(key).cloned().unwrap_or_default();
                if *new_value != old_value {
                    changes.push(Requirement::field_change(key, old_value, new_value.clone()));
                }
            }
            req.custom_fields = self.form_custom_fields.clone();

            // Track priority change (use effective_priority for comparison)
            let old_priority = req.effective_priority();
            if self.form_priority_string != old_priority {
                changes.push(Requirement::field_change(
                    "priority",
                    old_priority,
                    self.form_priority_string.clone(),
                ));
                req.set_priority_from_str(&self.form_priority_string);
            }

            // Track type change
            if self.form_type != req.req_type {
                changes.push(Requirement::field_change(
                    "type",
                    format!("{:?}", req.req_type),
                    format!("{:?}", self.form_type),
                ));
                req.req_type = self.form_type.clone();
            }

            // Track owner change
            if self.form_owner != req.owner {
                changes.push(Requirement::field_change(
                    "owner",
                    req.owner.clone(),
                    self.form_owner.clone(),
                ));
                req.owner = self.form_owner.clone();
            }

            // Track feature change
            if self.form_feature != req.feature {
                changes.push(Requirement::field_change(
                    "feature",
                    req.feature.clone(),
                    self.form_feature.clone(),
                ));
                req.feature = self.form_feature.clone();
            }

            // Track tags change
            let new_tags: HashSet<String> = self
                .form_tags
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            if new_tags != req.tags {
                let old_tags_vec: Vec<String> = req.tags.iter().cloned().collect();
                let new_tags_vec: Vec<String> = new_tags.iter().cloned().collect();
                changes.push(Requirement::field_change(
                    "tags",
                    old_tags_vec.join(", "),
                    new_tags_vec.join(", "),
                ));
                req.tags = new_tags;
            }

            // Track prefix override change and update spec_id
            if prefix_changed {
                let old_prefix_str = req.prefix_override.clone().unwrap_or_default();
                let new_prefix_str = new_prefix.clone().unwrap_or_default();
                changes.push(Requirement::field_change(
                    "prefix_override",
                    old_prefix_str,
                    new_prefix_str,
                ));
                req.prefix_override = new_prefix;
            }

            // Update spec_id if it changed
            if let Some(Ok(ref new_id)) = new_spec_id_result {
                let old_id = req.spec_id.clone().unwrap_or_default();
                if old_id != *new_id {
                    changes.push(Requirement::field_change("spec_id", old_id, new_id.clone()));
                    req.spec_id = Some(new_id.clone());
                }
            }

            // Record changes with author from user settings
            req.record_change(self.user_settings.display_name(), changes);

            // Mark requirement as modified for conflict tracking (FR-0153)
            self.mark_requirement_modified(req_uuid);

            self.save();
            self.clear_form();
            self.current_view = View::Detail;
            self.message = Some(("Requirement updated successfully".to_string(), false));
        }
    }

    fn delete_requirement(&mut self, idx: usize) {
        if idx < self.store.requirements.len() {
            // Capture ID before removing for conflict detection (FR-0153)
            let deleted_id = self.store.requirements[idx].id;
            self.store.requirements.remove(idx);
            // Mark as modified so conflict detection handles the deletion
            self.modified_requirement_ids.insert(deleted_id);
            self.save();
            self.selected_idx = None;
            self.current_view = View::List;
            self.message = Some(("Requirement deleted successfully".to_string(), false));
        }
    }

    fn toggle_archive(&mut self, idx: usize) {
        let (new_archived, author, req_id) = {
            if let Some(req) = self.store.requirements.get(idx) {
                (!req.archived, self.user_settings.display_name(), req.id)
            } else {
                return;
            }
        };

        if let Some(req) = self.store.requirements.get_mut(idx) {
            let was_archived = req.archived;
            req.archived = new_archived;

            // Record change in history
            let change = Requirement::field_change(
                "archived",
                was_archived.to_string(),
                new_archived.to_string(),
            );
            req.record_change(author, vec![change]);
        }

        // Mark requirement as modified for conflict tracking (FR-0153)
        self.mark_requirement_modified(req_id);

        self.save();
        let action = if new_archived {
            "archived"
        } else {
            "unarchived"
        };
        self.message = Some((format!("Requirement {}", action), false));
    }

    fn show_top_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                // Menu dropdown
                ui.menu_button("☰ Menu", |ui| {
                    if ui.button("🔄 Reload").clicked() {
                        self.reload();
                        ui.close_menu();
                    }
                    if ui.button("💾 Save As...").clicked() {
                        self.save_as();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("📁 Switch Project...").clicked() {
                        self.load_available_projects();
                        self.show_switch_project_dialog = true;
                        ui.close_menu();
                    }
                    if ui.button("📂 Open Project...").clicked() {
                        // Open a file dialog to select a project file
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("YAML Files", &["yaml", "yml"])
                            .add_filter("All Files", &["*"])
                            .set_title("Open Project")
                            .pick_file()
                        {
                            Self::open_project_in_new_window(path.to_string_lossy().to_string());
                        }
                        ui.close_menu();
                    }
                    if ui.button("➕ New Project...").clicked() {
                        self.clear_new_project_form();
                        self.show_new_project_dialog = true;
                        ui.close_menu();
                    }
                    if ui.button("🪟 New Window").clicked() {
                        Self::open_new_window();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("🔄 Restart").clicked() {
                        Self::restart_application();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("🚪 Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                // View menu
                ui.menu_button("👁 View", |ui| {
                    if ui.button("📋 Requirements").clicked() {
                        self.pending_view_change = Some(View::List);
                        ui.close_menu();
                    }
                    if ui.button("📊 KanBan Board").clicked() {
                        self.pending_view_change = Some(View::KanBan);
                        ui.close_menu();
                    }
                    if ui.button("🏢 Org Chart").clicked() {
                        self.pending_view_change = Some(View::OrgChart);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("🏷 Baselines").clicked() {
                        self.pending_view_change = Some(View::Baselines);
                        ui.close_menu();
                    }
                    if ui.button("📅 Timeline").clicked() {
                        self.pending_view_change = Some(View::Timeline);
                        self.rebuild_timeline_events();
                        ui.close_menu();
                    }
                    if ui.button("🏃 Sprint Planning").clicked() {
                        self.pending_view_change = Some(View::Planning);
                        ui.close_menu();
                    }
                });

                if ui.button("➕ Add").clicked() {
                    self.clear_form();
                    self.pending_view_change = Some(View::Add);
                }

                // Layout cycle button (only show in List/Detail view, not in Add/Edit forms)
                // Quick click cycles, long press (hold) or double-click shows menu
                let in_form_view = self.current_view == View::Add || self.current_view == View::Edit;
                if !in_form_view {
                    ui.separator();
                    let layout_tooltip = format!(
                        "Layout: {} (click to cycle, double-click or hold for menu)",
                        self.layout_mode.label()
                    );
                    let layout_button = ui.button("⊞")
                        .on_hover_text(&layout_tooltip);

                    // Check for double-click to show menu (and skip cycling)
                    let double_clicked = layout_button.double_clicked();
                    if double_clicked {
                        self.show_layout_menu = true;
                    }

                    // Track button press timing for long-press detection
                    if layout_button.is_pointer_button_down_on() {
                        if self.layout_button_press_start.is_none() {
                            self.layout_button_press_start = Some(std::time::Instant::now());
                        }
                        // Check if held long enough (300ms) to show menu
                        if let Some(start) = self.layout_button_press_start {
                            if start.elapsed() >= std::time::Duration::from_millis(300) {
                                self.show_layout_menu = true;
                            }
                        }
                    } else {
                        // Button released
                        if let Some(start) = self.layout_button_press_start.take() {
                            // If it was a quick click (< 300ms), menu isn't showing, and not a double-click, cycle
                            if start.elapsed() < std::time::Duration::from_millis(300)
                                && !self.show_layout_menu
                                && !double_clicked
                            {
                                self.layout_mode = self.layout_mode.next();
                            }
                        }
                    }

                    // Store button position for the popup menu
                    self.layout_button_rect = Some(layout_button.rect);
                }

                ui.separator();
                ui.label(format!("Requirements: {}", self.store.requirements.len()));

                // Show message
                if let Some((msg, is_error)) = &self.message {
                    ui.separator();
                    let color = if *is_error {
                        egui::Color32::RED
                    } else {
                        egui::Color32::GREEN
                    };
                    ui.colored_label(color, msg);
                }

                // Settings and help buttons (right-aligned)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙").on_hover_text("Settings").clicked() {
                        // Load current user settings into form
                        self.settings_form_name = self.user_settings.name.clone();
                        self.settings_form_email = self.user_settings.email.clone();
                        self.settings_form_handle = self.user_settings.handle.clone();
                        self.settings_form_font_size = self.user_settings.base_font_size;
                        self.settings_form_ui_heading_level = self.user_settings.ui_heading_level;
                        self.settings_form_perspective =
                            self.user_settings.preferred_perspective.clone();
                        self.settings_form_theme = self.user_settings.theme.clone();
                        self.settings_form_keybindings = self.user_settings.keybindings.clone();
                        self.settings_form_show_status_icons =
                            self.user_settings.show_status_icons;
                        self.settings_form_status_icons =
                            self.user_settings.status_icons.clone();
                        self.settings_form_priority_icons =
                            self.user_settings.priority_icons.clone();
                        self.capturing_key_for = None;

                        // Store original appearance settings for Cancel reversion
                        self.original_appearance_theme = self.user_settings.theme.clone();
                        self.original_appearance_font_size = self.user_settings.base_font_size;
                        self.original_appearance_ui_heading_level = self.user_settings.ui_heading_level;
                        self.original_appearance_show_status_icons = self.user_settings.show_status_icons;
                        self.original_appearance_status_icons = self.user_settings.status_icons.clone();
                        self.original_appearance_priority_icons = self.user_settings.priority_icons.clone();

                        // Load current project settings into form
                        self.settings_form_id_format = self.store.id_config.format.clone();
                        self.settings_form_numbering = self.store.id_config.numbering.clone();
                        self.settings_form_digits = self.store.id_config.digits;
                        self.show_settings_dialog = true;
                    }
                    if ui
                        .button("?")
                        .on_hover_text("Help - Open User Guide")
                        .clicked()
                    {
                        Self::open_user_guide();
                    }
                    // Show current zoom level
                    ui.label(format!("{}pt", self.current_font_size as i32));
                });
            });
        });
    }

    /// Load available projects from registry
    fn load_available_projects(&mut self) {
        self.available_projects.clear();
        if let Ok(registry_path) = aida_core::get_registry_path() {
            if registry_path.exists() {
                if let Ok(registry) = aida_core::Registry::load(&registry_path) {
                    for (name, project) in &registry.projects {
                        // Only include projects whose files exist
                        let path = std::path::Path::new(&project.path);
                        if path.exists() || path.is_relative() {
                            self.available_projects.push((
                                name.clone(),
                                project.path.clone(),
                                project.description.clone(),
                            ));
                        }
                    }
                    // Sort by name
                    self.available_projects.sort_by(|a, b| a.0.cmp(&b.0));
                }
            }
        }
    }

    /// Clear the new project form
    fn clear_new_project_form(&mut self) {
        self.new_project_dir = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        self.new_project_name = String::new();
        self.new_project_title = String::new();
        self.new_project_description = String::new();
        self.new_project_template = "current".to_string();
        self.new_project_include_users = false;
    }

    /// Switch to a different project
    fn switch_project(&mut self, path: &str) {
        let path = std::path::PathBuf::from(path);
        if path.exists() {
            self.storage = Storage::new(path.clone());
            if let Ok(store) = self.storage.load() {
                self.store = store;
                self.selected_idx = None;
                self.current_view = View::List;
                self.message = Some((format!("Switched to project: {}", path.display()), false));
            } else {
                self.message = Some((format!("Failed to load project: {}", path.display()), true));
            }
        } else {
            self.message = Some((format!("Project file not found: {}", path.display()), true));
        }
    }

    /// Create a new project from template
    fn create_new_project(&mut self) -> Result<(), String> {
        use std::fs;
        use std::path::PathBuf;

        let dir = PathBuf::from(&self.new_project_dir);
        if !dir.exists() {
            return Err(format!("Directory does not exist: {}", dir.display()));
        }

        let project_file = dir.join("requirements.yaml");
        if project_file.exists() {
            return Err(format!(
                "A requirements.yaml already exists in: {}",
                dir.display()
            ));
        }

        // Create new store from template
        let mut new_store = if self.new_project_template == "current" {
            // Copy from current project
            let mut store = self.store.clone();
            store.requirements.clear();
            store.next_spec_number = 1;
            store.prefix_counters.clear();
            if !self.new_project_include_users {
                // Keep only current user
                let current_user = store
                    .users
                    .iter()
                    .find(|u| {
                        u.handle == self.user_settings.handle || u.email == self.user_settings.email
                    })
                    .cloned();
                store.users.clear();
                if let Some(user) = current_user {
                    store.users.push(user);
                }
            }
            store
        } else {
            // Load from template file
            let templates_dir = aida_core::get_templates_dir().map_err(|e| e.to_string())?;
            let template_file = templates_dir.join(format!("{}.yaml", self.new_project_template));
            if template_file.exists() {
                let content = fs::read_to_string(&template_file)
                    .map_err(|e| format!("Failed to read template: {}", e))?;
                serde_yaml::from_str::<RequirementsStore>(&content)
                    .map_err(|e| format!("Failed to parse template: {}", e))?
            } else {
                return Err(format!("Template not found: {}", self.new_project_template));
            }
        };

        // Set project title if provided
        if !self.new_project_title.is_empty() {
            new_store.title = self.new_project_title.clone();
        }

        // Save the new project file
        let content =
            serde_yaml::to_string(&new_store).map_err(|e| format!("Failed to serialize: {}", e))?;
        fs::write(&project_file, content).map_err(|e| format!("Failed to write file: {}", e))?;

        // Register in registry
        if let Ok(registry_path) = aida_core::get_registry_path() {
            let mut registry = if registry_path.exists() {
                aida_core::Registry::load(&registry_path).unwrap_or_else(|_| aida_core::Registry {
                    projects: std::collections::HashMap::new(),
                    default_project: None,
                })
            } else {
                aida_core::Registry {
                    projects: std::collections::HashMap::new(),
                    default_project: None,
                }
            };

            registry.register_project(
                self.new_project_name.clone(),
                project_file.to_string_lossy().to_string(),
                self.new_project_description.clone(),
            );

            if let Err(e) = registry.save(&registry_path) {
                return Err(format!("Failed to update registry: {}", e));
            }
        }

        // Switch to the new project
        self.switch_project(&project_file.to_string_lossy());

        Ok(())
    }

    /// Show the switch project dialog
    fn show_switch_project_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_switch_project_dialog {
            return;
        }

        let mut close_dialog = false;
        let mut switch_to: Option<String> = None;

        let max_size = modal_max_size(ctx);

        egui::Window::new("📁 Switch Project")
            .collapsible(false)
            .resizable(true)
            .min_width(400.0)
            .max_width(max_size.x)
            .max_height(max_size.y)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.heading("Available Projects");
                ui.add_space(10.0);

                if self.available_projects.is_empty() {
                    ui.label("No projects found in registry.");
                } else {
                    egui::ScrollArea::vertical()
                        .max_height((max_size.y - 100.0).max(200.0))
                        .show(ui, |ui| {
                            for (name, path, description) in &self.available_projects {
                                ui.group(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.strong(name);
                                        if ui.small_button("Open").clicked() {
                                            switch_to = Some(path.clone());
                                        }
                                    });
                                    ui.label(
                                        egui::RichText::new(path)
                                            .small()
                                            .color(egui::Color32::GRAY),
                                    );
                                    if !description.is_empty() {
                                        ui.label(description);
                                    }
                                });
                            }
                        });
                }

                ui.add_space(10.0);
                ui.separator();

                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close_dialog = true;
                    }
                });
            });

        if let Some(path) = switch_to {
            self.switch_project(&path);
            close_dialog = true;
        }

        if close_dialog {
            self.show_switch_project_dialog = false;
        }
    }

    /// Show the new project dialog
    fn show_new_project_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_new_project_dialog {
            return;
        }

        let mut close_dialog = false;
        let mut create_project = false;
        let max_size = modal_max_size(ctx);

        egui::Window::new("➕ New Project")
            .collapsible(false)
            .resizable(true)
            .min_width(450.0)
            .max_width(max_size.x)
            .max_height(max_size.y)
            .scroll([false, true])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.heading("Create New Project");
                ui.add_space(10.0);

                egui::Grid::new("new_project_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Directory:");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_project_dir)
                                    .desired_width(300.0),
                            );
                        });
                        ui.end_row();

                        ui.label("Project Name:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_project_name)
                                .hint_text("my-project")
                                .desired_width(200.0),
                        );
                        ui.end_row();

                        ui.label("Title:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_project_title)
                                .hint_text("My Project Requirements")
                                .desired_width(300.0),
                        );
                        ui.end_row();

                        ui.label("Description:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_project_description)
                                .hint_text("Brief description")
                                .desired_width(300.0),
                        );
                        ui.end_row();

                        ui.label("Template:");
                        egui::ComboBox::from_id_salt("template_combo")
                            .selected_text(&self.new_project_template)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.new_project_template,
                                    "current".to_string(),
                                    "Current Project",
                                );
                                // TODO: Add templates from ~/.config/aida/templates/
                            });
                        ui.end_row();

                        ui.label("Include Users:");
                        ui.checkbox(
                            &mut self.new_project_include_users,
                            "Copy all users from template",
                        );
                        ui.end_row();
                    });

                ui.add_space(15.0);
                ui.separator();

                ui.horizontal(|ui| {
                    let can_create =
                        !self.new_project_dir.is_empty() && !self.new_project_name.is_empty();

                    if ui
                        .add_enabled(can_create, egui::Button::new("Create"))
                        .clicked()
                    {
                        create_project = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close_dialog = true;
                    }
                });
            });

        if create_project {
            match self.create_new_project() {
                Ok(()) => {
                    self.message = Some(("Project created successfully".to_string(), false));
                    close_dialog = true;
                }
                Err(e) => {
                    self.message = Some((e, true));
                }
            }
        }

        if close_dialog {
            self.show_new_project_dialog = false;
        }
    }

    // trace:FR-0152 | ai:claude:high
    /// Show the project scaffolding dialog for generating Claude Code artifacts
    fn show_scaffold_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_scaffold_dialog {
            return;
        }

        let mut close_dialog = false;
        let mut apply_scaffolding = false;
        let max_size = modal_max_size(ctx);

        egui::Window::new("🔧 Scaffold Project")
            .collapsible(false)
            .resizable(true)
            .min_width(500.0)
            .max_width(max_size.x)
            .max_height(max_size.y)
            .scroll([false, true])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.heading("Generate Claude Code Integration");
                ui.add_space(5.0);
                ui.label("Configure and generate Claude Code artifacts for this project.");
                ui.add_space(10.0);

                // Configuration options
                ui.heading("Artifacts to Generate");
                ui.add_space(5.0);

                ui.checkbox(&mut self.scaffold_config.generate_claude_md, "CLAUDE.md")
                    .on_hover_text("Project instructions file for Claude Code");
                ui.checkbox(&mut self.scaffold_config.generate_commands, ".claude/commands/")
                    .on_hover_text("Slash commands directory for project-specific commands");
                ui.checkbox(&mut self.scaffold_config.generate_skills, ".claude/skills/")
                    .on_hover_text("Skills directory for requirements-driven development");

                if self.scaffold_config.generate_skills {
                    ui.indent("skills_indent", |ui| {
                        ui.checkbox(&mut self.scaffold_config.include_aida_req_skill, "Include aida-req skill")
                            .on_hover_text("Skill for adding requirements with AI evaluation");
                        ui.checkbox(&mut self.scaffold_config.include_aida_implement_skill, "Include aida-implement skill")
                            .on_hover_text("Skill for implementing requirements with traceability");
                    });
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                // Project type selection
                ui.heading("Project Type");
                ui.add_space(5.0);
                ui.label("Select project type to customize generated content:");

                egui::ComboBox::from_id_salt("project_type_combo")
                    .selected_text(self.scaffold_config.project_type.label())
                    .show_ui(ui, |ui| {
                        for project_type in aida_core::ProjectType::all() {
                            ui.selectable_value(
                                &mut self.scaffold_config.project_type,
                                project_type.clone(),
                                project_type.label(),
                            );
                        }
                    });

                ui.add_space(10.0);

                // Tech stack input
                ui.heading("Tech Stack");
                ui.add_space(5.0);
                ui.label("Add technologies used in this project:");

                ui.horizontal(|ui| {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.scaffold_tech_stack_input)
                            .hint_text("e.g., Rust, egui, SQLite")
                            .desired_width(200.0),
                    );
                    if ui.button("Add").clicked() || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                        let tech = self.scaffold_tech_stack_input.trim().to_string();
                        if !tech.is_empty() && !self.scaffold_config.tech_stack.contains(&tech) {
                            self.scaffold_config.tech_stack.push(tech);
                            self.scaffold_tech_stack_input.clear();
                        }
                    }
                });

                if !self.scaffold_config.tech_stack.is_empty() {
                    ui.add_space(5.0);
                    ui.horizontal_wrapped(|ui| {
                        let mut to_remove = None;
                        for (idx, tech) in self.scaffold_config.tech_stack.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(format!("• {}", tech));
                                if ui.small_button("✕").clicked() {
                                    to_remove = Some(idx);
                                }
                            });
                        }
                        if let Some(idx) = to_remove {
                            self.scaffold_config.tech_stack.remove(idx);
                        }
                    });
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                // Preview section
                ui.heading("Preview");
                ui.add_space(5.0);

                if let Some(ref preview) = self.scaffold_preview {
                    // Show what will be created/overwritten
                    if !preview.new_dirs.is_empty() {
                        ui.label("📁 Directories to create:");
                        for dir in &preview.new_dirs {
                            ui.label(format!("   • {}", dir.display()));
                        }
                        ui.add_space(5.0);
                    }

                    if !preview.new_files.is_empty() {
                        ui.colored_label(egui::Color32::from_rgb(100, 200, 100), "📄 New files:");
                        for file in &preview.new_files {
                            ui.label(format!("   • {}", file.display()));
                        }
                        ui.add_space(5.0);
                    }

                    if !preview.overwrites.is_empty() {
                        ui.colored_label(egui::Color32::from_rgb(255, 180, 100), "⚠ Files to overwrite:");
                        for file in &preview.overwrites {
                            ui.label(format!("   • {}", file.display()));
                        }
                        ui.add_space(5.0);
                    }

                    // Artifact details (collapsible)
                    ui.add_space(5.0);
                    egui::CollapsingHeader::new("📋 Artifact Details")
                        .default_open(false)
                        .show(ui, |ui| {
                            for artifact in &preview.artifacts {
                                ui.horizontal(|ui| {
                                    if artifact.exists {
                                        ui.colored_label(egui::Color32::from_rgb(255, 180, 100), "⚠");
                                    } else {
                                        ui.colored_label(egui::Color32::from_rgb(100, 200, 100), "✓");
                                    }
                                    ui.label(artifact.path.display().to_string());
                                });
                                ui.indent("artifact_desc", |ui| {
                                    ui.label(&artifact.description);
                                });
                            }
                        });
                } else {
                    ui.label("No preview available");
                }

                ui.add_space(15.0);
                ui.separator();
                ui.add_space(10.0);

                // Buttons
                ui.horizontal(|ui| {
                    // Refresh preview button
                    if ui.button("🔄 Refresh Preview").clicked() {
                        let project_dir = self.storage.path().parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| std::path::PathBuf::from("."));
                        let scaffolder = aida_core::Scaffolder::new(project_dir, self.scaffold_config.clone());
                        self.scaffold_preview = Some(scaffolder.preview(&self.store));
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            close_dialog = true;
                        }
                        if ui.button("Apply").clicked() {
                            apply_scaffolding = true;
                        }
                    });
                });
            });

        if apply_scaffolding {
            if let Some(ref preview) = self.scaffold_preview {
                let project_dir = self.storage.path().parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let scaffolder = aida_core::Scaffolder::new(project_dir, self.scaffold_config.clone());

                match scaffolder.apply(preview) {
                    Ok(files) => {
                        self.message = Some((format!("Scaffolding complete! Created {} files", files.len()), false));
                        close_dialog = true;
                    }
                    Err(e) => {
                        self.message = Some((format!("Scaffolding failed: {}", e), true));
                    }
                }
            }
        }

        if close_dialog {
            self.show_scaffold_dialog = false;
            self.scaffold_preview = None;
        }
    }

    // trace:FR-0153 | ai:claude:high
    /// Show the conflict resolution dialog when a save conflict is detected
    fn show_conflict_resolution_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_conflict_dialog {
            return;
        }

        let conflict = match &self.current_conflict {
            Some(c) => c.clone(),
            None => {
                self.show_conflict_dialog = false;
                return;
            }
        };

        let mut close_dialog = false;
        let mut resolution: Option<ConflictResolution> = None;
        let max_size = modal_max_size(ctx);

        egui::Window::new("⚠ Conflict Detected")
            .collapsible(false)
            .resizable(true)
            .min_width(600.0)
            .max_width(max_size.x)
            .max_height(max_size.y)
            .scroll([false, true])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.heading("Save Conflict");
                ui.add_space(5.0);
                ui.colored_label(
                    egui::Color32::from_rgb(255, 180, 100),
                    format!(
                        "Requirement {} was modified externally while you were editing it.",
                        conflict.spec_id
                    ),
                );
                ui.add_space(10.0);

                // Show conflicting fields
                ui.heading("Conflicting Fields");
                ui.add_space(5.0);

                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for field in &conflict.conflicting_fields {
                            ui.group(|ui| {
                                ui.strong(&field.field_name);
                                ui.add_space(5.0);

                                egui::Grid::new(format!("conflict_{}", field.field_name))
                                    .num_columns(2)
                                    .spacing([10.0, 5.0])
                                    .show(ui, |ui| {
                                        ui.colored_label(
                                            egui::Color32::from_rgb(100, 150, 255),
                                            "Your change:",
                                        );
                                        ui.add(
                                            egui::TextEdit::multiline(
                                                &mut field.local_value.clone(),
                                            )
                                            .desired_rows(2)
                                            .desired_width(250.0)
                                            .interactive(false),
                                        );
                                        ui.end_row();

                                        ui.colored_label(
                                            egui::Color32::from_rgb(255, 150, 100),
                                            "External change:",
                                        );
                                        ui.add(
                                            egui::TextEdit::multiline(&mut field.disk_value.clone())
                                                .desired_rows(2)
                                                .desired_width(250.0)
                                                .interactive(false),
                                        );
                                        ui.end_row();
                                    });
                            });
                            ui.add_space(5.0);
                        }
                    });

                ui.add_space(15.0);
                ui.separator();
                ui.add_space(10.0);

                // Resolution options
                ui.heading("Resolution Options");
                ui.add_space(5.0);

                ui.horizontal(|ui| {
                    if ui
                        .button("📥 Keep My Changes")
                        .on_hover_text("Overwrite the external changes with your version")
                        .clicked()
                    {
                        resolution = Some(ConflictResolution::ForceLocal);
                    }

                    if ui
                        .button("📤 Keep External Changes")
                        .on_hover_text("Discard your changes and keep the external version")
                        .clicked()
                    {
                        resolution = Some(ConflictResolution::KeepDisk);
                    }

                    if ui
                        .button("🔀 Merge")
                        .on_hover_text("Keep your field changes, merge comments/history/relationships")
                        .clicked()
                    {
                        resolution = Some(ConflictResolution::Merge);
                    }
                });

                ui.add_space(10.0);

                if ui.button("Cancel").clicked() {
                    close_dialog = true;
                }
            });

        if let Some(res) = resolution {
            let req_id = conflict.requirement_id;
            match self.storage.save_with_resolution(&self.store, req_id, res) {
                Ok(new_store) => {
                    self.store = new_store;
                    self.original_timestamps = Storage::get_requirement_timestamps(&self.store);
                    self.modified_requirement_ids.remove(&req_id);
                    self.message = Some((
                        format!("Conflict resolved for {}", conflict.spec_id),
                        false,
                    ));
                    close_dialog = true;
                }
                Err(e) => {
                    self.message = Some((format!("Error resolving conflict: {}", e), true));
                }
            }
        }

        if close_dialog {
            self.show_conflict_dialog = false;
            self.current_conflict = None;
        }
    }

    fn show_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_settings_dialog {
            return;
        }

        let max_size = modal_max_size(ctx);
        // Constrain Settings window width to be reasonable - max 800px or 90% of screen
        let settings_max_width = 800.0_f32.min(max_size.x * 0.9);

        egui::Window::new("⚙ Settings")
            .collapsible(false)
            .resizable(true)
            .min_width(400.0)
            .max_width(settings_max_width)
            .max_height(max_size.y)
            .scroll([false, true]) // Enable vertical scrolling for the whole window
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                // Constrain content width to prevent overflow
                ui.set_max_width(settings_max_width - 30.0);
                // Tabs
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.settings_tab, SettingsTab::User, "👤 User");
                    ui.selectable_value(
                        &mut self.settings_tab,
                        SettingsTab::Appearance,
                        "🎨 Appearance",
                    );
                    ui.selectable_value(&mut self.settings_tab, SettingsTab::Keybindings, "⌨ Keys");
                    ui.selectable_value(&mut self.settings_tab, SettingsTab::IDs, "🔢 IDs");
                    ui.selectable_value(
                        &mut self.settings_tab,
                        SettingsTab::Relationships,
                        "🔗 Relations",
                    );
                    ui.selectable_value(
                        &mut self.settings_tab,
                        SettingsTab::Reactions,
                        "😊 Reactions",
                    );
                    ui.selectable_value(
                        &mut self.settings_tab,
                        SettingsTab::TypeDefinitions,
                        "📝 Types",
                    );
                    ui.selectable_value(&mut self.settings_tab, SettingsTab::Members, "👥 Members");
                    ui.selectable_value(&mut self.settings_tab, SettingsTab::AiPrompts, "✦ AI");
                    ui.selectable_value(&mut self.settings_tab, SettingsTab::Database, "🗄 Db");
                });

                ui.separator();
                ui.add_space(10.0);

                // Tab content
                match self.settings_tab {
                    SettingsTab::User => {
                        self.show_settings_user_tab(ui);
                    }
                    SettingsTab::Appearance => {
                        self.show_settings_appearance_tab(ui);
                    }
                    SettingsTab::Keybindings => {
                        self.show_settings_keybindings_tab(ui, ctx);
                    }
                    SettingsTab::IDs => {
                        self.show_settings_ids_tab(ui);
                    }
                    SettingsTab::Relationships => {
                        self.show_settings_relationships_tab(ui);
                    }
                    SettingsTab::Reactions => {
                        self.show_settings_reactions_tab(ui);
                    }
                    SettingsTab::TypeDefinitions => {
                        self.show_settings_type_definitions_tab(ui);
                    }
                    SettingsTab::Members => {
                        self.show_settings_members_tab(ui);
                    }
                    SettingsTab::Database => {
                        self.show_settings_database_tab(ui);
                    }
                    SettingsTab::AiPrompts => {
                        self.show_settings_ai_prompts_tab(ui);
                    }
                }

                ui.add_space(15.0);
                ui.separator();
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if ui.button("💾 Save").clicked() {
                        // Update user settings from form
                        self.user_settings.name = self.settings_form_name.clone();
                        self.user_settings.email = self.settings_form_email.clone();
                        self.user_settings.handle = self.settings_form_handle.clone();
                        self.user_settings.base_font_size = self.settings_form_font_size;
                        self.user_settings.ui_heading_level = self.settings_form_ui_heading_level;
                        self.user_settings.preferred_perspective =
                            self.settings_form_perspective.clone();
                        self.user_settings.theme = self.settings_form_theme.clone();
                        self.user_settings.keybindings = self.settings_form_keybindings.clone();
                        self.user_settings.show_status_icons =
                            self.settings_form_show_status_icons;
                        self.user_settings.status_icons =
                            self.settings_form_status_icons.clone();
                        self.user_settings.priority_icons =
                            self.settings_form_priority_icons.clone();

                        // Update project settings (stored in requirements file)
                        self.store.id_config.format = self.settings_form_id_format.clone();
                        self.store.id_config.numbering = self.settings_form_numbering.clone();
                        self.store.id_config.digits = self.settings_form_digits;

                        // Apply the new base font size as current
                        self.current_font_size = self.settings_form_font_size;

                        // Apply the new preferred perspective
                        self.perspective = self.settings_form_perspective.clone();

                        // Theme will be applied on next frame via update()

                        // Save user settings to file
                        let mut save_success = true;
                        match self.user_settings.save() {
                            Ok(()) => {}
                            Err(e) => {
                                self.message =
                                    Some((format!("Failed to save user settings: {}", e), true));
                                save_success = false;
                            }
                        }

                        // Save project settings (requirements store) to file
                        if save_success {
                            match self.storage.save(&self.store) {
                                Ok(()) => {
                                    self.message =
                                        Some(("Settings saved successfully".to_string(), false));
                                }
                                Err(e) => {
                                    self.message = Some((
                                        format!("Failed to save project settings: {}", e),
                                        true,
                                    ));
                                }
                            }
                        }
                        self.show_settings_dialog = false;
                    }

                    if ui.button("❌ Cancel").clicked() {
                        // Revert appearance settings to original values (live preview cleanup)
                        self.user_settings.theme = self.original_appearance_theme.clone();
                        self.user_settings.base_font_size = self.original_appearance_font_size;
                        self.current_font_size = self.original_appearance_font_size;
                        self.user_settings.ui_heading_level = self.original_appearance_ui_heading_level;
                        self.user_settings.show_status_icons = self.original_appearance_show_status_icons;
                        self.user_settings.status_icons = self.original_appearance_status_icons.clone();
                        self.user_settings.priority_icons = self.original_appearance_priority_icons.clone();
                        self.show_settings_dialog = false;
                    }
                });
            });
    }

    fn show_settings_user_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("User Profile");
        ui.add_space(10.0);

        egui::Grid::new("settings_user_grid")
            .num_columns(2)
            .spacing([20.0, 10.0])
            .show(ui, |ui| {
                ui.label("Name:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings_form_name)
                        .hint_text("Your full name"),
                );
                ui.end_row();

                ui.label("Email:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings_form_email)
                        .hint_text("your.email@example.com"),
                );
                ui.end_row();

                ui.label("Handle (@):");
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings_form_handle)
                        .hint_text("nickname for @mentions"),
                );
                ui.end_row();
            });
    }

    fn show_settings_appearance_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Display Settings");
        ui.add_space(10.0);

        egui::Grid::new("settings_appearance_grid")
            .num_columns(2)
            .spacing([20.0, 10.0])
            .show(ui, |ui| {
                ui.label("Theme:");
                ui.horizontal(|ui| {
                    // Track if theme changed to apply it immediately
                    let old_theme = self.settings_form_theme.clone();

                    // Collect saved custom themes first to avoid borrow issues
                    let saved_customs: Vec<(String, CustomTheme)> = self
                        .user_settings
                        .custom_themes
                        .iter()
                        .map(|ct| (ct.name.clone(), ct.clone()))
                        .collect();

                    egui::ComboBox::from_id_salt("settings_theme_combo")
                        .selected_text(self.settings_form_theme.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.settings_form_theme,
                                Theme::Dark,
                                Theme::Dark.label(),
                            );
                            ui.selectable_value(
                                &mut self.settings_form_theme,
                                Theme::Light,
                                Theme::Light.label(),
                            );
                            ui.selectable_value(
                                &mut self.settings_form_theme,
                                Theme::HighContrastDark,
                                Theme::HighContrastDark.label(),
                            );
                            ui.selectable_value(
                                &mut self.settings_form_theme,
                                Theme::SolarizedDark,
                                Theme::SolarizedDark.label(),
                            );
                            ui.selectable_value(
                                &mut self.settings_form_theme,
                                Theme::Nord,
                                Theme::Nord.label(),
                            );

                            // Show saved custom themes
                            if !saved_customs.is_empty() {
                                ui.separator();
                                for (name, custom) in saved_customs {
                                    ui.selectable_value(
                                        &mut self.settings_form_theme,
                                        Theme::Custom(Box::new(custom)),
                                        format!("Custom: {}", name),
                                    );
                                }
                            }
                        });

                    // Apply theme change immediately for live preview
                    if self.settings_form_theme != old_theme {
                        self.user_settings.theme = self.settings_form_theme.clone();
                    }

                    if ui.button("🎨 Edit Theme...").clicked() {
                        // Store original theme for Cancel
                        self.theme_editor_original_theme = self.settings_form_theme.clone();

                        // Initialize the theme editor with current theme or create new
                        self.theme_editor_theme = if let Theme::Custom(ref custom) = self.settings_form_theme {
                            (**custom).clone()
                        } else {
                            // Create a new custom theme based on current theme
                            let base = match self.settings_form_theme {
                                Theme::Light => BaseTheme::Light,
                                _ => BaseTheme::Dark,
                            };
                            CustomTheme::from_base(base, "My Theme".to_string())
                        };
                        self.theme_editor_category = ThemeEditorCategory::default();
                        self.show_theme_editor = true;
                    }
                });
                ui.end_row();

                ui.label("Base Font Size:");
                ui.horizontal(|ui| {
                    let old_font_size = self.settings_form_font_size;
                    ui.add(
                        egui::Slider::new(
                            &mut self.settings_form_font_size,
                            MIN_FONT_SIZE..=MAX_FONT_SIZE,
                        )
                        .suffix("pt")
                        .step_by(1.0),
                    );
                    if ui.button("Reset").clicked() {
                        self.settings_form_font_size = DEFAULT_FONT_SIZE;
                    }
                    // Apply font size change immediately for live preview
                    if self.settings_form_font_size != old_font_size {
                        self.user_settings.base_font_size = self.settings_form_font_size;
                        self.current_font_size = self.settings_form_font_size;
                    }
                });
                ui.end_row();

                ui.label("UI Title Size:");
                ui.horizontal(|ui| {
                    let old_heading_level = self.settings_form_ui_heading_level;
                    egui::ComboBox::from_id_salt("settings_heading_level_combo")
                        .selected_text(format!("H{}", self.settings_form_ui_heading_level))
                        .show_ui(ui, |ui| {
                            for level in 1..=6_u8 {
                                ui.selectable_value(
                                    &mut self.settings_form_ui_heading_level,
                                    level,
                                    format!("H{}", level),
                                );
                            }
                        });
                    if ui.button("Reset").clicked() {
                        self.settings_form_ui_heading_level = default_ui_heading_level();
                    }
                    // Apply heading level change immediately for live preview
                    if self.settings_form_ui_heading_level != old_heading_level {
                        self.user_settings.ui_heading_level = self.settings_form_ui_heading_level;
                    }
                });
                ui.end_row();

                ui.label("Default View:");
                egui::ComboBox::from_id_salt("settings_perspective_combo")
                    .selected_text(self.settings_form_perspective.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.settings_form_perspective,
                            Perspective::Flat,
                            Perspective::Flat.label(),
                        );
                        ui.selectable_value(
                            &mut self.settings_form_perspective,
                            Perspective::ParentChild,
                            Perspective::ParentChild.label(),
                        );
                        ui.selectable_value(
                            &mut self.settings_form_perspective,
                            Perspective::Verification,
                            Perspective::Verification.label(),
                        );
                        ui.selectable_value(
                            &mut self.settings_form_perspective,
                            Perspective::References,
                            Perspective::References.label(),
                        );
                        ui.selectable_value(
                            &mut self.settings_form_perspective,
                            Perspective::ByOwner,
                            Perspective::ByOwner.label(),
                        );
                    });
                ui.end_row();

                ui.label("Status Icons:");
                ui.horizontal(|ui| {
                    let old_show_icons = self.settings_form_show_status_icons;
                    ui.checkbox(
                        &mut self.settings_form_show_status_icons,
                        "Show status icons",
                    );
                    // Apply status icons toggle immediately for live preview
                    if self.settings_form_show_status_icons != old_show_icons {
                        self.user_settings.show_status_icons = self.settings_form_show_status_icons;
                    }
                    if ui.button("Edit Icons...").clicked() {
                        self.show_icon_editor = true;
                    }
                });
                ui.end_row();

                ui.label("KanBan Card Details:");
                ui.horizontal(|ui| {
                    let old_action = self.user_settings.kanban_click_action;
                    egui::ComboBox::from_id_salt("settings_kanban_click_action")
                        .selected_text(self.user_settings.kanban_click_action.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.user_settings.kanban_click_action,
                                KanBanClickAction::RightClick,
                                KanBanClickAction::RightClick.label(),
                            );
                            ui.selectable_value(
                                &mut self.user_settings.kanban_click_action,
                                KanBanClickAction::SingleClick,
                                KanBanClickAction::SingleClick.label(),
                            );
                        });
                    ui.weak("(opens detail modal)");
                    // Save if changed
                    if self.user_settings.kanban_click_action != old_action {
                        let _ = self.user_settings.save();
                    }
                });
                ui.end_row();
            });

        ui.add_space(5.0);
        ui.label("Tip: Use Ctrl+MouseWheel or Ctrl+Plus/Minus to zoom");
    }

    /// Show the icon editor dialog
    fn show_icon_editor_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_icon_editor {
            return;
        }

        let max_size = modal_max_size(ctx);
        let icon_height = 700.0_f32.min(max_size.y);

        egui::Window::new("Status & Priority Icons")
            .collapsible(false)
            .resizable(true)
            .default_width(650.0)
            .default_height(icon_height)
            .max_width(max_size.x)
            .max_height(max_size.y)
            .scroll([false, true])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.heading("Configure Status Icons");
                ui.add_space(5.0);
                ui.label("Define icons for different status keywords. Icons are matched using 'contains' matching.");
                ui.add_space(10.0);

                // Status icons section
                ui.group(|ui| {
                    ui.heading("Status Icons");
                    ui.add_space(5.0);

                    // Get sorted list of status keywords
                    let mut status_keys: Vec<String> =
                        self.settings_form_status_icons.icons.keys().cloned().collect();
                    status_keys.sort();

                    const ITEMS_PER_ROW: usize = 4;

                    egui::ScrollArea::vertical()
                        .id_salt("status_icons_scroll")
                        .max_height(250.0)
                        .show(ui, |ui| {
                            let mut to_remove: Option<String> = None;
                            // 4 entries per row, each with: Keyword, Icon, Delete = 12 columns
                            egui::Grid::new("status_icons_grid")
                                .num_columns(ITEMS_PER_ROW * 3)
                                .spacing([8.0, 4.0])
                                .show(ui, |ui| {
                                    // Header row
                                    for _ in 0..ITEMS_PER_ROW {
                                        ui.label(egui::RichText::new("Keyword").strong());
                                        ui.label(egui::RichText::new("Icon").strong());
                                        ui.label("");
                                    }
                                    ui.end_row();

                                    // Data rows - wrap after every ITEMS_PER_ROW entries
                                    for (idx, key) in status_keys.iter().enumerate() {
                                        ui.label(key);
                                        if let Some(icon) = self.settings_form_status_icons.icons.get_mut(key) {
                                            let response = ui.add(
                                                egui::TextEdit::singleline(icon).desired_width(40.0),
                                            );
                                            if response.clicked() {
                                                self.symbol_picker_target = Some(format!("status:{}", key));
                                                self.show_symbol_picker = true;
                                            }
                                        }
                                        if ui.small_button("🗑").on_hover_text("Remove").clicked() {
                                            to_remove = Some(key.clone());
                                        }
                                        // End row after every ITEMS_PER_ROW entries
                                        if (idx + 1) % ITEMS_PER_ROW == 0 {
                                            ui.end_row();
                                        }
                                    }
                                    // End final row if not complete
                                    if !status_keys.is_empty() && status_keys.len() % ITEMS_PER_ROW != 0 {
                                        ui.end_row();
                                    }
                                });
                            if let Some(key) = to_remove {
                                self.settings_form_status_icons.icons.remove(&key);
                            }
                        });

                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label("Default:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings_form_status_icons.default_icon)
                                .desired_width(60.0),
                        );
                    });

                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label("Add new:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.icon_editor_new_keyword)
                                .desired_width(100.0)
                                .hint_text("keyword"),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.icon_editor_new_icon)
                                .desired_width(60.0)
                                .hint_text("icon"),
                        );
                        if ui.button("Add Status").clicked()
                            && !self.icon_editor_new_keyword.is_empty()
                        {
                            let keyword = self.icon_editor_new_keyword.to_lowercase();
                            let icon = if self.icon_editor_new_icon.is_empty() {
                                "[*]".to_string()
                            } else {
                                self.icon_editor_new_icon.clone()
                            };
                            self.settings_form_status_icons.icons.insert(keyword, icon);
                            self.icon_editor_new_keyword.clear();
                            self.icon_editor_new_icon.clear();
                        }
                    });
                });

                ui.add_space(15.0);

                // Priority icons section
                ui.group(|ui| {
                    ui.heading("Priority Icons");
                    ui.add_space(5.0);

                    let mut priority_keys: Vec<String> =
                        self.settings_form_priority_icons.icons.keys().cloned().collect();
                    priority_keys.sort();

                    const ITEMS_PER_ROW: usize = 4;

                    egui::ScrollArea::vertical()
                        .id_salt("priority_icons_scroll")
                        .max_height(200.0)
                        .show(ui, |ui| {
                            let mut to_remove: Option<String> = None;
                            // 4 entries per row, each with: Priority, Icon, Delete = 12 columns
                            egui::Grid::new("priority_icons_grid")
                                .num_columns(ITEMS_PER_ROW * 3)
                                .spacing([8.0, 4.0])
                                .show(ui, |ui| {
                                    // Header row
                                    for _ in 0..ITEMS_PER_ROW {
                                        ui.label(egui::RichText::new("Priority").strong());
                                        ui.label(egui::RichText::new("Icon").strong());
                                        ui.label("");
                                    }
                                    ui.end_row();

                                    // Data rows - wrap after every ITEMS_PER_ROW entries
                                    for (idx, key) in priority_keys.iter().enumerate() {
                                        ui.label(key);
                                        if let Some(icon) = self.settings_form_priority_icons.icons.get_mut(key) {
                                            ui.add(
                                                egui::TextEdit::singleline(icon).desired_width(40.0),
                                            );
                                        }
                                        if ui.small_button("🗑").on_hover_text("Remove").clicked() {
                                            to_remove = Some(key.clone());
                                        }
                                        // End row after every ITEMS_PER_ROW entries
                                        if (idx + 1) % ITEMS_PER_ROW == 0 {
                                            ui.end_row();
                                        }
                                    }
                                    // End final row if not complete
                                    if !priority_keys.is_empty() && priority_keys.len() % ITEMS_PER_ROW != 0 {
                                        ui.end_row();
                                    }
                                });
                            if let Some(key) = to_remove {
                                self.settings_form_priority_icons.icons.remove(&key);
                            }
                        });

                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label("Default:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings_form_priority_icons.default_icon)
                                .desired_width(60.0),
                        );
                    });
                });

                ui.add_space(15.0);

                // Symbol picker section
                ui.group(|ui| {
                    ui.heading("Quick Symbols");
                    ui.label("Click to copy, then paste into an icon field:");
                    ui.add_space(5.0);

                    // Common symbols organized by category
                    let symbols = vec![
                        ("Checkmarks", vec!["✓", "✗", "✔", "✘", "☑", "☐", "☒"]),
                        ("Shapes", vec!["●", "○", "◆", "◇", "■", "□", "▸", "▹", "◐", "◑"]),
                        ("Brackets", vec!["[x]", "[ ]", "[-]", "[+]", "[~]", "[?]", "[!]", "[.]", "[*]"]),
                        ("Arrows", vec!["→", "←", "↑", "↓", "⇒", "⇐", "↔", "⟹"]),
                        ("Stars", vec!["★", "☆", "✦", "✧", "⭐", "✪"]),
                        ("Misc", vec!["•", "·", "‣", "⁃", "※", "†", "‡", "§"]),
                    ];

                    for (category, syms) in symbols {
                        ui.horizontal(|ui| {
                            ui.label(format!("{}:", category));
                            for sym in syms {
                                if ui.small_button(sym).on_hover_text("Click to copy").clicked() {
                                    ui.output_mut(|o| o.copied_text = sym.to_string());
                                }
                            }
                        });
                    }
                });

                ui.add_space(15.0);

                // Apply icon changes for live preview
                self.user_settings.status_icons = self.settings_form_status_icons.clone();
                self.user_settings.priority_icons = self.settings_form_priority_icons.clone();

                // Buttons
                ui.horizontal(|ui| {
                    if ui.button("Reset to Defaults").clicked() {
                        self.settings_form_status_icons = StatusIconConfig::default();
                        self.settings_form_priority_icons = PriorityIconConfig::default();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.show_icon_editor = false;
                        }
                    });
                });
            });
    }

    fn show_settings_keybindings_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Keyboard Shortcuts");
        ui.add_space(10.0);

        // If we're capturing a key, show instructions
        if let Some(action) = self.capturing_key_for {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("Press a key for '{}' (Escape to cancel)", action.label()),
            );
            ui.add_space(5.0);

            // Get the current context for this action (to preserve it)
            let current_context = self
                .settings_form_keybindings
                .bindings
                .get(&action)
                .map(|b| b.context)
                .unwrap_or(action.default_context());

            // Check for key press
            let captured = ctx.input(|i| {
                // Cancel with Escape
                if i.key_pressed(egui::Key::Escape) {
                    return Some(None);
                }
                // Check for any key press
                for key in [
                    egui::Key::ArrowUp,
                    egui::Key::ArrowDown,
                    egui::Key::ArrowLeft,
                    egui::Key::ArrowRight,
                    egui::Key::Enter,
                    egui::Key::Space,
                    egui::Key::Tab,
                    egui::Key::Backspace,
                    egui::Key::Delete,
                    egui::Key::Home,
                    egui::Key::End,
                    egui::Key::PageUp,
                    egui::Key::PageDown,
                    egui::Key::Plus,
                    egui::Key::Minus,
                    egui::Key::Equals,
                    egui::Key::Num0,
                    egui::Key::Num1,
                    egui::Key::Num2,
                    egui::Key::Num3,
                    egui::Key::Num4,
                    egui::Key::Num5,
                    egui::Key::Num6,
                    egui::Key::Num7,
                    egui::Key::Num8,
                    egui::Key::Num9,
                    egui::Key::A,
                    egui::Key::B,
                    egui::Key::C,
                    egui::Key::D,
                    egui::Key::E,
                    egui::Key::F,
                    egui::Key::G,
                    egui::Key::H,
                    egui::Key::I,
                    egui::Key::J,
                    egui::Key::K,
                    egui::Key::L,
                    egui::Key::M,
                    egui::Key::N,
                    egui::Key::O,
                    egui::Key::P,
                    egui::Key::Q,
                    egui::Key::R,
                    egui::Key::S,
                    egui::Key::T,
                    egui::Key::U,
                    egui::Key::V,
                    egui::Key::W,
                    egui::Key::X,
                    egui::Key::Y,
                    egui::Key::Z,
                    egui::Key::F1,
                    egui::Key::F2,
                    egui::Key::F3,
                    egui::Key::F4,
                    egui::Key::F5,
                    egui::Key::F6,
                    egui::Key::F7,
                    egui::Key::F8,
                    egui::Key::F9,
                    egui::Key::F10,
                    egui::Key::F11,
                    egui::Key::F12,
                ] {
                    if i.key_pressed(key) {
                        let binding = KeyBinding {
                            key_name: key_to_string(key).to_string(),
                            ctrl: i.modifiers.ctrl,
                            shift: i.modifiers.shift,
                            alt: i.modifiers.alt,
                            context: current_context, // Preserve the context
                        };
                        return Some(Some(binding));
                    }
                }
                None
            });

            if let Some(result) = captured {
                if let Some(binding) = result {
                    // Set the new binding
                    self.settings_form_keybindings
                        .bindings
                        .insert(action, binding);
                }
                self.capturing_key_for = None;
            }
        }

        // Keybindings table
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("keybindings_grid")
                .num_columns(4)
                .spacing([15.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    // Header
                    ui.strong("Action");
                    ui.strong("Key");
                    ui.strong("Context");
                    ui.strong("");
                    ui.end_row();

                    // Clone the actions to avoid borrow issues
                    let actions: Vec<KeyAction> = KeyAction::all().to_vec();

                    for action in actions {
                        ui.label(action.label());

                        let binding_display = self
                            .settings_form_keybindings
                            .bindings
                            .get(&action)
                            .map(|b| b.display())
                            .unwrap_or_else(|| "Unbound".to_string());

                        // Highlight if we're capturing for this action
                        if self.capturing_key_for == Some(action) {
                            ui.colored_label(egui::Color32::YELLOW, "...");
                        } else {
                            ui.monospace(&binding_display);
                        }

                        // Context selector
                        let current_context = self
                            .settings_form_keybindings
                            .bindings
                            .get(&action)
                            .map(|b| b.context)
                            .unwrap_or(action.default_context());

                        let mut selected_context = current_context;
                        egui::ComboBox::from_id_salt(format!("context_{:?}", action))
                            .width(120.0)
                            .selected_text(selected_context.label())
                            .show_ui(ui, |ui| {
                                for ctx in KeyContext::all() {
                                    ui.selectable_value(&mut selected_context, *ctx, ctx.label());
                                }
                            });

                        // Update context if changed
                        if selected_context != current_context {
                            if let Some(binding) =
                                self.settings_form_keybindings.bindings.get_mut(&action)
                            {
                                binding.context = selected_context;
                            }
                        }

                        if self.capturing_key_for.is_none() {
                            if ui.button("Change").clicked() {
                                self.capturing_key_for = Some(action);
                            }
                        } else {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
        });

        ui.add_space(10.0);

        ui.horizontal(|ui| {
            if ui.button("Reset to Defaults").clicked() {
                self.settings_form_keybindings = KeyBindings::default();
            }

            ui.add_space(20.0);
            ui.label("Context: Where the shortcut is active");
        });
    }

    fn show_settings_ids_tab(&mut self, ui: &mut egui::Ui) {
        // Subtab selector
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.ids_subtab, IdsSubTab::Format, "📐 Format");
            ui.selectable_value(&mut self.ids_subtab, IdsSubTab::Prefixes, "🏷 Prefixes");
        });
        ui.separator();
        ui.add_space(5.0);

        match self.ids_subtab {
            IdsSubTab::Format => self.show_ids_format_subtab(ui),
            IdsSubTab::Prefixes => self.show_ids_prefixes_subtab(ui),
        }
    }

    fn show_ids_format_subtab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("ID Format Configuration");
            ui.add_space(10.0);

            // Validate current settings against proposed changes
            let validation = self.store.validate_id_config_change(
                &self.settings_form_id_format,
                &self.settings_form_numbering,
                self.settings_form_digits,
            );

            egui::Grid::new("settings_project_grid")
                .num_columns(2)
                .spacing([20.0, 10.0])
                .show(ui, |ui| {
                    // ID Format selection
                    ui.label("ID Format:");
                    egui::ComboBox::from_id_salt("settings_id_format_combo")
                        .selected_text(match self.settings_form_id_format {
                            IdFormat::SingleLevel => "Single Level (PREFIX-NNN)",
                            IdFormat::TwoLevel => "Two Level (FEATURE-TYPE-NNN)",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.settings_form_id_format,
                                IdFormat::SingleLevel,
                                "Single Level (PREFIX-NNN)"
                            ).on_hover_text("e.g., AUTH-001, FR-002");
                            ui.selectable_value(
                                &mut self.settings_form_id_format,
                                IdFormat::TwoLevel,
                                "Two Level (FEATURE-TYPE-NNN)"
                            ).on_hover_text("e.g., AUTH-FR-001, PAY-NFR-001");
                        });
                    ui.end_row();

                    // Numbering Strategy selection
                    ui.label("Numbering:");
                    egui::ComboBox::from_id_salt("settings_numbering_combo")
                        .selected_text(match self.settings_form_numbering {
                            NumberingStrategy::Global => "Global Sequential",
                            NumberingStrategy::PerPrefix => "Per Prefix",
                            NumberingStrategy::PerFeatureType => "Per Feature+Type",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.settings_form_numbering,
                                NumberingStrategy::Global,
                                "Global Sequential"
                            ).on_hover_text("All requirements share one counter: AUTH-001, FR-002, PAY-003");
                            ui.selectable_value(
                                &mut self.settings_form_numbering,
                                NumberingStrategy::PerPrefix,
                                "Per Prefix"
                            ).on_hover_text("Each prefix has its own counter: AUTH-001, FR-001, PAY-001");
                            // Only show PerFeatureType for TwoLevel format
                            if self.settings_form_id_format == IdFormat::TwoLevel {
                                ui.selectable_value(
                                    &mut self.settings_form_numbering,
                                    NumberingStrategy::PerFeatureType,
                                    "Per Feature+Type"
                                ).on_hover_text("Each feature+type combo has its own counter: AUTH-FR-001, AUTH-NFR-001");
                            }
                        });
                    ui.end_row();

                    // Number of digits
                    ui.label("Digits:");
                    ui.horizontal(|ui| {
                        ui.add(egui::Slider::new(&mut self.settings_form_digits, 1..=6)
                            .step_by(1.0));
                        ui.label(format!("(e.g., {:0>width$})", 1, width = self.settings_form_digits as usize));
                    });
                    ui.end_row();
                });

            ui.add_space(10.0);

            // Show validation status
            if let Some(error) = &validation.error {
                ui.colored_label(egui::Color32::RED, format!("⚠ {}", error));
                ui.add_space(5.0);
            }

            if let Some(warning) = &validation.warning {
                ui.colored_label(egui::Color32::YELLOW, format!("ℹ {}", warning));
                ui.add_space(5.0);
            }

            // Migration button (only show if changes are valid and migration is possible)
            if validation.valid && validation.can_migrate && validation.affected_count > 0 {
                ui.add_space(5.0);
                if ui
                    .button("🔄 Migrate Existing IDs")
                    .on_hover_text("Update all existing requirement IDs to match the new format")
                    .clicked()
                {
                    self.pending_migration = Some((
                        self.settings_form_id_format.clone(),
                        self.settings_form_numbering.clone(),
                        self.settings_form_digits,
                    ));
                    self.show_migration_dialog = true;
                }
            }

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            // Show examples based on current settings
            ui.heading("Example IDs");
            ui.add_space(5.0);

            let width = self.settings_form_digits as usize;
            let examples = match self.settings_form_id_format {
                IdFormat::SingleLevel => {
                    vec![
                        format!("AUTH-{:0>width$}", 1, width = width),
                        format!("FR-{:0>width$}", 2, width = width),
                        format!("PAY-{:0>width$}", 3, width = width),
                    ]
                }
                IdFormat::TwoLevel => {
                    vec![
                        format!("AUTH-FR-{:0>width$}", 1, width = width),
                        format!("AUTH-NFR-{:0>width$}", 2, width = width),
                        format!("PAY-FR-{:0>width$}", 3, width = width),
                    ]
                }
            };

            for example in examples {
                ui.monospace(&example);
            }

            ui.add_space(10.0);
            if validation.affected_count == 0 {
                ui.colored_label(
                    egui::Color32::from_rgb(180, 180, 100),
                    "Note: Changes will apply to new requirements only.",
                );
            }
        });
    }

    fn show_ids_prefixes_subtab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Prefix Management");
            ui.add_space(10.0);

            // Toggle for restricting prefixes
            let mut restrict = self.store.restrict_prefixes;
            if ui
                .checkbox(&mut restrict, "Restrict prefixes to allowed list")
                .on_hover_text("When enabled, users must select from the allowed prefixes list. When disabled, users can enter any valid prefix.")
                .changed()
            {
                self.store.restrict_prefixes = restrict;
                self.save();
            }

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);

            // Add new prefix section
            ui.strong("Add New Prefix");
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_prefix_input)
                        .desired_width(100.0)
                        .hint_text("e.g., SEC"),
                );

                let can_add = !self.new_prefix_input.is_empty()
                    && self.new_prefix_input.chars().all(|c| c.is_ascii_alphabetic());

                if ui.add_enabled(can_add, egui::Button::new("➕ Add")).clicked() {
                    let prefix = self.new_prefix_input.to_uppercase();
                    self.store.add_allowed_prefix(&prefix);
                    self.save();
                    self.new_prefix_input.clear();
                }
            });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);

            // Show prefixes in use
            let used_prefixes = self.store.get_used_prefixes();
            ui.strong("Prefixes In Use");
            ui.add_space(5.0);

            if used_prefixes.is_empty() {
                ui.label("No prefixes currently in use.");
            } else {
                // Show count of requirements per prefix
                let mut prefix_counts: Vec<(String, usize)> = used_prefixes.iter().map(|p| {
                    let count = self.store.requirements.iter()
                        .filter(|r| r.spec_id.as_ref().map(|id| id.starts_with(p)).unwrap_or(false))
                        .count();
                    (p.clone(), count)
                }).collect();
                prefix_counts.sort_by(|a, b| a.0.cmp(&b.0));

                egui::ScrollArea::vertical()
                    .id_salt("used_prefixes_scroll")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for (prefix, count) in &prefix_counts {
                            ui.horizontal(|ui| {
                                ui.monospace(prefix);
                                ui.label(format!("({} req{})", count, if *count == 1 { "" } else { "s" }));

                                // Rename button
                                if ui.small_button("✏").on_hover_text("Rename this prefix").clicked() {
                                    self.rename_prefix_from = prefix.clone();
                                    self.rename_prefix_to = prefix.clone();
                                    self.show_rename_prefix_dialog = true;
                                }
                            });
                        }
                    });
            }

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);

            // Allowed prefixes list
            ui.strong("Allowed Prefixes");
            ui.add_space(5.0);

            if self.store.allowed_prefixes.is_empty() {
                ui.label("No prefixes in allowed list (all valid prefixes permitted).");
            } else {
                let prefixes_to_show: Vec<String> = self.store.allowed_prefixes.clone();
                let mut to_remove: Option<String> = None;

                egui::ScrollArea::vertical()
                    .id_salt("allowed_prefixes_scroll")
                    .max_height(150.0)
                    .show(ui, |ui| {
                        for prefix in &prefixes_to_show {
                            let in_use = used_prefixes.contains(prefix);
                            ui.horizontal(|ui| {
                                ui.monospace(prefix);
                                if in_use {
                                    ui.colored_label(egui::Color32::from_rgb(100, 180, 100), "[in use]");
                                } else {
                                    ui.colored_label(egui::Color32::GRAY, "[unused]");
                                }

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui
                                        .small_button("🗑")
                                        .on_hover_text("Remove from allowed list")
                                        .clicked()
                                    {
                                        to_remove = Some(prefix.clone());
                                    }
                                });
                            });
                        }
                    });

                if let Some(prefix) = to_remove {
                    self.store.remove_allowed_prefix(&prefix);
                    self.save();
                }
            }
        });
    }

    fn show_rename_prefix_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_rename_prefix_dialog {
            return;
        }

        let max_size = modal_max_size(ctx);
        let modal_width = 400.0_f32.min(max_size.x);

        egui::Window::new("Rename Prefix")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([modal_width, 0.0])
            .show(ctx, |ui| {
                ui.add_space(10.0);

                ui.label(format!("Rename prefix '{}' to:", self.rename_prefix_from));
                ui.add_space(5.0);

                ui.add(
                    egui::TextEdit::singleline(&mut self.rename_prefix_to)
                        .desired_width(150.0)
                        .hint_text("New prefix"),
                );

                // Count affected requirements
                let affected_count = self.store.requirements.iter()
                    .filter(|r| r.spec_id.as_ref()
                        .map(|id| id.starts_with(&self.rename_prefix_from))
                        .unwrap_or(false))
                    .count();

                ui.add_space(10.0);

                let new_prefix = self.rename_prefix_to.to_uppercase();
                let is_valid = !new_prefix.is_empty()
                    && new_prefix.chars().all(|c| c.is_ascii_uppercase());
                let is_same = new_prefix == self.rename_prefix_from;
                let target_exists = self.store.get_used_prefixes().contains(&new_prefix)
                    && new_prefix != self.rename_prefix_from;

                if target_exists {
                    ui.colored_label(
                        egui::Color32::from_rgb(200, 180, 100),
                        format!("⚠ Will merge {} requirement(s) into existing prefix '{}'",
                            affected_count, new_prefix),
                    );
                } else if is_valid && !is_same {
                    ui.label(format!("Will rename {} requirement(s)", affected_count));
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let can_rename = is_valid && !is_same;
                    if ui.add_enabled(can_rename, egui::Button::new("✓ Rename")).clicked() {
                        // Perform the rename
                        self.rename_prefix(&self.rename_prefix_from.clone(), &new_prefix);
                        self.show_rename_prefix_dialog = false;
                    }
                    if ui.button("✗ Cancel").clicked() {
                        self.show_rename_prefix_dialog = false;
                    }
                });
            });
    }

    fn rename_prefix(&mut self, from: &str, to: &str) {
        // Update all requirements with this prefix
        for req in &mut self.store.requirements {
            if let Some(ref mut spec_id) = req.spec_id {
                if spec_id.starts_with(from) {
                    // Replace the prefix portion
                    let suffix = &spec_id[from.len()..];
                    *spec_id = format!("{}{}", to, suffix);
                }
            }
        }

        // Update allowed prefixes list if needed
        if self.store.allowed_prefixes.contains(&from.to_string()) {
            self.store.remove_allowed_prefix(from);
            if !self.store.allowed_prefixes.contains(&to.to_string()) {
                self.store.add_allowed_prefix(to);
            }
        }

        self.save();
    }

    fn show_settings_relationships_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Relationship Definitions");
        ui.add_space(5.0);
        ui.label("Configure relationship types and their constraints.");
        ui.add_space(10.0);

        // Add new relationship button
        if !self.show_rel_def_form {
            if ui.button("➕ Add Relationship Type").clicked() {
                self.show_rel_def_form = true;
                self.editing_rel_def = None;
                self.clear_rel_def_form();
            }
        }

        // Show form if active
        if self.show_rel_def_form {
            ui.add_space(10.0);
            self.show_rel_def_form_ui(ui);
            ui.add_space(10.0);
        }

        ui.separator();
        ui.add_space(10.0);

        // List existing relationship definitions
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .show(ui, |ui| {
                // Collect definitions to avoid borrow issues
                let definitions: Vec<_> = self.store.get_relationship_definitions().to_vec();

                for def in definitions {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            // Display name and built-in badge
                            ui.strong(&def.display_name);
                            if def.built_in {
                                ui.label(
                                    egui::RichText::new("[built-in]")
                                        .small()
                                        .color(egui::Color32::GRAY),
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // Delete button (only for non-built-in)
                                    if !def.built_in {
                                        if ui.small_button("🗑").on_hover_text("Delete").clicked()
                                        {
                                            if let Err(e) =
                                                self.store.remove_relationship_definition(&def.name)
                                            {
                                                self.message = Some((
                                                    format!("Failed to remove: {}", e),
                                                    true,
                                                ));
                                            } else {
                                                self.save();
                                                self.message = Some((
                                                    "Relationship definition removed".to_string(),
                                                    false,
                                                ));
                                            }
                                        }
                                    }
                                    // Edit button
                                    if ui.small_button("✏").on_hover_text("Edit").clicked() {
                                        self.editing_rel_def = Some(def.name.clone());
                                        self.load_rel_def_form(&def);
                                        self.show_rel_def_form = true;
                                    }
                                },
                            );
                        });

                        // Details
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&def.name).small().monospace());
                            ui.label("|");
                            if def.symmetric {
                                ui.label("↔ symmetric");
                            } else if let Some(ref inv) = def.inverse {
                                ui.label(format!("↔ {}", inv));
                            }
                            ui.label("|");
                            ui.label(format!("{}", def.cardinality));
                        });

                        if !def.description.is_empty() {
                            ui.label(
                                egui::RichText::new(&def.description)
                                    .small()
                                    .color(egui::Color32::GRAY),
                            );
                        }

                        // Show type constraints if any
                        if !def.source_types.is_empty() || !def.target_types.is_empty() {
                            ui.horizontal(|ui| {
                                if !def.source_types.is_empty() {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "From: {}",
                                            def.source_types.join(", ")
                                        ))
                                        .small(),
                                    );
                                }
                                if !def.target_types.is_empty() {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "To: {}",
                                            def.target_types.join(", ")
                                        ))
                                        .small(),
                                    );
                                }
                            });
                        }

                        // Show color if set
                        if let Some(ref color) = def.color {
                            ui.horizontal(|ui| {
                                // Try to parse and display color swatch
                                if let Some(c) = parse_hex_color(color) {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(16.0, 16.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(rect, 2.0, c);
                                }
                                ui.label(egui::RichText::new(color).small());
                            });
                        }
                    });
                    ui.add_space(5.0);
                }
            });
    }

    fn clear_rel_def_form(&mut self) {
        self.rel_def_form_name.clear();
        self.rel_def_form_display_name.clear();
        self.rel_def_form_description.clear();
        self.rel_def_form_inverse.clear();
        self.rel_def_form_symmetric = false;
        self.rel_def_form_cardinality = Cardinality::default();
        self.rel_def_form_source_types.clear();
        self.rel_def_form_target_types.clear();
        self.rel_def_form_color.clear();
    }

    fn load_rel_def_form(&mut self, def: &RelationshipDefinition) {
        self.rel_def_form_name = def.name.clone();
        self.rel_def_form_display_name = def.display_name.clone();
        self.rel_def_form_description = def.description.clone();
        self.rel_def_form_inverse = def.inverse.clone().unwrap_or_default();
        self.rel_def_form_symmetric = def.symmetric;
        self.rel_def_form_cardinality = def.cardinality.clone();
        self.rel_def_form_source_types = def.source_types.join(", ");
        self.rel_def_form_target_types = def.target_types.join(", ");
        self.rel_def_form_color = def.color.clone().unwrap_or_default();
    }

    fn show_rel_def_form_ui(&mut self, ui: &mut egui::Ui) {
        let is_editing = self.editing_rel_def.is_some();
        let is_built_in = if let Some(ref name) = self.editing_rel_def {
            self.store
                .get_relationship_definition(name)
                .map(|d| d.built_in)
                .unwrap_or(false)
        } else {
            false
        };

        ui.group(|ui| {
            let title = if is_editing {
                if is_built_in {
                    "Edit Built-in Relationship (limited)"
                } else {
                    "Edit Relationship"
                }
            } else {
                "Add Relationship Type"
            };
            ui.heading(title);
            ui.add_space(5.0);

            egui::Grid::new("rel_def_form_grid")
                .num_columns(2)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    // Name (readonly for built-in)
                    ui.label("Name:");
                    if is_editing {
                        ui.label(&self.rel_def_form_name);
                    } else {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.rel_def_form_name)
                                .hint_text("e.g., blocks"),
                        );
                    }
                    ui.end_row();

                    // Display name (editable)
                    ui.label("Display Name:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.rel_def_form_display_name)
                            .hint_text("e.g., Blocks"),
                    );
                    ui.end_row();

                    // Description (editable)
                    ui.label("Description:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.rel_def_form_description)
                            .hint_text("What this relationship means"),
                    );
                    ui.end_row();

                    // For non-built-in: inverse, symmetric, cardinality
                    if !is_built_in {
                        ui.label("Inverse:");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.rel_def_form_inverse)
                                    .hint_text("e.g., blocked_by")
                                    .desired_width(120.0),
                            );
                            ui.checkbox(&mut self.rel_def_form_symmetric, "Symmetric");
                        });
                        ui.end_row();

                        ui.label("Cardinality:");
                        egui::ComboBox::from_id_salt("cardinality_combo")
                            .selected_text(format!("{}", self.rel_def_form_cardinality))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.rel_def_form_cardinality,
                                    Cardinality::ManyToMany,
                                    "N:N (Many to Many)",
                                );
                                ui.selectable_value(
                                    &mut self.rel_def_form_cardinality,
                                    Cardinality::OneToMany,
                                    "1:N (One to Many)",
                                );
                                ui.selectable_value(
                                    &mut self.rel_def_form_cardinality,
                                    Cardinality::ManyToOne,
                                    "N:1 (Many to One)",
                                );
                                ui.selectable_value(
                                    &mut self.rel_def_form_cardinality,
                                    Cardinality::OneToOne,
                                    "1:1 (One to One)",
                                );
                            });
                        ui.end_row();
                    }

                    // Source types (editable)
                    ui.label("Source Types:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.rel_def_form_source_types)
                            .hint_text("Functional, System (comma-separated, empty = all)"),
                    );
                    ui.end_row();

                    // Target types (editable)
                    ui.label("Target Types:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.rel_def_form_target_types)
                            .hint_text("Functional, System (comma-separated, empty = all)"),
                    );
                    ui.end_row();

                    // Color (editable)
                    ui.label("Color:");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.rel_def_form_color)
                                .hint_text("#ff6b6b")
                                .desired_width(80.0),
                        );
                        // Show color preview
                        if let Some(c) = parse_hex_color(&self.rel_def_form_color) {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 2.0, c);
                        }
                    });
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("💾 Save").clicked() {
                    self.save_rel_def_form();
                }
                if ui.button("❌ Cancel").clicked() {
                    self.show_rel_def_form = false;
                    self.editing_rel_def = None;
                }
            });
        });
    }

    fn save_rel_def_form(&mut self) {
        // Parse source/target types
        let source_types: Vec<String> = self
            .rel_def_form_source_types
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let target_types: Vec<String> = self
            .rel_def_form_target_types
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let color = if self.rel_def_form_color.trim().is_empty() {
            None
        } else {
            Some(self.rel_def_form_color.trim().to_string())
        };

        let inverse = if self.rel_def_form_inverse.trim().is_empty() {
            None
        } else {
            Some(self.rel_def_form_inverse.trim().to_lowercase())
        };

        if let Some(ref edit_name) = self.editing_rel_def {
            // Update existing
            let is_built_in = self
                .store
                .get_relationship_definition(edit_name)
                .map(|d| d.built_in)
                .unwrap_or(false);

            let mut updated = if is_built_in {
                // For built-in, start from existing and only update allowed fields
                self.store
                    .get_relationship_definition(edit_name)
                    .cloned()
                    .unwrap_or_else(|| {
                        RelationshipDefinition::new(
                            &self.rel_def_form_name,
                            &self.rel_def_form_display_name,
                        )
                    })
            } else {
                RelationshipDefinition::new(
                    &self.rel_def_form_name,
                    &self.rel_def_form_display_name,
                )
            };

            updated.display_name = self.rel_def_form_display_name.clone();
            updated.description = self.rel_def_form_description.clone();
            updated.source_types = source_types;
            updated.target_types = target_types;
            updated.color = color;

            if !is_built_in {
                updated.inverse = inverse;
                updated.symmetric = self.rel_def_form_symmetric;
                updated.cardinality = self.rel_def_form_cardinality.clone();
            }

            match self
                .store
                .update_relationship_definition(edit_name, updated)
            {
                Ok(()) => {
                    self.save();
                    self.message = Some(("Relationship definition updated".to_string(), false));
                    self.show_rel_def_form = false;
                    self.editing_rel_def = None;
                }
                Err(e) => {
                    self.message = Some((format!("Failed to update: {}", e), true));
                }
            }
        } else {
            // Add new
            if self.rel_def_form_name.trim().is_empty() {
                self.message = Some(("Name is required".to_string(), true));
                return;
            }

            let mut new_def = RelationshipDefinition::new(
                &self.rel_def_form_name.trim().to_lowercase(),
                if self.rel_def_form_display_name.trim().is_empty() {
                    &self.rel_def_form_name
                } else {
                    &self.rel_def_form_display_name
                },
            );

            new_def.description = self.rel_def_form_description.clone();
            new_def.inverse = inverse;
            new_def.symmetric = self.rel_def_form_symmetric;
            new_def.cardinality = self.rel_def_form_cardinality.clone();
            new_def.source_types = source_types;
            new_def.target_types = target_types;
            new_def.color = color;

            match self.store.add_relationship_definition(new_def) {
                Ok(()) => {
                    self.save();
                    self.message = Some(("Relationship definition added".to_string(), false));
                    self.show_rel_def_form = false;
                }
                Err(e) => {
                    self.message = Some((format!("Failed to add: {}", e), true));
                }
            }
        }
    }

    fn show_settings_reactions_tab(&mut self, ui: &mut egui::Ui) {
        use aida_core::ReactionDefinition;

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Comment Reactions");
            ui.add_space(5.0);
            ui.label("Configure emoji reactions that can be added to comments.");
            ui.add_space(10.0);

            // Add new reaction button
            if ui.button("➕ Add Reaction").clicked() {
                self.show_reaction_def_form = true;
                self.editing_reaction_def = None;
                self.reaction_def_form_name.clear();
                self.reaction_def_form_emoji.clear();
                self.reaction_def_form_label.clear();
                self.reaction_def_form_description.clear();
            }

            ui.add_space(10.0);

            // Reaction form (inline)
            if self.show_reaction_def_form {
                ui.group(|ui| {
                    let title = if self.editing_reaction_def.is_some() {
                        "Edit Reaction"
                    } else {
                        "Add Reaction"
                    };
                    ui.heading(title);

                    egui::Grid::new("reaction_form_grid")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .show(ui, |ui| {
                            ui.label("Name (ID):");
                            let name_editable = self.editing_reaction_def.is_none();
                            ui.add_enabled(
                                name_editable,
                                egui::TextEdit::singleline(&mut self.reaction_def_form_name)
                                    .hint_text("e.g., thumbs_up"),
                            );
                            ui.end_row();

                            ui.label("Emoji:");
                            ui.text_edit_singleline(&mut self.reaction_def_form_emoji);
                            ui.end_row();

                            ui.label("Label:");
                            ui.text_edit_singleline(&mut self.reaction_def_form_label);
                            ui.end_row();

                            ui.label("Description:");
                            ui.text_edit_singleline(&mut self.reaction_def_form_description);
                            ui.end_row();
                        });

                    ui.add_space(5.0);

                    ui.horizontal(|ui| {
                        let can_save = !self.reaction_def_form_name.is_empty()
                            && !self.reaction_def_form_emoji.is_empty()
                            && !self.reaction_def_form_label.is_empty();

                        if ui
                            .add_enabled(can_save, egui::Button::new("💾 Save"))
                            .clicked()
                        {
                            if let Some(ref editing_name) = self.editing_reaction_def.clone() {
                                // Update existing
                                if let Some(def) = self
                                    .store
                                    .reaction_definitions
                                    .iter_mut()
                                    .find(|d| &d.name == editing_name)
                                {
                                    def.emoji = self.reaction_def_form_emoji.clone();
                                    def.label = self.reaction_def_form_label.clone();
                                    def.description =
                                        if self.reaction_def_form_description.is_empty() {
                                            None
                                        } else {
                                            Some(self.reaction_def_form_description.clone())
                                        };
                                }
                            } else {
                                // Add new
                                let mut new_def = ReactionDefinition::new(
                                    self.reaction_def_form_name.clone(),
                                    self.reaction_def_form_emoji.clone(),
                                    self.reaction_def_form_label.clone(),
                                );
                                if !self.reaction_def_form_description.is_empty() {
                                    new_def.description =
                                        Some(self.reaction_def_form_description.clone());
                                }
                                self.store.reaction_definitions.push(new_def);
                            }
                            self.save();
                            self.show_reaction_def_form = false;
                            self.editing_reaction_def = None;
                        }

                        if ui.button("Cancel").clicked() {
                            self.show_reaction_def_form = false;
                            self.editing_reaction_def = None;
                        }
                    });
                });

                ui.add_space(10.0);
            }

            // List existing reactions
            ui.heading("Defined Reactions");
            ui.add_space(5.0);

            // Clone to avoid borrow issues
            let reactions: Vec<_> = self.store.reaction_definitions.clone();

            if reactions.is_empty() {
                ui.label("No reactions defined.");
            } else {
                egui::Grid::new("reactions_table")
                    .num_columns(5)
                    .striped(true)
                    .spacing([10.0, 5.0])
                    .show(ui, |ui| {
                        // Header
                        ui.strong("Emoji");
                        ui.strong("Name");
                        ui.strong("Label");
                        ui.strong("Description");
                        ui.strong("Actions");
                        ui.end_row();

                        let mut to_delete: Option<String> = None;

                        for def in &reactions {
                            ui.label(&def.emoji);
                            ui.horizontal(|ui| {
                                ui.label(&def.name);
                                if def.built_in {
                                    ui.small("[built-in]");
                                }
                            });
                            ui.label(&def.label);
                            ui.label(def.description.as_deref().unwrap_or("-"));

                            ui.horizontal(|ui| {
                                if ui.small_button("✏").on_hover_text("Edit").clicked() {
                                    self.editing_reaction_def = Some(def.name.clone());
                                    self.reaction_def_form_name = def.name.clone();
                                    self.reaction_def_form_emoji = def.emoji.clone();
                                    self.reaction_def_form_label = def.label.clone();
                                    self.reaction_def_form_description =
                                        def.description.clone().unwrap_or_default();
                                    self.show_reaction_def_form = true;
                                }
                                // Only allow deletion of non-built-in reactions
                                if !def.built_in {
                                    if ui.small_button("🗑").on_hover_text("Delete").clicked() {
                                        to_delete = Some(def.name.clone());
                                    }
                                }
                            });
                            ui.end_row();
                        }

                        // Handle deletion outside the iteration
                        if let Some(name) = to_delete {
                            self.store.reaction_definitions.retain(|d| d.name != name);
                            self.save();
                        }
                    });
            }

            ui.add_space(15.0);

            // Reset to defaults button
            if ui
                .button("↺ Reset to Defaults")
                .on_hover_text("Restore built-in reactions")
                .clicked()
            {
                self.store.reaction_definitions = aida_core::default_reaction_definitions();
                self.save();
            }
        });
    }

    fn show_settings_type_definitions_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Requirement Type Definitions");
            ui.add_space(5.0);
            ui.label("Configure requirement types, their available statuses, and custom fields.");
            ui.add_space(10.0);

            // Show type definition form if editing/adding
            if self.show_type_def_form {
                self.show_type_definition_form(ui);
                return;
            }

            // Add new type button
            if ui.button("➕ Add New Type").clicked() {
                self.editing_type_def = None;
                self.type_def_form_name.clear();
                self.type_def_form_display_name.clear();
                self.type_def_form_description.clear();
                self.type_def_form_prefix.clear();
                self.type_def_form_statuses = vec![
                    "Draft".to_string(),
                    "Approved".to_string(),
                    "Completed".to_string(),
                    "Rejected".to_string(),
                ];
                self.type_def_form_priorities = vec![
                    "High".to_string(),
                    "Medium".to_string(),
                    "Low".to_string(),
                ];
                self.type_def_form_fields.clear();
                self.show_type_def_form = true;
            }

            ui.add_space(10.0);

            // Clone type definitions to avoid borrow issues
            let type_defs = self.store.type_definitions.clone();

            // Separate stateful and stateless types
            let stateful_types: Vec<_> = type_defs.iter().filter(|t| !t.stateless).collect();
            let stateless_types: Vec<_> = type_defs.iter().filter(|t| t.stateless).collect();

            if type_defs.is_empty() {
                ui.label("No type definitions configured.");
            } else {
                let mut type_to_delete: Option<String> = None;
                let mut type_to_edit: Option<String> = None;
                let mut type_to_reset: Option<String> = None;

                // Show stateful types
                for type_def in &stateful_types {
                    self.show_type_def_entry(ui, type_def, &mut type_to_edit, &mut type_to_reset, &mut type_to_delete);
                }

                // Show stateless types section if any exist
                if !stateless_types.is_empty() {
                    ui.add_space(15.0);
                    ui.separator();
                    ui.add_space(10.0);
                    ui.heading("📁 Organizational Types (Stateless)");
                    ui.add_space(5.0);
                    ui.label("Types without status/priority tracking, used for organizing requirements.");
                    ui.add_space(10.0);

                    for type_def in &stateless_types {
                        self.show_type_def_entry(ui, type_def, &mut type_to_edit, &mut type_to_reset, &mut type_to_delete);
                    }
                }

                // Handle edit action
                if let Some(name) = type_to_edit {
                    if let Some(type_def) =
                        self.store.type_definitions.iter().find(|t| t.name == name)
                    {
                        self.editing_type_def = Some(name);
                        self.type_def_form_name = type_def.name.clone();
                        self.type_def_form_display_name = type_def.display_name.clone();
                        self.type_def_form_description =
                            type_def.description.clone().unwrap_or_default();
                        self.type_def_form_prefix = type_def.prefix.clone().unwrap_or_default();
                        self.type_def_form_statuses = type_def.statuses.clone();
                        self.type_def_form_priorities = type_def.priorities.clone();
                        self.type_def_form_fields = type_def.custom_fields.clone();
                        self.show_type_def_form = true;
                    }
                }

                // Handle reset action
                if let Some(name) = type_to_reset {
                    let defaults = aida_core::default_type_definitions();
                    if let Some(default_def) = defaults.iter().find(|t| t.name == name) {
                        if let Some(idx) = self
                            .store
                            .type_definitions
                            .iter()
                            .position(|t| t.name == name)
                        {
                            self.store.type_definitions[idx] = default_def.clone();
                            self.save();
                        }
                    }
                }

                // Handle delete action
                if let Some(name) = type_to_delete {
                    // Check if any requirements use this type
                    let in_use = self
                        .store
                        .requirements
                        .iter()
                        .any(|r| format!("{:?}", r.req_type) == name);
                    if in_use {
                        self.message = Some((
                            format!(
                                "Cannot delete '{}': type is in use by existing requirements",
                                name
                            ),
                            true,
                        ));
                    } else {
                        self.store.type_definitions.retain(|t| t.name != name);
                        self.save();
                    }
                }
            }

            ui.add_space(15.0);

            // Reset all to defaults button
            if ui
                .button("↺ Reset All to Defaults")
                .on_hover_text("Restore all built-in type definitions")
                .clicked()
            {
                self.store.type_definitions = aida_core::default_type_definitions();
                self.save();
            }
        });
    }

    fn show_type_def_entry(
        &self,
        ui: &mut egui::Ui,
        type_def: &aida_core::CustomTypeDefinition,
        type_to_edit: &mut Option<String>,
        type_to_reset: &mut Option<String>,
        type_to_delete: &mut Option<String>,
    ) {
        let header = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            egui::Id::new(format!("type_def_{}", type_def.name)),
            false,
        );

        header
            .show_header(ui, |ui| {
                ui.horizontal(|ui| {
                    // Icon: folder for stateless, package for built-in, pencil for custom
                    let icon = if type_def.stateless {
                        "📁"
                    } else if type_def.built_in {
                        "📦"
                    } else {
                        "📝"
                    };
                    ui.label(format!("{} {}", icon, &type_def.display_name));

                    // Edit button
                    if ui.small_button("✏").on_hover_text("Edit type").clicked() {
                        *type_to_edit = Some(type_def.name.clone());
                    }

                    // Reset to default button (only for built-in types)
                    if type_def.built_in {
                        if ui
                            .small_button("↺")
                            .on_hover_text("Reset to default")
                            .clicked()
                        {
                            *type_to_reset = Some(type_def.name.clone());
                        }
                    } else {
                        // Delete button (only for custom types)
                        if ui.small_button("🗑").on_hover_text("Delete type").clicked() {
                            *type_to_delete = Some(type_def.name.clone());
                        }
                    }
                });
            })
            .body(|ui| {
                egui::Grid::new(format!("type_def_grid_{}", type_def.name))
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .show(ui, |ui| {
                        ui.label("Internal Name:");
                        ui.label(&type_def.name);
                        ui.end_row();

                        if let Some(ref prefix) = type_def.prefix {
                            ui.label("ID Prefix:");
                            ui.label(prefix);
                            ui.end_row();
                        }

                        if let Some(ref desc) = type_def.description {
                            ui.label("Description:");
                            ui.label(desc);
                            ui.end_row();
                        }

                        if type_def.stateless {
                            ui.label("");
                            ui.small("[Stateless - no status/priority]");
                            ui.end_row();
                        } else if type_def.built_in {
                            ui.label("");
                            ui.small("[Built-in type]");
                            ui.end_row();
                        }
                    });

                // Statuses section (only for stateful types)
                if !type_def.stateless && !type_def.statuses.is_empty() {
                    ui.add_space(5.0);
                    ui.label("Available Statuses:");
                    ui.horizontal_wrapped(|ui| {
                        for status in &type_def.statuses {
                            ui.label(format!("• {}", status));
                        }
                    });
                }

                // Custom fields section
                if !type_def.custom_fields.is_empty() {
                    ui.add_space(5.0);
                    ui.label("Custom Fields:");
                    egui::Grid::new(format!("custom_fields_grid_{}", type_def.name))
                        .num_columns(4)
                        .striped(true)
                        .spacing([10.0, 3.0])
                        .show(ui, |ui| {
                            ui.strong("Field");
                            ui.strong("Type");
                            ui.strong("Required");
                            ui.strong("Options/Default");
                            ui.end_row();

                            for field in &type_def.custom_fields {
                                ui.label(&field.label);
                                ui.label(Self::field_type_display(&field.field_type));
                                ui.label(if field.required { "Yes" } else { "No" });
                                // Show options or default
                                let extra_info = if !field.options.is_empty() {
                                    field.options.join(", ")
                                } else if let Some(ref def) = field.default_value {
                                    format!("Default: {}", def)
                                } else {
                                    "-".to_string()
                                };
                                ui.label(extra_info);
                                ui.end_row();
                            }
                        });
                }
            });
        ui.add_space(5.0);
    }

    fn field_type_display(field_type: &CustomFieldType) -> &'static str {
        match field_type {
            CustomFieldType::Text => "Text",
            CustomFieldType::TextArea => "Text Area",
            CustomFieldType::Select => "Select",
            CustomFieldType::Boolean => "Boolean",
            CustomFieldType::Date => "Date",
            CustomFieldType::User => "User Reference",
            CustomFieldType::Requirement => "Requirement Reference",
            CustomFieldType::Number => "Number",
        }
    }

    fn show_type_definition_form(&mut self, ui: &mut egui::Ui) {
        let is_editing = self.editing_type_def.is_some();
        let title = if is_editing {
            format!("Edit Type: {}", self.editing_type_def.as_ref().unwrap())
        } else {
            "Add New Type".to_string()
        };

        ui.heading(&title);
        ui.add_space(10.0);

        // Show custom field form if editing/adding a field
        if self.show_field_form {
            self.show_custom_field_form(ui);
            return;
        }

        egui::Grid::new("type_def_form_grid")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                // Name field (only editable for new types)
                ui.label("Internal Name:");
                if is_editing {
                    ui.label(&self.type_def_form_name);
                } else {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.type_def_form_name)
                            .hint_text("e.g., BugReport")
                            .desired_width(200.0),
                    );
                }
                ui.end_row();

                // Display name
                ui.label("Display Name:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.type_def_form_display_name)
                        .hint_text("e.g., Bug Report")
                        .desired_width(200.0),
                );
                ui.end_row();

                // Description
                ui.label("Description:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.type_def_form_description)
                        .hint_text("Optional description")
                        .desired_width(300.0),
                );
                ui.end_row();

                // ID Prefix
                ui.label("ID Prefix:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.type_def_form_prefix)
                        .hint_text("e.g., BUG")
                        .desired_width(100.0),
                );
                ui.end_row();
            });

        ui.add_space(15.0);

        // Statuses section
        ui.heading("Statuses");
        ui.add_space(5.0);

        // Display current statuses with remove buttons
        let mut status_to_remove: Option<usize> = None;
        ui.horizontal_wrapped(|ui| {
            for (idx, status) in self.type_def_form_statuses.iter().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(status);
                        if ui
                            .small_button("✕")
                            .on_hover_text("Remove status")
                            .clicked()
                        {
                            status_to_remove = Some(idx);
                        }
                    });
                });
            }
        });

        if let Some(idx) = status_to_remove {
            // Check if status is in use before removing
            let status_name = &self.type_def_form_statuses[idx];
            if let Some(ref editing_name) = self.editing_type_def {
                let in_use = self.store.requirements.iter().any(|r| {
                    format!("{:?}", r.req_type) == *editing_name
                        && r.custom_status.as_ref() == Some(status_name)
                });
                if in_use {
                    self.message = Some((
                        format!("Cannot remove '{}': status is in use", status_name),
                        true,
                    ));
                } else {
                    self.type_def_form_statuses.remove(idx);
                }
            } else {
                self.type_def_form_statuses.remove(idx);
            }
        }

        // Add new status
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_status_input)
                    .hint_text("New status name")
                    .desired_width(150.0),
            );
            if ui.button("Add Status").clicked() && !self.new_status_input.is_empty() {
                if !self.type_def_form_statuses.contains(&self.new_status_input) {
                    self.type_def_form_statuses
                        .push(self.new_status_input.clone());
                    self.new_status_input.clear();
                }
            }
        });

        ui.add_space(15.0);

        // Priorities section
        ui.heading("Priorities");
        ui.add_space(5.0);

        // Display current priorities with remove buttons
        let mut priority_to_remove: Option<usize> = None;
        ui.horizontal_wrapped(|ui| {
            for (idx, priority) in self.type_def_form_priorities.iter().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(priority);
                        if ui
                            .small_button("✕")
                            .on_hover_text("Remove priority")
                            .clicked()
                        {
                            priority_to_remove = Some(idx);
                        }
                    });
                });
            }
        });

        if let Some(idx) = priority_to_remove {
            // Check if priority is in use before removing
            let priority_name = &self.type_def_form_priorities[idx];
            if let Some(ref editing_name) = self.editing_type_def {
                let in_use = self.store.requirements.iter().any(|r| {
                    format!("{:?}", r.req_type) == *editing_name
                        && r.custom_priority.as_ref() == Some(priority_name)
                });
                if in_use {
                    self.message = Some((
                        format!("Cannot remove '{}': priority is in use", priority_name),
                        true,
                    ));
                } else {
                    self.type_def_form_priorities.remove(idx);
                }
            } else {
                self.type_def_form_priorities.remove(idx);
            }
        }

        // Add new priority
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_priority_input)
                    .hint_text("New priority name")
                    .desired_width(150.0),
            );
            if ui.button("Add Priority").clicked() && !self.new_priority_input.is_empty() {
                if !self.type_def_form_priorities.contains(&self.new_priority_input) {
                    self.type_def_form_priorities
                        .push(self.new_priority_input.clone());
                    self.new_priority_input.clear();
                }
            }
        });

        ui.add_space(15.0);

        // Custom fields section
        ui.heading("Custom Fields");
        ui.add_space(5.0);

        if self.type_def_form_fields.is_empty() {
            ui.label("No custom fields defined.");
        } else {
            let mut field_to_remove: Option<usize> = None;
            let mut field_to_edit: Option<usize> = None;

            egui::Grid::new("type_def_fields_grid")
                .num_columns(5)
                .striped(true)
                .spacing([10.0, 3.0])
                .show(ui, |ui| {
                    ui.strong("Field");
                    ui.strong("Type");
                    ui.strong("Required");
                    ui.strong("Options");
                    ui.strong("Actions");
                    ui.end_row();

                    for (idx, field) in self.type_def_form_fields.iter().enumerate() {
                        ui.label(&field.label);
                        ui.label(Self::field_type_display(&field.field_type));
                        ui.label(if field.required { "Yes" } else { "No" });
                        let extra = if !field.options.is_empty() {
                            field.options.join(", ")
                        } else if let Some(ref def) = field.default_value {
                            format!("Default: {}", def)
                        } else {
                            "-".to_string()
                        };
                        ui.label(extra);
                        ui.horizontal(|ui| {
                            if ui.small_button("✏").on_hover_text("Edit field").clicked() {
                                field_to_edit = Some(idx);
                            }
                            if ui.small_button("🗑").on_hover_text("Remove field").clicked() {
                                field_to_remove = Some(idx);
                            }
                        });
                        ui.end_row();
                    }
                });

            if let Some(idx) = field_to_edit {
                let field = &self.type_def_form_fields[idx];
                self.editing_field_idx = Some(idx);
                self.field_form_name = field.name.clone();
                self.field_form_label = field.label.clone();
                self.field_form_type = field.field_type.clone();
                self.field_form_required = field.required;
                self.field_form_options = field.options.join(", ");
                self.field_form_default = field.default_value.clone().unwrap_or_default();
                self.show_field_form = true;
            }

            if let Some(idx) = field_to_remove {
                // Check if field is in use before removing
                let field_name = &self.type_def_form_fields[idx].name;
                if let Some(ref editing_name) = self.editing_type_def {
                    let in_use = self.store.requirements.iter().any(|r| {
                        format!("{:?}", r.req_type) == *editing_name
                            && r.custom_fields.contains_key(field_name)
                    });
                    if in_use {
                        self.message = Some((
                            format!("Cannot remove '{}': field is in use", field_name),
                            true,
                        ));
                    } else {
                        self.type_def_form_fields.remove(idx);
                    }
                } else {
                    self.type_def_form_fields.remove(idx);
                }
            }
        }

        ui.add_space(5.0);
        if ui.button("➕ Add Field").clicked() {
            self.editing_field_idx = None;
            self.field_form_name.clear();
            self.field_form_label.clear();
            self.field_form_type = CustomFieldType::Text;
            self.field_form_required = false;
            self.field_form_options.clear();
            self.field_form_default.clear();
            self.show_field_form = true;
        }

        ui.add_space(20.0);
        ui.separator();
        ui.add_space(10.0);

        // Save/Cancel buttons
        ui.horizontal(|ui| {
            let can_save = !self.type_def_form_name.is_empty()
                && !self.type_def_form_display_name.is_empty()
                && !self.type_def_form_statuses.is_empty()
                && !self.type_def_form_priorities.is_empty();

            if ui
                .add_enabled(can_save, egui::Button::new("💾 Save"))
                .clicked()
            {
                self.save_type_definition();
            }

            if ui.button("Cancel").clicked() {
                self.show_type_def_form = false;
            }
        });

        if self.type_def_form_name.is_empty() || self.type_def_form_display_name.is_empty() {
            ui.small("Name and display name are required.");
        }
        if self.type_def_form_statuses.is_empty() {
            ui.small("At least one status is required.");
        }
        if self.type_def_form_priorities.is_empty() {
            ui.small("At least one priority is required.");
        }
    }

    fn show_custom_field_form(&mut self, ui: &mut egui::Ui) {
        let is_editing = self.editing_field_idx.is_some();
        let title = if is_editing {
            "Edit Custom Field"
        } else {
            "Add Custom Field"
        };

        ui.heading(title);
        ui.add_space(10.0);

        egui::Grid::new("custom_field_form_grid")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                // Field name (internal key)
                ui.label("Field Name:");
                if is_editing {
                    ui.label(&self.field_form_name);
                } else {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.field_form_name)
                            .hint_text("e.g., impact_level")
                            .desired_width(200.0),
                    );
                }
                ui.end_row();

                // Label (display name)
                ui.label("Label:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.field_form_label)
                        .hint_text("e.g., Impact Level")
                        .desired_width(200.0),
                );
                ui.end_row();

                // Field type
                ui.label("Type:");
                egui::ComboBox::from_id_salt("field_type_combo")
                    .selected_text(Self::field_type_display(&self.field_form_type))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.field_form_type,
                            CustomFieldType::Text,
                            "Text",
                        );
                        ui.selectable_value(
                            &mut self.field_form_type,
                            CustomFieldType::TextArea,
                            "Text Area",
                        );
                        ui.selectable_value(
                            &mut self.field_form_type,
                            CustomFieldType::Select,
                            "Select",
                        );
                        ui.selectable_value(
                            &mut self.field_form_type,
                            CustomFieldType::Boolean,
                            "Boolean",
                        );
                        ui.selectable_value(
                            &mut self.field_form_type,
                            CustomFieldType::Date,
                            "Date",
                        );
                        ui.selectable_value(
                            &mut self.field_form_type,
                            CustomFieldType::Number,
                            "Number",
                        );
                        ui.selectable_value(
                            &mut self.field_form_type,
                            CustomFieldType::User,
                            "User Reference",
                        );
                        ui.selectable_value(
                            &mut self.field_form_type,
                            CustomFieldType::Requirement,
                            "Requirement Reference",
                        );
                    });
                ui.end_row();

                // Required checkbox
                ui.label("Required:");
                ui.checkbox(&mut self.field_form_required, "");
                ui.end_row();

                // Options (for Select type)
                if self.field_form_type == CustomFieldType::Select {
                    ui.label("Options:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.field_form_options)
                            .hint_text("Option1, Option2, Option3")
                            .desired_width(300.0),
                    );
                    ui.end_row();
                }

                // Default value
                ui.label("Default Value:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.field_form_default)
                        .hint_text("Optional default")
                        .desired_width(200.0),
                );
                ui.end_row();
            });

        ui.add_space(15.0);

        // Save/Cancel buttons
        ui.horizontal(|ui| {
            let can_save = !self.field_form_name.is_empty() && !self.field_form_label.is_empty();

            if ui
                .add_enabled(can_save, egui::Button::new("💾 Save Field"))
                .clicked()
            {
                let options: Vec<String> = if self.field_form_type == CustomFieldType::Select {
                    self.field_form_options
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                } else {
                    Vec::new()
                };

                let field = CustomFieldDefinition {
                    name: self.field_form_name.clone(),
                    label: self.field_form_label.clone(),
                    field_type: self.field_form_type.clone(),
                    required: self.field_form_required,
                    options,
                    default_value: if self.field_form_default.is_empty() {
                        None
                    } else {
                        Some(self.field_form_default.clone())
                    },
                    description: None,
                    order: 0,
                };

                if let Some(idx) = self.editing_field_idx {
                    self.type_def_form_fields[idx] = field;
                } else {
                    self.type_def_form_fields.push(field);
                }

                self.show_field_form = false;
            }

            if ui.button("Cancel").clicked() {
                self.show_field_form = false;
            }
        });
    }

    fn save_type_definition(&mut self) {
        use aida_core::CustomTypeDefinition;

        let type_def = CustomTypeDefinition {
            name: self.type_def_form_name.clone(),
            display_name: self.type_def_form_display_name.clone(),
            description: if self.type_def_form_description.is_empty() {
                None
            } else {
                Some(self.type_def_form_description.clone())
            },
            prefix: if self.type_def_form_prefix.is_empty() {
                None
            } else {
                Some(self.type_def_form_prefix.to_uppercase())
            },
            statuses: self.type_def_form_statuses.clone(),
            priorities: self.type_def_form_priorities.clone(),
            custom_fields: self.type_def_form_fields.clone(),
            built_in: if let Some(ref editing_name) = self.editing_type_def {
                // Preserve built_in status when editing
                self.store
                    .type_definitions
                    .iter()
                    .find(|t| &t.name == editing_name)
                    .map(|t| t.built_in)
                    .unwrap_or(false)
            } else {
                false
            },
            color: None,
            stateless: if let Some(ref editing_name) = self.editing_type_def {
                // Preserve stateless status when editing
                self.store
                    .type_definitions
                    .iter()
                    .find(|t| &t.name == editing_name)
                    .map(|t| t.stateless)
                    .unwrap_or(false)
            } else {
                false
            },
        };

        if let Some(ref editing_name) = self.editing_type_def {
            // Update existing type
            if let Some(idx) = self
                .store
                .type_definitions
                .iter()
                .position(|t| &t.name == editing_name)
            {
                self.store.type_definitions[idx] = type_def;
            }
        } else {
            // Add new type
            self.store.type_definitions.push(type_def);
        }

        self.save();
        self.show_type_def_form = false;
    }

    fn show_settings_members_tab(&mut self, ui: &mut egui::Ui) {
        // Sub-tabs for Users and Teams
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.members_sub_tab, MembersSubTab::Users, "👤 Users");
            ui.selectable_value(&mut self.members_sub_tab, MembersSubTab::Teams, "👥 Teams");
        });
        ui.separator();
        ui.add_space(5.0);

        match self.members_sub_tab {
            MembersSubTab::Users => {
                self.show_settings_users_tab(ui);
            }
            MembersSubTab::Teams => {
                self.show_settings_teams_tab(ui);
            }
        }
    }

    fn show_settings_teams_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Team Management");
            ui.add_space(5.0);

            ui.horizontal(|ui| {
                if ui.button("➕ Add Team").clicked() {
                    self.show_team_form = true;
                    self.editing_team_id = None;
                    self.team_form_name.clear();
                    self.team_form_description.clear();
                    self.team_form_parent_id = None;
                }
                ui.checkbox(&mut self.show_archived_teams, "Show Archived");
            });

            ui.add_space(5.0);

            // Team form (inline)
            if self.show_team_form {
                ui.group(|ui| {
                    let title = if self.editing_team_id.is_some() {
                        "Edit Team"
                    } else {
                        "Add Team"
                    };
                    ui.label(title);

                    egui::Grid::new("team_form_grid")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .show(ui, |ui| {
                            ui.label("Name:");
                            ui.text_edit_singleline(&mut self.team_form_name);
                            ui.end_row();

                            ui.label("Description:");
                            ui.add(
                                egui::TextEdit::multiline(&mut self.team_form_description)
                                    .desired_rows(2)
                                    .desired_width(250.0),
                            );
                            ui.end_row();

                            ui.label("Parent Team:");
                            // Dropdown for parent team selection
                            let current_parent_name = self
                                .team_form_parent_id
                                .and_then(|id| self.store.find_team_by_id(&id))
                                .map(|t| t.name.clone())
                                .unwrap_or_else(|| "(None)".to_string());

                            egui::ComboBox::from_id_salt("parent_team_select")
                                .selected_text(&current_parent_name)
                                .show_ui(ui, |ui| {
                                    if ui.selectable_label(self.team_form_parent_id.is_none(), "(None)").clicked() {
                                        self.team_form_parent_id = None;
                                    }
                                    // Clone teams to avoid borrow issues
                                    let teams: Vec<_> = self.store.teams.iter()
                                        .filter(|t| !t.archived)
                                        .filter(|t| {
                                            // Don't allow selecting self as parent when editing
                                            self.editing_team_id.map(|eid| t.id != eid).unwrap_or(true)
                                        })
                                        .map(|t| (t.id, t.name.clone()))
                                        .collect();
                                    for (team_id, team_name) in teams {
                                        let is_selected = self.team_form_parent_id == Some(team_id);
                                        if ui.selectable_label(is_selected, &team_name).clicked() {
                                            self.team_form_parent_id = Some(team_id);
                                        }
                                    }
                                });
                            ui.end_row();
                        });

                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            if self.editing_team_id.is_some() {
                                self.save_edited_team();
                            } else {
                                self.add_new_team();
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_team_form = false;
                            self.editing_team_id = None;
                        }
                    });
                });
                ui.add_space(5.0);
            }

            // Teams table
            self.show_teams_table(ui);
        });
    }

    fn add_new_team(&mut self) {
        if self.team_form_name.trim().is_empty() {
            self.message = Some(("Team name cannot be empty".to_string(), true));
            return;
        }

        // Check for circular reference if parent is set
        if let Some(parent_id) = self.team_form_parent_id {
            if let Some(editing_id) = self.editing_team_id {
                if self.store.would_create_team_cycle(&editing_id, &parent_id) {
                    self.message = Some(("Cannot set parent: would create circular reference".to_string(), true));
                    return;
                }
            }
        }

        let _spec_id = self.store.add_team_with_id(
            self.team_form_name.clone(),
            self.team_form_description.clone(),
            self.team_form_parent_id,
        );

        self.show_team_form = false;
        self.team_form_name.clear();
        self.team_form_description.clear();
        self.team_form_parent_id = None;

        self.save();
        self.message = Some(("Team added successfully".to_string(), false));
    }

    fn save_edited_team(&mut self) {
        if self.team_form_name.trim().is_empty() {
            self.message = Some(("Team name cannot be empty".to_string(), true));
            return;
        }

        let team_id = match self.editing_team_id {
            Some(id) => id,
            None => return,
        };

        // Check for circular reference if parent is set
        if let Some(parent_id) = self.team_form_parent_id {
            if self.store.would_create_team_cycle(&team_id, &parent_id) {
                self.message = Some(("Cannot set parent: would create circular reference".to_string(), true));
                return;
            }
        }

        if let Some(team) = self.store.get_team_by_id_mut(&team_id) {
            team.name = self.team_form_name.clone();
            team.description = self.team_form_description.clone();
            team.parent_team_id = self.team_form_parent_id;
            team.modified_at = Some(chrono::Utc::now());
        }

        self.show_team_form = false;
        self.editing_team_id = None;

        self.save();
        self.message = Some(("Team updated successfully".to_string(), false));
    }

    fn show_teams_table(&mut self, ui: &mut egui::Ui) {
        let teams: Vec<_> = self.store.teams.iter()
            .filter(|t| self.show_archived_teams || !t.archived)
            .map(|t| (t.id, t.spec_id.clone(), t.name.clone(), t.description.clone(),
                      t.parent_team_id, t.member_ids.len(), t.archived))
            .collect();

        if teams.is_empty() {
            ui.label("No teams defined yet.");
            return;
        }

        egui::Grid::new("teams_table")
            .num_columns(6)
            .striped(true)
            .spacing([10.0, 5.0])
            .show(ui, |ui| {
                // Header
                ui.strong("ID");
                ui.strong("Name");
                ui.strong("Parent");
                ui.strong("Members");
                ui.strong("Status");
                ui.strong("Actions");
                ui.end_row();

                for (team_id, spec_id, name, _desc, parent_id, member_count, archived) in &teams {
                    // ID
                    ui.label(spec_id.as_deref().unwrap_or("-"));

                    // Name
                    ui.label(name);

                    // Parent
                    let parent_name = parent_id
                        .and_then(|pid| self.store.find_team_by_id(&pid))
                        .map(|t| t.name.as_str())
                        .unwrap_or("-");
                    ui.label(parent_name);

                    // Members
                    ui.label(format!("{}", member_count));

                    // Status
                    if *archived {
                        ui.label("📦 Archived");
                    } else {
                        ui.label("Active");
                    }

                    // Actions
                    ui.horizontal(|ui| {
                        if ui.small_button("✏").on_hover_text("Edit").clicked() {
                            self.editing_team_id = Some(*team_id);
                            self.show_team_form = true;
                            if let Some(team) = self.store.find_team_by_id(team_id) {
                                self.team_form_name = team.name.clone();
                                self.team_form_description = team.description.clone();
                                self.team_form_parent_id = team.parent_team_id;
                            }
                        }

                        if ui.small_button("👥").on_hover_text("Manage Members").clicked() {
                            self.show_team_members_dialog = Some(*team_id);
                        }

                        if *archived {
                            if ui.small_button("📤").on_hover_text("Unarchive").clicked() {
                                if let Some(team) = self.store.get_team_by_id_mut(team_id) {
                                    team.archived = false;
                                    team.modified_at = Some(chrono::Utc::now());
                                }
                                self.save();
                            }
                        } else {
                            if ui.small_button("📦").on_hover_text("Archive").clicked() {
                                if let Some(team) = self.store.get_team_by_id_mut(team_id) {
                                    team.archived = true;
                                    team.modified_at = Some(chrono::Utc::now());
                                }
                                self.save();
                            }
                        }
                    });

                    ui.end_row();
                }
            });

        // Team members dialog
        self.show_team_members_dialog_ui(ui);
    }

    fn show_team_members_dialog_ui(&mut self, ui: &mut egui::Ui) {
        let team_id = match self.show_team_members_dialog {
            Some(id) => id,
            None => return,
        };

        let team_name = self.store.find_team_by_id(&team_id)
            .map(|t| t.name.clone())
            .unwrap_or_default();

        let mut open = true;
        egui::Window::new(format!("Members of '{}'", team_name))
            .collapsible(false)
            .resizable(true)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                // Current members
                ui.heading("Current Members");

                let member_ids: Vec<Uuid> = self.store.find_team_by_id(&team_id)
                    .map(|t| t.member_ids.clone())
                    .unwrap_or_default();

                if member_ids.is_empty() {
                    ui.label("No members in this team.");
                } else {
                    for member_id in &member_ids {
                        ui.horizontal(|ui| {
                            let user_name = self.store.find_user_by_id(member_id)
                                .map(|u| format!("{} (@{})", u.name, u.handle))
                                .unwrap_or_else(|| format!("Unknown user: {}", member_id));
                            ui.label(&user_name);
                            if ui.small_button("❌").on_hover_text("Remove from team").clicked() {
                                if let Some(team) = self.store.get_team_by_id_mut(&team_id) {
                                    team.remove_member(member_id);
                                }
                                self.save();
                            }
                        });
                    }
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(5.0);

                // Add member
                ui.heading("Add Member");

                // Show users not already in the team
                let available_users: Vec<_> = self.store.users.iter()
                    .filter(|u| !u.archived)
                    .filter(|u| !member_ids.contains(&u.id))
                    .map(|u| (u.id, u.name.clone(), u.handle.clone()))
                    .collect();

                if available_users.is_empty() {
                    ui.label("All users are already members of this team.");
                } else {
                    for (user_id, name, handle) in available_users {
                        ui.horizontal(|ui| {
                            ui.label(format!("{} (@{})", name, handle));
                            if ui.small_button("➕").on_hover_text("Add to team").clicked() {
                                if let Some(team) = self.store.get_team_by_id_mut(&team_id) {
                                    team.add_member(user_id);
                                }
                                self.save();
                            }
                        });
                    }
                }
            });

        if !open {
            self.show_team_members_dialog = None;
        }
    }

    /// Render the organization chart view showing team hierarchy and members
    fn show_org_chart_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("🏢 Organization Chart");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("📋 Back to Requirements").clicked() {
                    self.pending_view_change = Some(View::List);
                }
                if ui.button("⚙ Manage Teams").clicked() {
                    self.show_settings_dialog = true;
                    self.settings_tab = SettingsTab::Members;
                    self.members_sub_tab = MembersSubTab::Teams;
                }
            });
        });
        ui.separator();
        ui.add_space(10.0);

        if self.store.teams.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(50.0);
                ui.label("No teams defined yet.");
                ui.add_space(10.0);
                if ui.button("➕ Create First Team").clicked() {
                    self.show_settings_dialog = true;
                    self.settings_tab = SettingsTab::Members;
                    self.members_sub_tab = MembersSubTab::Teams;
                    self.show_team_form = true;
                }
            });
            return;
        }

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Get root teams (teams without a parent)
                let root_teams: Vec<_> = self.store.teams.iter()
                    .filter(|t| !t.archived && t.parent_team_id.is_none())
                    .map(|t| t.id)
                    .collect();

                if root_teams.is_empty() {
                    // All teams have parents but there are teams - show all as orphans
                    ui.label("All teams have parent references. Showing all teams:");
                    ui.add_space(10.0);
                    let all_teams: Vec<_> = self.store.teams.iter()
                        .filter(|t| !t.archived)
                        .map(|t| t.id)
                        .collect();
                    for team_id in all_teams {
                        self.render_org_chart_team_node(ui, &team_id, 0);
                    }
                } else {
                    for team_id in root_teams {
                        self.render_org_chart_team_node(ui, &team_id, 0);
                    }
                }
            });
    }

    /// Recursively render a team node in the org chart
    fn render_org_chart_team_node(&mut self, ui: &mut egui::Ui, team_id: &Uuid, depth: usize) {
        let indent = depth as f32 * 30.0;

        // Get team data (clone to avoid borrowing issues)
        let team_data = self.store.find_team_by_id(team_id).map(|t| {
            (
                t.name.clone(),
                t.description.clone(),
                t.spec_id.clone(),
                t.member_ids.clone(),
            )
        });

        let Some((name, description, spec_id, member_ids)) = team_data else {
            return;
        };

        ui.horizontal(|ui| {
            ui.add_space(indent);

            // Team card
            ui.group(|ui| {
                ui.set_min_width(250.0);
                ui.vertical(|ui| {
                    // Team header
                    ui.horizontal(|ui| {
                        ui.label("👥");
                        ui.strong(&name);
                        if let Some(id) = &spec_id {
                            ui.weak(format!("({})", id));
                        }
                    });

                    // Description if present
                    if !description.is_empty() {
                        ui.label(&description);
                    }

                    // Member count and expandable member list
                    let member_count = member_ids.len();
                    ui.horizontal(|ui| {
                        ui.label(format!("👤 {} member{}", member_count, if member_count == 1 { "" } else { "s" }));
                    });

                    // Show members
                    if !member_ids.is_empty() {
                        ui.add_space(5.0);
                        ui.separator();
                        ui.label("Members:");
                        for member_id in &member_ids {
                            let member_name = self.store.find_user_by_id(member_id)
                                .map(|u| format!("• {} (@{})", u.name, u.handle))
                                .unwrap_or_else(|| format!("• Unknown user"));
                            ui.small(&member_name);
                        }
                    }
                });
            });
        });

        ui.add_space(5.0);

        // Render child teams
        let child_team_ids: Vec<_> = self.store.teams.iter()
            .filter(|t| !t.archived && t.parent_team_id == Some(*team_id))
            .map(|t| t.id)
            .collect();

        for child_id in child_team_ids {
            self.render_org_chart_team_node(ui, &child_id, depth + 1);
        }
    }

    /// Show the KanBan board view
    fn show_kanban_view(&mut self, ui: &mut egui::Ui) {
        // Header with controls
        ui.horizontal(|ui| {
            ui.heading("📊 KanBan Board");

            ui.separator();

            // Field selector (what determines columns)
            ui.label("Group by:");
            egui::ComboBox::from_id_salt("kanban_field_selector")
                .selected_text(self.kanban_field.label())
                .show_ui(ui, |ui| {
                    if ui.selectable_label(self.kanban_field == KanBanField::Status, "Status").clicked() {
                        self.kanban_field = KanBanField::Status;
                    }
                    if ui.selectable_label(self.kanban_field == KanBanField::Priority, "Priority").clicked() {
                        self.kanban_field = KanBanField::Priority;
                    }
                });

            ui.separator();

            // Type filter
            ui.label("Type:");
            let type_label = self.kanban_filter_type
                .as_ref()
                .map(|t| format!("{:?}", t))
                .unwrap_or_else(|| "All".to_string());
            egui::ComboBox::from_id_salt("kanban_type_filter")
                .selected_text(&type_label)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(self.kanban_filter_type.is_none(), "All").clicked() {
                        self.kanban_filter_type = None;
                    }
                    for req_type in [
                        RequirementType::Functional,
                        RequirementType::NonFunctional,
                        RequirementType::System,
                        RequirementType::User,
                        RequirementType::ChangeRequest,
                        RequirementType::Bug,
                        RequirementType::Epic,
                        RequirementType::Story,
                        RequirementType::Task,
                        RequirementType::Spike,
                    ] {
                        if ui.selectable_label(
                            self.kanban_filter_type.as_ref() == Some(&req_type),
                            format!("{:?}", req_type)
                        ).clicked() {
                            self.kanban_filter_type = Some(req_type);
                        }
                    }
                });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("📋 Back to List").clicked() {
                    self.pending_view_change = Some(View::List);
                }
            });
        });

        ui.separator();

        // Get column definitions based on selected field
        let columns = self.get_kanban_columns();

        if columns.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("No columns configured for this field");
            });
            return;
        }

        // Calculate column width
        let available_width = ui.available_width();
        let column_count = columns.len();
        let column_width = (available_width / column_count as f32).max(150.0) - 10.0;

        // Collect column rects for drop detection
        let mut column_rects: Vec<(String, egui::Rect)> = Vec::new();

        // Render columns
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    for (col_idx, column) in columns.iter().enumerate() {
                        let rect = self.render_kanban_column(ui, column, column_width, col_idx);
                        column_rects.push((column.value.clone(), rect));
                    }
                });
            });

        // Handle drag-and-drop using pointer position (more reliable than hovered())
        if self.kanban_drag_card.is_some() {
            if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                // Find which column the pointer is over
                self.kanban_drop_column = None;
                for (col_value, rect) in &column_rects {
                    if rect.contains(pointer_pos) {
                        self.kanban_drop_column = Some(col_value.clone());
                        break;
                    }
                }
            }

            // Show drag indicator following cursor
            if let Some(drag_id) = self.kanban_drag_card {
                if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                    // Find the spec_id of the dragged card
                    let spec_id = self.store.requirements.iter()
                        .find(|r| r.id == drag_id)
                        .and_then(|r| r.spec_id.clone())
                        .unwrap_or_else(|| "Card".to_string());

                    egui::Area::new(egui::Id::new("kanban_drag_indicator"))
                        .fixed_pos(pos + egui::vec2(10.0, 10.0))
                        .order(egui::Order::Tooltip)
                        .show(ui.ctx(), |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                ui.label(format!("📎 {}", spec_id));
                            });
                        });
                }
            }

            // Handle drag release
            let released = ui.input(|i| i.pointer.primary_released());
            if released {
                if let Some(drag_id) = self.kanban_drag_card.take() {
                    if let Some(target_column) = self.kanban_drop_column.take() {
                        self.update_kanban_field(&drag_id, &target_column);
                    }
                }
            }
        }

        // Clear drop target when not dragging (cleanup stale state)
        if self.kanban_drag_card.is_none() {
            self.kanban_drop_column = None;
        }
    }

    /// Get column definitions for the current KanBan field
    fn get_kanban_columns(&self) -> Vec<KanBanColumn> {
        match self.kanban_field {
            KanBanField::Status => {
                // Use status values from the store's definitions or defaults
                vec![
                    KanBanColumn { value: "Draft".to_string(), label: "📝 Draft".to_string(), wip_limit: 0 },
                    KanBanColumn { value: "Approved".to_string(), label: "✅ Approved".to_string(), wip_limit: 0 },
                    KanBanColumn { value: "Completed".to_string(), label: "🏁 Completed".to_string(), wip_limit: 0 },
                    KanBanColumn { value: "Rejected".to_string(), label: "❌ Rejected".to_string(), wip_limit: 0 },
                ]
            }
            KanBanField::Priority => {
                vec![
                    KanBanColumn { value: "High".to_string(), label: "🔴 High".to_string(), wip_limit: 0 },
                    KanBanColumn { value: "Medium".to_string(), label: "🟡 Medium".to_string(), wip_limit: 0 },
                    KanBanColumn { value: "Low".to_string(), label: "🟢 Low".to_string(), wip_limit: 0 },
                ]
            }
        }
    }

    /// Get the value of a requirement for the current KanBan field
    fn get_kanban_field_value(&self, req: &Requirement) -> String {
        match self.kanban_field {
            KanBanField::Status => req.effective_status(),
            KanBanField::Priority => req.effective_priority(),
        }
    }

    /// Render a single KanBan column, returns the column rect for drop detection
    fn render_kanban_column(&mut self, ui: &mut egui::Ui, column: &KanBanColumn, width: f32, _col_idx: usize) -> egui::Rect {
        // Get requirements for this column
        let reqs_in_column: Vec<(usize, Uuid, String, String, String)> = self.store.requirements
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                // Filter by archived
                if r.archived && !self.show_archived {
                    return false;
                }
                // Exclude Folder types - they're stateless organizational containers
                if r.req_type == RequirementType::Folder {
                    return false;
                }
                // Filter by type if set
                if let Some(ref filter_type) = self.kanban_filter_type {
                    if &r.req_type != filter_type {
                        return false;
                    }
                }
                // Filter by column value
                let value = self.get_kanban_field_value(r);
                value == column.value
            })
            .map(|(idx, r)| (
                idx,
                r.id,
                r.spec_id.clone().unwrap_or_default(),
                r.title.clone(),
                r.owner.clone(),
            ))
            .collect();

        let card_count = reqs_in_column.len();
        let wip_limit = self.kanban_wip_limits.get(&column.value).copied().unwrap_or(column.wip_limit);
        let over_wip = wip_limit > 0 && card_count > wip_limit;
        let is_dragging = self.kanban_drag_card.is_some();
        let is_drop_target = self.kanban_drop_column.as_ref() == Some(&column.value);

        // Column frame - highlight when it's a valid drop target
        let column_color = if over_wip {
            egui::Color32::from_rgb(255, 200, 200) // Light red for over WIP
        } else if is_drop_target && is_dragging {
            egui::Color32::from_rgb(180, 220, 255) // Brighter blue for active drop target
        } else {
            ui.visuals().window_fill()
        };

        let column_stroke = if is_drop_target && is_dragging {
            egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 150, 255))
        } else {
            ui.visuals().window_stroke()
        };

        let column_value = column.value.clone();

        let frame_response = egui::Frame::none()
            .fill(column_color)
            .stroke(column_stroke)
            .rounding(5.0)
            .inner_margin(5.0)
            .show(ui, |ui| {
                ui.set_min_width(width);
                ui.set_max_width(width);

                // Column header
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(&column.label);
                        ui.weak(format!("({})", card_count));
                        if wip_limit > 0 {
                            let wip_text = if over_wip {
                                format!("⚠️ WIP: {}", wip_limit)
                            } else {
                                format!("WIP: {}", wip_limit)
                            };
                            ui.weak(&wip_text);
                        }
                    });

                    ui.separator();

                    // Cards scroll area
                    egui::ScrollArea::vertical()
                        .id_salt(format!("kanban_col_{}", column_value))
                        .max_height(ui.available_height() - 30.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (idx, req_id, spec_id, title, owner) in reqs_in_column {
                                self.render_kanban_card(ui, idx, &req_id, &spec_id, &title, &owner, &column_value);
                            }

                            // Drop zone hint when empty
                            if card_count == 0 {
                                let drop_zone_height = 100.0;
                                let (rect, _response) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), drop_zone_height),
                                    egui::Sense::hover()
                                );

                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "Drop cards here",
                                    egui::FontId::default(),
                                    ui.visuals().weak_text_color(),
                                );
                            }
                        });
                });
            });

        ui.add_space(5.0);

        // Return the column rect for drop detection
        frame_response.response.rect
    }

    /// Render a single KanBan card
    fn render_kanban_card(
        &mut self,
        ui: &mut egui::Ui,
        idx: usize,
        req_id: &Uuid,
        spec_id: &str,
        title: &str,
        owner: &str,
        _column_value: &str,
    ) {
        let is_this_card_dragging = self.kanban_drag_card == Some(*req_id);
        let is_selected = self.selected_idx == Some(idx);

        // Card appearance based on state
        let card_color = if is_this_card_dragging {
            egui::Color32::from_rgb(180, 180, 255) // Purple tint for dragging card
        } else if is_selected {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().widgets.inactive.bg_fill
        };

        let card_stroke = if is_this_card_dragging {
            egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 100, 200))
        } else {
            egui::Stroke::new(1.0, ui.visuals().widgets.inactive.bg_stroke.color)
        };

        let response = egui::Frame::none()
            .fill(card_color)
            .stroke(card_stroke)
            .rounding(3.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width() - 10.0);

                ui.vertical(|ui| {
                    // Spec ID and owner
                    ui.horizontal(|ui| {
                        ui.strong(spec_id);
                        if !owner.is_empty() {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.weak(owner);
                            });
                        }
                    });

                    // Title (truncated if too long)
                    let display_title = if title.len() > 60 {
                        format!("{}...", &title[..57])
                    } else {
                        title.to_string()
                    };
                    ui.label(&display_title);
                });
            }).response;

        // Handle interactions
        let response = response.interact(egui::Sense::click_and_drag());

        // Open detail modal based on configured click action
        let open_modal = match self.user_settings.kanban_click_action {
            KanBanClickAction::RightClick => response.secondary_clicked(),
            KanBanClickAction::SingleClick => response.clicked() && self.kanban_drag_card.is_none(),
        };

        if open_modal {
            self.selected_idx = Some(idx);
            self.kanban_detail_modal = Some(idx);
        }

        // Single click selects the card (when using right-click for modal)
        if response.clicked() && self.kanban_drag_card.is_none() {
            self.selected_idx = Some(idx);
        }

        // Double click - open edit view
        if response.double_clicked() {
            self.selected_idx = Some(idx);
            self.kanban_detail_modal = None; // Close any open modal
            self.load_form_from_requirement(idx);
            self.pending_view_change = Some(View::Edit);
        }

        // Drag start - begin dragging this card
        // Release is handled globally in show_kanban_view
        if response.drag_started() {
            self.kanban_drag_card = Some(*req_id);
            self.kanban_drop_column = None; // Clear any previous drop target
        }

        ui.add_space(5.0);
    }

    /// Update a requirement's field based on KanBan column drop
    fn update_kanban_field(&mut self, req_id: &Uuid, new_value: &str) {
        if let Some(req) = self.store.requirements.iter_mut().find(|r| r.id == *req_id) {
            match self.kanban_field {
                KanBanField::Status => {
                    req.set_status_from_str(new_value);
                    req.modified_at = chrono::Utc::now();
                }
                KanBanField::Priority => {
                    req.set_priority_from_str(new_value);
                    req.modified_at = chrono::Utc::now();
                }
            }
            // Save changes
            if let Err(e) = self.storage.save(&self.store) {
                eprintln!("Failed to save after KanBan update: {}", e);
            }
        }
    }

    /// Show the KanBan detail modal dialog
    fn show_kanban_detail_modal(&mut self, ctx: &egui::Context) {
        let Some(idx) = self.kanban_detail_modal else {
            return;
        };

        let Some(req) = self.store.requirements.get(idx).cloned() else {
            self.kanban_detail_modal = None;
            return;
        };

        let mut close_modal = false;
        let mut open_edit = false;

        // Calculate modal size based on screen size
        let screen_rect = ctx.screen_rect();
        let modal_width = (screen_rect.width() * 0.7).min(900.0).max(400.0);
        let modal_height = (screen_rect.height() * 0.8).min(700.0).max(300.0);

        let title = format!("{} - {}",
            req.spec_id.as_deref().unwrap_or("N/A"),
            if req.title.len() > 50 { format!("{}...", &req.title[..47]) } else { req.title.clone() }
        );

        egui::Window::new(title)
            .id(egui::Id::new("kanban_detail_modal"))
            .collapsible(false)
            .resizable(true)
            .default_width(modal_width)
            .default_height(modal_height)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                // Header with buttons
                ui.horizontal(|ui| {
                    if ui.button("✏ Edit").clicked() {
                        open_edit = true;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("✕ Close").clicked() {
                            close_modal = true;
                        }
                    });
                });

                ui.separator();

                // Scrollable content area
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Status and Priority row
                        ui.horizontal(|ui| {
                            ui.label("Status:");
                            ui.strong(req.effective_status());
                            ui.separator();
                            ui.label("Priority:");
                            ui.strong(format!("{}", req.priority));
                            ui.separator();
                            ui.label("Type:");
                            ui.strong(format!("{:?}", req.req_type));
                        });

                        ui.add_space(10.0);

                        // Owner and Feature
                        ui.horizontal(|ui| {
                            if !req.owner.is_empty() {
                                ui.label("Owner:");
                                ui.strong(&req.owner);
                                ui.separator();
                            }
                            if !req.feature.is_empty() {
                                ui.label("Feature:");
                                ui.strong(&req.feature);
                            }
                        });

                        ui.add_space(10.0);

                        // Description
                        ui.heading("Description");
                        ui.separator();

                        if req.description.is_empty() {
                            ui.weak("No description provided.");
                        } else {
                            // Use a label with text wrapping for the description
                            ui.add(egui::Label::new(&req.description).wrap());
                        }

                        ui.add_space(15.0);

                        // Tags
                        if !req.tags.is_empty() {
                            ui.heading("Tags");
                            ui.horizontal_wrapped(|ui| {
                                for tag in &req.tags {
                                    ui.label(format!("#{}", tag));
                                }
                            });
                            ui.add_space(10.0);
                        }

                        // URLs
                        if !req.urls.is_empty() {
                            ui.heading("Links");
                            for url in &req.urls {
                                ui.horizontal(|ui| {
                                    if !url.title.is_empty() {
                                        ui.label(format!("{}: {}", url.title, url.url));
                                    } else {
                                        ui.label(&url.url);
                                    }
                                });
                            }
                            ui.add_space(10.0);
                        }

                        // Relationships
                        if !req.relationships.is_empty() {
                            ui.heading("Relationships");
                            for rel in &req.relationships {
                                // Find the related requirement
                                if let Some(related) = self.store.requirements.iter().find(|r| r.id == rel.target_id) {
                                    ui.horizontal(|ui| {
                                        ui.label(format!("{:?} →", rel.rel_type));
                                        ui.strong(related.spec_id.as_deref().unwrap_or("N/A"));
                                        ui.weak(&related.title);
                                    });
                                }
                            }
                            ui.add_space(10.0);
                        }

                        // Timestamps
                        ui.add_space(10.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.weak(format!("Created: {}", req.created_at.format("%Y-%m-%d %H:%M")));
                            ui.separator();
                            ui.weak(format!("Modified: {}", req.modified_at.format("%Y-%m-%d %H:%M")));
                        });
                    });
            });

        // Handle actions after the window is drawn
        if close_modal {
            self.kanban_detail_modal = None;
        }

        if open_edit {
            self.kanban_detail_modal = None;
            self.selected_idx = Some(idx);
            self.load_form_from_requirement(idx);
            self.pending_view_change = Some(View::Edit);
        }

        // Close modal on Escape key
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.kanban_detail_modal = None;
        }
    }

    /// Show the baselines management view
    fn show_baselines_view(&mut self, ui: &mut egui::Ui) {
        // Header with controls
        ui.horizontal(|ui| {
            ui.heading("🏷 Baselines");

            ui.separator();

            if ui.button("➕ Create Baseline").clicked() {
                self.show_create_baseline_dialog = true;
                self.baseline_form_name.clear();
                self.baseline_form_description.clear();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("📋 Back to List").clicked() {
                    self.pending_view_change = Some(View::List);
                }
            });
        });

        ui.separator();

        // List of baselines
        let baselines = self.store.baselines.clone();

        if baselines.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(50.0);
                    ui.label(egui::RichText::new("No baselines created yet").size(16.0).color(egui::Color32::GRAY));
                    ui.add_space(10.0);
                    ui.label("Baselines are named snapshots of your requirements at a point in time.");
                    ui.label("Use them to track releases, milestones, or approved states.");
                    ui.add_space(20.0);
                    if ui.button("➕ Create First Baseline").clicked() {
                        self.show_create_baseline_dialog = true;
                        self.baseline_form_name.clear();
                        self.baseline_form_description.clear();
                    }
                });
            });
            return;
        }

        // Use full available width
        let available_width = ui.available_width();
        ui.set_min_width(available_width);

        // Baselines table
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(available_width - 20.0);

                egui::Grid::new("baselines_grid")
                    .num_columns(6)
                    .spacing([20.0, 8.0])
                    .striped(true)
                    .min_col_width(80.0)
                    .show(ui, |ui| {
                        // Header
                        ui.strong("Name");
                        ui.strong("Created");
                        ui.strong("By");
                        ui.strong("Requirements");
                        ui.strong("Git Tag");
                        ui.strong("Actions");
                        ui.end_row();

                        let mut delete_baseline_id: Option<Uuid> = None;

                        for baseline in &baselines {
                            // Name
                            let selected = self.selected_baseline_id == Some(baseline.id);
                            if ui.selectable_label(selected, &baseline.name).clicked() {
                                self.selected_baseline_id = Some(baseline.id);
                            }

                            // Created date
                            ui.label(baseline.created_at.format("%Y-%m-%d %H:%M").to_string());

                            // Created by
                            ui.label(&baseline.created_by);

                            // Requirement count
                            ui.label(format!("{}", baseline.requirements.len()));

                            // Git tag
                            if let Some(ref tag) = baseline.git_tag {
                                ui.label(egui::RichText::new(tag).monospace().color(egui::Color32::from_rgb(100, 150, 200)));
                            } else {
                                ui.label(egui::RichText::new("-").color(egui::Color32::GRAY));
                            }

                            // Actions
                            ui.horizontal(|ui| {
                                if ui.small_button("📊 Compare").on_hover_text("Compare with current state").clicked() {
                                    self.baseline_compare_source = Some(baseline.id);
                                    self.baseline_compare_target = None; // Compare with current
                                    self.show_baseline_comparison = true;
                                }

                                if !baseline.locked {
                                    if ui.small_button("🗑").on_hover_text("Delete baseline").clicked() {
                                        delete_baseline_id = Some(baseline.id);
                                    }
                                } else {
                                    ui.small_button("🔒").on_hover_text("Baseline is locked");
                                }
                            });

                            ui.end_row();

                            // Show description if present
                            if let Some(ref desc) = baseline.description {
                                if !desc.is_empty() {
                                    ui.label(""); // Empty first column
                                    ui.add(egui::Label::new(egui::RichText::new(desc).italics().color(egui::Color32::GRAY)).wrap());
                                    ui.end_row();
                                }
                            }
                        }

                        // Handle delete after iteration
                        if let Some(id) = delete_baseline_id {
                            self.store.delete_baseline(&id);
                            if let Err(e) = self.storage.save(&self.store) {
                                self.message = Some((format!("Failed to save: {}", e), true));
                            }
                        }
                    });

                // Selected baseline details
                if let Some(baseline_id) = self.selected_baseline_id {
                    if let Some(baseline) = baselines.iter().find(|b| b.id == baseline_id) {
                        ui.add_space(20.0);
                        ui.separator();
                        ui.add_space(10.0);

                        ui.heading(format!("Baseline: {}", baseline.name));

                        if self.show_baseline_comparison {
                            self.show_baseline_comparison_inline(ui, baseline);
                        } else {
                            // Show requirements in this baseline
                            ui.label(format!("{} requirements in this baseline:", baseline.requirements.len()));
                            ui.add_space(5.0);

                            // Use remaining height for requirements list
                            let remaining_height = ui.available_height() - 20.0;
                            let scroll_height = remaining_height.max(100.0);

                            egui::ScrollArea::vertical()
                                .id_salt("baseline_reqs_scroll")
                                .max_height(scroll_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_min_width(available_width - 40.0);
                                    for snapshot in &baseline.requirements {
                                        ui.horizontal(|ui| {
                                            if let Some(ref spec_id) = snapshot.spec_id {
                                                ui.label(egui::RichText::new(spec_id).monospace().strong());
                                            }
                                            ui.label(&snapshot.title);
                                            ui.label(format!("[{:?}]", snapshot.status));
                                        });
                                    }
                                });
                        }
                    }
                }
            });
    }

    /// Show comparison between baseline and current state
    fn show_baseline_comparison_inline(&mut self, ui: &mut egui::Ui, baseline: &aida_core::Baseline) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Comparing with current state").strong());
            if ui.small_button("✕ Close Comparison").clicked() {
                self.show_baseline_comparison = false;
            }
        });

        if let Some(comparison) = self.store.compare_with_baseline(&baseline.id) {
            ui.add_space(10.0);

            // Summary
            ui.horizontal(|ui| {
                if !comparison.added.is_empty() {
                    ui.label(egui::RichText::new(format!("➕ {} added", comparison.added.len()))
                        .color(egui::Color32::from_rgb(100, 200, 100)));
                }
                if !comparison.removed.is_empty() {
                    ui.label(egui::RichText::new(format!("➖ {} removed", comparison.removed.len()))
                        .color(egui::Color32::from_rgb(200, 100, 100)));
                }
                if !comparison.modified.is_empty() {
                    ui.label(egui::RichText::new(format!("✏ {} modified", comparison.modified.len()))
                        .color(egui::Color32::from_rgb(200, 200, 100)));
                }
                ui.label(format!("({} unchanged)", comparison.unchanged.len()));
            });

            ui.add_space(10.0);

            // Added requirements
            if !comparison.added.is_empty() {
                ui.collapsing(format!("➕ Added ({}):", comparison.added.len()), |ui| {
                    for id in &comparison.added {
                        if let Some(req) = self.store.requirements.iter().find(|r| &r.id == id) {
                            ui.horizontal(|ui| {
                                if let Some(ref spec_id) = req.spec_id {
                                    ui.label(egui::RichText::new(spec_id).monospace().color(egui::Color32::from_rgb(100, 200, 100)));
                                }
                                ui.label(&req.title);
                            });
                        }
                    }
                });
            }

            // Removed requirements
            if !comparison.removed.is_empty() {
                ui.collapsing(format!("➖ Removed ({}):", comparison.removed.len()), |ui| {
                    for id in &comparison.removed {
                        if let Some(snapshot) = baseline.requirements.iter().find(|s| &s.original_id == id) {
                            ui.horizontal(|ui| {
                                if let Some(ref spec_id) = snapshot.spec_id {
                                    ui.label(egui::RichText::new(spec_id).monospace().color(egui::Color32::from_rgb(200, 100, 100)));
                                }
                                ui.label(&snapshot.title);
                            });
                        }
                    }
                });
            }

            // Modified requirements
            if !comparison.modified.is_empty() {
                ui.collapsing(format!("✏ Modified ({}):", comparison.modified.len()), |ui| {
                    for diff in &comparison.modified {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                if let Some(ref spec_id) = diff.spec_id {
                                    ui.label(egui::RichText::new(spec_id).monospace().strong());
                                }
                            });

                            for change in &diff.changes {
                                ui.horizontal(|ui| {
                                    ui.label(format!("  {}:", change.field_name));
                                    ui.label(egui::RichText::new(&change.old_value).strikethrough().color(egui::Color32::from_rgb(200, 100, 100)));
                                    ui.label("→");
                                    ui.label(egui::RichText::new(&change.new_value).color(egui::Color32::from_rgb(100, 200, 100)));
                                });
                            }
                        });
                    }
                });
            }
        } else {
            ui.label("Unable to compute comparison");
        }
    }

    /// Show the create baseline dialog
    fn show_create_baseline_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_create_baseline_dialog {
            return;
        }

        let mut close_dialog = false;
        let mut create_baseline = false;

        egui::Window::new("Create Baseline")
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label("Create a snapshot of all current requirements.");
                ui.add_space(10.0);

                egui::Grid::new("create_baseline_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Name:");
                        ui.add(egui::TextEdit::singleline(&mut self.baseline_form_name)
                            .hint_text("e.g., Release 1.0, Sprint 5 End")
                            .desired_width(250.0));
                        ui.end_row();

                        ui.label("Description:");
                        ui.add(egui::TextEdit::multiline(&mut self.baseline_form_description)
                            .hint_text("Optional description...")
                            .desired_width(250.0)
                            .desired_rows(3));
                        ui.end_row();
                    });

                ui.add_space(10.0);

                // Summary of what will be captured
                let active_count = self.store.requirements.iter().filter(|r| !r.archived).count();
                ui.label(format!("This will snapshot {} active requirements.", active_count));

                ui.add_space(15.0);

                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close_dialog = true;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_create = !self.baseline_form_name.trim().is_empty();
                        if ui.add_enabled(can_create, egui::Button::new("✓ Create Baseline")).clicked() {
                            create_baseline = true;
                        }
                    });
                });
            });

        if close_dialog {
            self.show_create_baseline_dialog = false;
        }

        if create_baseline {
            let name = self.baseline_form_name.trim().to_string();
            let description = if self.baseline_form_description.trim().is_empty() {
                None
            } else {
                Some(self.baseline_form_description.trim().to_string())
            };

            // Get username from settings
            let created_by = if !self.user_settings.name.is_empty() {
                self.user_settings.name.clone()
            } else {
                "Unknown".to_string()
            };

            // Create the baseline
            let baseline = self.store.create_baseline(name.clone(), description, created_by).clone();

            // Save
            if let Err(e) = self.storage.save(&self.store) {
                self.message = Some((format!("Failed to save baseline: {}", e), true));
            } else {
                // Select the new baseline
                self.selected_baseline_id = Some(baseline.id);

                // Show success message
                let git_msg = if baseline.git_tag.is_some() {
                    " (git tag created)"
                } else {
                    ""
                };
                self.message = Some((format!("Baseline '{}' created{}", name, git_msg), false));
            }

            self.show_create_baseline_dialog = false;
        }

        // Close on Escape
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_create_baseline_dialog = false;
        }
    }

    // =========================================================================
    // Timeline View
    // =========================================================================

    /// Rebuild the cached timeline events from all requirements
    fn rebuild_timeline_events(&mut self) {
        let mut events: Vec<TimelineEvent> = Vec::new();

        // Collect events from all requirements
        for req in &self.store.requirements {
            let spec_id = req.spec_id.clone().unwrap_or_else(|| "N/A".to_string());
            let req_title = req.title.clone();

            // Add creation event
            events.push(TimelineEvent {
                timestamp: req.created_at,
                event_type: TimelineEventType::Created,
                req_id: req.id,
                spec_id: spec_id.clone(),
                req_title: req_title.clone(),
                author: req.created_by.clone().unwrap_or_else(|| "Unknown".to_string()),
                description: format!("Created requirement: {}", req_title),
                changes: Vec::new(),
            });

            // Add history entries as modification events
            for history in &req.history {
                let description = if history.changes.len() == 1 {
                    format!(
                        "Changed {}: {} → {}",
                        history.changes[0].field_name,
                        history.changes[0].old_value,
                        history.changes[0].new_value
                    )
                } else {
                    format!("Modified {} fields", history.changes.len())
                };

                events.push(TimelineEvent {
                    timestamp: history.timestamp,
                    event_type: TimelineEventType::Modified,
                    req_id: req.id,
                    spec_id: spec_id.clone(),
                    req_title: req_title.clone(),
                    author: history.author.clone(),
                    description,
                    changes: history.changes.clone(),
                });
            }

            // Add comment events (recursively)
            Self::collect_comment_events(
                &req.comments,
                req.id,
                &spec_id,
                &req_title,
                &mut events,
            );
        }

        // Add baseline creation events
        for baseline in &self.store.baselines {
            events.push(TimelineEvent {
                timestamp: baseline.created_at,
                event_type: TimelineEventType::BaselineCreated,
                req_id: Uuid::nil(), // Not associated with a specific requirement
                spec_id: String::new(),
                req_title: baseline.name.clone(),
                author: baseline.created_by.clone(),
                description: format!(
                    "Baseline created: {} ({} requirements)",
                    baseline.name,
                    baseline.requirements.len()
                ),
                changes: Vec::new(),
            });
        }

        // Sort by timestamp (newest first)
        events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        self.timeline_events = events;
        self.timeline_selected_event_idx = None;
    }

    /// Helper to recursively collect comment events
    fn collect_comment_events(
        comments: &[aida_core::Comment],
        req_id: Uuid,
        spec_id: &str,
        req_title: &str,
        events: &mut Vec<TimelineEvent>,
    ) {
        for comment in comments {
            events.push(TimelineEvent {
                timestamp: comment.created_at,
                event_type: TimelineEventType::CommentAdded,
                req_id,
                spec_id: spec_id.to_string(),
                req_title: req_title.to_string(),
                author: comment.author.clone(),
                description: if comment.content.len() > 80 {
                    format!("{}...", &comment.content[..80])
                } else {
                    comment.content.clone()
                },
                changes: Vec::new(),
            });

            // Recursively collect replies
            Self::collect_comment_events(&comment.replies, req_id, spec_id, req_title, events);
        }
    }

    /// Get filtered timeline events based on current filters
    fn get_filtered_timeline_events(&self) -> Vec<&TimelineEvent> {
        self.timeline_events
            .iter()
            .filter(|e| {
                // Filter by author
                if !self.timeline_filter_author.is_empty() {
                    if !e.author.to_lowercase().contains(&self.timeline_filter_author.to_lowercase()) {
                        return false;
                    }
                }
                // Filter by field name (for modifications)
                if !self.timeline_filter_field.is_empty() {
                    if e.event_type != TimelineEventType::Modified {
                        return false;
                    }
                    let filter_lower = self.timeline_filter_field.to_lowercase();
                    if !e.changes.iter().any(|c| c.field_name.to_lowercase().contains(&filter_lower)) {
                        return false;
                    }
                }
                true
            })
            .collect()
    }

    /// Show the Timeline view
    fn show_timeline_view(&mut self, ui: &mut egui::Ui) {
        // Check if mouse has moved significantly from suppression point - if so, re-enable hover
        // We compare against the position where hover was suppressed, not the last frame position,
        // to avoid re-enabling hover due to small mouse movements during click actions
        if self.timeline_suppress_hover {
            if let Some(current) = ui.ctx().input(|i| i.pointer.hover_pos()) {
                if let Some(suppress_pos) = self.timeline_suppress_hover_pos {
                    let dist = (current - suppress_pos).length();
                    // Require significant movement (15px) from the suppression point to re-enable hover
                    if dist > 15.0 {
                        self.timeline_suppress_hover = false;
                        self.timeline_suppress_hover_pos = None;
                    }
                }
            }
        }

        // Header with controls
        ui.horizontal(|ui| {
            ui.heading("📅 Timeline");

            ui.separator();

            if ui.button("🔄 Refresh").clicked() {
                self.rebuild_timeline_events();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("📋 Back to List").clicked() {
                    self.pending_view_change = Some(View::List);
                }
            });
        });

        ui.separator();

        // Filters row
        ui.horizontal(|ui| {
            ui.label("Filter by:");

            ui.label("Author:");
            let author_response = ui.add(
                egui::TextEdit::singleline(&mut self.timeline_filter_author)
                    .hint_text("any")
                    .desired_width(100.0),
            );

            ui.separator();

            ui.label("Field:");
            let field_response = ui.add(
                egui::TextEdit::singleline(&mut self.timeline_filter_field)
                    .hint_text("any (e.g., status)")
                    .desired_width(120.0),
            );

            if !self.timeline_filter_author.is_empty() || !self.timeline_filter_field.is_empty() {
                if ui.button("✕ Clear").clicked() {
                    self.timeline_filter_author.clear();
                    self.timeline_filter_field.clear();
                }
            }

            ui.separator();

            let filtered = self.get_filtered_timeline_events();
            ui.label(format!("{} events", filtered.len()));
        });

        ui.separator();

        // Get filtered events - clone to avoid borrow issues with closures
        let filtered_events: Vec<TimelineEvent> = self.get_filtered_timeline_events()
            .into_iter()
            .cloned()
            .collect();
        let no_events = self.timeline_events.is_empty();

        if filtered_events.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(50.0);
                    if no_events {
                        ui.label(
                            egui::RichText::new("No timeline events yet")
                                .size(16.0)
                                .color(egui::Color32::GRAY),
                        );
                        ui.add_space(10.0);
                        ui.label("Timeline shows all changes to requirements over time.");
                        ui.label("Create or modify requirements to see events here.");
                    } else {
                        ui.label(
                            egui::RichText::new("No events match the current filters")
                                .size(16.0)
                                .color(egui::Color32::GRAY),
                        );
                    }
                });
            });
            return;
        }

        // Pre-compute the selected event before entering closures
        let selected_event_idx = self.timeline_selected_event_idx;
        let selected_event: Option<TimelineEvent> = selected_event_idx
            .and_then(|idx| filtered_events.get(idx).cloned());

        // Pre-compute requirement index lookup for navigation
        let req_positions: std::collections::HashMap<Uuid, usize> = self
            .store
            .requirements
            .iter()
            .enumerate()
            .map(|(idx, r)| (r.id, idx))
            .collect();

        // Track state changes that need to happen after the UI render
        let mut new_selected_idx: Option<usize> = None;
        let mut navigate_to_req: Option<(usize, View)> = None;
        let mut suppress_hover_pos: Option<egui::Pos2> = None;
        let suppress_hover = self.timeline_suppress_hover;

        // Timeline display - two columns: list on left, detail on right
        ui.columns(2, |columns| {
            // Left column: Event list
            egui::ScrollArea::vertical()
                .id_salt("timeline_events")
                .auto_shrink([false, false])
                .show(&mut columns[0], |ui| {
                    let mut current_date: Option<chrono::NaiveDate> = None;

                    for (idx, event) in filtered_events.iter().enumerate() {
                        let event_date = event.timestamp.date_naive();

                        // Show date header when date changes
                        if current_date != Some(event_date) {
                            if current_date.is_some() {
                                ui.add_space(10.0);
                            }
                            ui.label(
                                egui::RichText::new(event_date.format("%A, %B %d, %Y").to_string())
                                    .strong()
                                    .color(egui::Color32::from_rgb(100, 150, 200)),
                            );
                            ui.separator();
                            current_date = Some(event_date);
                        }

                        // Event row - custom rendering to support hover suppression
                        let selected = selected_event_idx == Some(idx);
                        let label_text = format!(
                            "{} {} {} - {} by {}",
                            event.timestamp.format("%H:%M"),
                            event.event_type.icon(),
                            event.event_type.label(),
                            if event.spec_id.is_empty() {
                                event.req_title.clone()
                            } else {
                                format!("{} ({})", event.spec_id, truncate_string(&event.req_title, 30))
                            },
                            event.author
                        );

                        // Use custom selectable with hover suppression
                        let response = if suppress_hover {
                            // When hover is suppressed, manually draw the item without hover effect
                            // Match the sizing of selectable_label exactly
                            let button_padding = ui.spacing().button_padding;
                            let text = egui::WidgetText::from(&label_text);
                            let galley = text.into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
                            let desired_size = egui::vec2(
                                ui.available_width(),
                                (galley.size().y + button_padding.y * 2.0).max(ui.spacing().interact_size.y)
                            );
                            let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

                            if ui.is_rect_visible(rect) {
                                // Only show selection highlight, no hover
                                if selected {
                                    let visuals = ui.visuals().selection;
                                    ui.painter().rect_filled(rect, ui.visuals().widgets.hovered.rounding, visuals.bg_fill);
                                }
                                let text_pos = egui::Align2::LEFT_CENTER.anchor_size(
                                    rect.left_center() + egui::vec2(button_padding.x, 0.0),
                                    galley.size()
                                ).min;
                                ui.painter().galley(text_pos, galley, ui.visuals().text_color());
                            }
                            response
                        } else {
                            ui.selectable_label(selected, &label_text)
                        };

                        if response.clicked() {
                            new_selected_idx = Some(idx);
                            // Capture mouse position for hover suppression
                            suppress_hover_pos = ui.ctx().input(|i| i.pointer.hover_pos());
                        }

                        // Double-click to navigate to requirement
                        if response.double_clicked() && !event.req_id.is_nil() {
                            if let Some(&req_idx) = req_positions.get(&event.req_id) {
                                navigate_to_req = Some((req_idx, View::List));
                            }
                        }
                    }
                });

            // Right column: Event detail with scroll support
            egui::ScrollArea::both()
                .id_salt("timeline_detail")
                .auto_shrink([false, false])
                .show(&mut columns[1], |ui| {
                if let Some(event) = &selected_event {
                    ui.heading(format!("{} {}", event.event_type.icon(), event.event_type.label()));
                    ui.add_space(10.0);

                    // Track if we need to navigate via link click
                    let mut link_navigate_req: Option<Uuid> = None;

                    egui::Grid::new("event_detail_grid")
                        .num_columns(2)
                        .spacing([20.0, 8.0])
                        .show(ui, |ui| {
                            ui.label("Time:");
                            ui.label(event.timestamp.format("%Y-%m-%d %H:%M:%S").to_string());
                            ui.end_row();

                            ui.label("Author:");
                            ui.label(&event.author);
                            ui.end_row();

                            if !event.spec_id.is_empty() {
                                ui.label("Requirement:");
                                if ui.link(&event.spec_id).clicked() {
                                    link_navigate_req = Some(event.req_id);
                                }
                                ui.end_row();

                                ui.label("Title:");
                                ui.label(&event.req_title);
                                ui.end_row();
                            }
                        });

                    // Handle link navigation
                    if let Some(req_id) = link_navigate_req {
                        if let Some(&req_idx) = req_positions.get(&req_id) {
                            navigate_to_req = Some((req_idx, View::List));
                        }
                    }

                    ui.add_space(15.0);
                    ui.separator();
                    ui.add_space(10.0);

                    // Show changes for modification events
                    if event.event_type == TimelineEventType::Modified && !event.changes.is_empty() {
                        ui.label(egui::RichText::new("Changes:").strong());
                        ui.add_space(5.0);

                        for change in &event.changes {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&change.field_name)
                                        .monospace()
                                        .color(egui::Color32::from_rgb(150, 150, 200)),
                                );
                            });

                            ui.horizontal_wrapped(|ui| {
                                ui.add_space(20.0);
                                // Use inline word diff for better readability
                                render_inline_diff(ui, &change.old_value, &change.new_value);
                            });

                            ui.add_space(5.0);
                        }
                    } else {
                        // Show description for other event types
                        ui.label(egui::RichText::new("Description:").strong());
                        ui.add_space(5.0);
                        ui.label(&event.description);
                    }
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new("Select an event to see details")
                                .color(egui::Color32::GRAY),
                        );
                    });
                }
            });
        });

        // Apply state changes after UI render
        if let Some(idx) = new_selected_idx {
            self.timeline_selected_event_idx = Some(idx);
        }
        if let Some(pos) = suppress_hover_pos {
            self.timeline_suppress_hover = true;
            self.timeline_suppress_hover_pos = Some(pos);
        }
        if let Some((req_idx, view)) = navigate_to_req {
            self.selected_idx = Some(req_idx);
            self.pending_view_change = Some(view);
            // Center the requirement in the list when navigating from Timeline
            self.scroll_to_center = true;
            if let Some(req) = self.store.requirements.get(req_idx) {
                self.scroll_to_requirement = Some(req.id);
            }
        }
    }

    /// Get a flat list of all planning items in display order (sprints items first, then backlog)
    /// Returns a list of (item_id, in_sprint) tuples
    fn get_planning_items_list(&self) -> Vec<Uuid> {
        let mut items = Vec::new();

        // Get sprints (filtered by show_completed setting)
        let sprints: Vec<Uuid> = self.store
            .get_sprints()
            .into_iter()
            .filter(|s| {
                let status = s.effective_status();
                self.planning_show_completed_sprints
                    || (status != "Completed" && status != "Archived")
            })
            .map(|s| s.id)
            .collect();

        // Add items from each non-collapsed sprint
        for sprint_id in &sprints {
            if !self.planning_collapsed_sprints.contains(sprint_id) {
                let sprint_items = self.store.get_sprint_items(sprint_id);
                for item in sprint_items {
                    items.push(item.id);
                }
            }
        }

        // Add backlog items if not collapsed
        if !self.planning_collapsed_sprints.contains(&Uuid::nil()) {
            let backlog = self.store.get_backlog();
            for item in backlog {
                items.push(item.id);
            }
        }

        items
    }

    /// Show the Sprint Planning view
    fn show_planning_view(&mut self, ui: &mut egui::Ui) {
        // Header with controls
        ui.horizontal(|ui| {
            ui.heading("🏃 Sprint Planning");

            ui.separator();

            if ui.button("➕ New Sprint").clicked() {
                // Create a new Sprint requirement
                self.clear_form();
                self.form_type = RequirementType::Sprint;
                self.pending_view_change = Some(View::Add);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Show completed sprints toggle
                ui.checkbox(&mut self.planning_show_completed_sprints, "Show Completed");

                if ui.button("📋 Back to List").clicked() {
                    self.pending_view_change = Some(View::List);
                }
            });
        });

        ui.separator();

        // Get sprints and backlog
        let sprints: Vec<(Uuid, String, String, String, String)> = self.store
            .get_sprints()
            .into_iter()
            .filter_map(|s| {
                let status = s.effective_status();
                // Filter out completed/archived unless show_completed is enabled
                if !self.planning_show_completed_sprints
                    && (status == "Completed" || status == "Archived") {
                    return None;
                }
                Some((
                    s.id,
                    s.spec_id.clone().unwrap_or_default(),
                    s.title.clone(),
                    status,
                    s.custom_fields.get("sprint_goal").cloned().unwrap_or_default(),
                ))
            })
            .collect();

        let backlog_items: Vec<(Uuid, String, String, String, RequirementType)> = self.store
            .get_backlog()
            .into_iter()
            .map(|r| (r.id, r.spec_id.clone().unwrap_or_default(), r.title.clone(), r.effective_status(), r.req_type.clone()))
            .collect();

        // Check if we have any content
        if sprints.is_empty() && backlog_items.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(50.0);
                    ui.label(
                        egui::RichText::new("No sprints or backlog items")
                            .size(16.0)
                            .color(egui::Color32::GRAY),
                    );
                    ui.add_space(10.0);
                    ui.label("Create a Sprint to start planning work.");
                    ui.add_space(10.0);
                    if ui.button("➕ Create Sprint").clicked() {
                        self.clear_form();
                        self.form_type = RequirementType::Sprint;
                        self.pending_view_change = Some(View::Add);
                    }
                });
            });
            return;
        }

        // Main content area with scroll
        egui::ScrollArea::vertical()
            .id_salt("planning_view")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Check if we're currently dragging
                let is_dragging = self.planning_drag_source.is_some();

                // Render each Sprint section
                for (sprint_id, spec_id, title, status, goal) in &sprints {
                    let is_collapsed = self.planning_collapsed_sprints.contains(sprint_id);
                    let is_drop_target = self.planning_drag_target_sprint == Some(Some(*sprint_id));

                    // Sprint header - make it a drop zone
                    let header_response = ui.scope(|ui| {
                        // Highlight if this is the current drop target
                        if is_drop_target && is_dragging {
                            ui.visuals_mut().widgets.noninteractive.bg_fill =
                                egui::Color32::from_rgba_unmultiplied(59, 130, 246, 50);
                        }

                        ui.horizontal(|ui| {
                            let collapse_icon = if is_collapsed { "▶" } else { "▼" };
                            if ui.button(collapse_icon).clicked() {
                                if is_collapsed {
                                    self.planning_collapsed_sprints.remove(sprint_id);
                                } else {
                                    self.planning_collapsed_sprints.insert(*sprint_id);
                                }
                            }

                            // Status indicator color
                            let status_color = match status.as_str() {
                                "Draft" => egui::Color32::from_rgb(156, 163, 175),
                                "In Progress" => egui::Color32::from_rgb(59, 130, 246),
                                "Completed" => egui::Color32::from_rgb(34, 197, 94),
                                "Archived" => egui::Color32::from_rgb(107, 114, 128),
                                _ => egui::Color32::GRAY,
                            };

                            ui.colored_label(status_color, "●");

                            let header_text = format!("{} - {}", spec_id, title);
                            ui.strong(&header_text);

                            if !goal.is_empty() {
                                ui.label(format!("({})", goal));
                            }

                            // Sprint item count
                            let sprint_items: Vec<_> = self.store
                                .get_sprint_items(sprint_id)
                                .into_iter()
                                .map(|r| (r.id, r.spec_id.clone().unwrap_or_default(), r.title.clone(), r.effective_status(), r.req_type.clone()))
                                .collect();
                            ui.label(format!("[{} items]", sprint_items.len()));

                            // Show drop indicator when dragging
                            if is_dragging && is_drop_target {
                                ui.label(egui::RichText::new("⬅ Drop here").color(egui::Color32::from_rgb(59, 130, 246)));
                            }
                        })
                    });

                    // Handle drop zone for sprint header - use pointer position for reliable detection
                    if is_dragging {
                        if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                            if header_response.response.rect.contains(pointer_pos) {
                                self.planning_drag_target_sprint = Some(Some(*sprint_id));
                            }
                        }
                    }

                    if !is_collapsed {
                        // Get items for this sprint
                        let sprint_items: Vec<_> = self.store
                            .get_sprint_items(sprint_id)
                            .into_iter()
                            .map(|r| (r.id, r.spec_id.clone().unwrap_or_default(), r.title.clone(), r.effective_status(), r.req_type.clone()))
                            .collect();

                        ui.indent(format!("sprint_{}", sprint_id), |ui| {
                            if sprint_items.is_empty() {
                                // Show drop zone indicator when empty and dragging
                                if is_dragging {
                                    let empty_response = ui.label(
                                        egui::RichText::new("Drop item here to add to this Sprint")
                                            .italics()
                                            .color(if is_drop_target {
                                                egui::Color32::from_rgb(59, 130, 246)
                                            } else {
                                                egui::Color32::GRAY
                                            }),
                                    );
                                    // Use pointer position for reliable drop detection
                                    if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                                        if empty_response.rect.contains(pointer_pos) {
                                            self.planning_drag_target_sprint = Some(Some(*sprint_id));
                                        }
                                    }
                                } else {
                                    ui.label(
                                        egui::RichText::new("No items assigned to this Sprint")
                                            .italics()
                                            .color(egui::Color32::GRAY),
                                    );
                                }
                            } else {
                                for (item_id, item_spec_id, item_title, item_status, item_type) in &sprint_items {
                                    self.render_planning_item(ui, *item_id, item_spec_id, item_title, item_status, item_type, Some(*sprint_id));
                                }
                            }
                        });
                    }

                    ui.add_space(10.0);
                    ui.separator();
                }

                // Backlog section
                ui.add_space(10.0);

                let backlog_collapsed = self.planning_collapsed_sprints.contains(&Uuid::nil());
                let is_backlog_drop_target = self.planning_drag_target_sprint == Some(None);

                // Backlog header - make it a drop zone
                let backlog_header_response = ui.scope(|ui| {
                    // Highlight if this is the current drop target
                    if is_backlog_drop_target && is_dragging {
                        ui.visuals_mut().widgets.noninteractive.bg_fill =
                            egui::Color32::from_rgba_unmultiplied(245, 158, 11, 50);
                    }

                    ui.horizontal(|ui| {
                        let collapse_icon = if backlog_collapsed { "▶" } else { "▼" };
                        if ui.button(collapse_icon).clicked() {
                            if backlog_collapsed {
                                self.planning_collapsed_sprints.remove(&Uuid::nil());
                            } else {
                                self.planning_collapsed_sprints.insert(Uuid::nil());
                            }
                        }

                        ui.colored_label(egui::Color32::from_rgb(245, 158, 11), "●");
                        ui.strong("Backlog");
                        ui.label(format!("[{} items]", backlog_items.len()));

                        // Show drop indicator when dragging
                        if is_dragging && is_backlog_drop_target {
                            ui.label(egui::RichText::new("⬅ Drop here").color(egui::Color32::from_rgb(245, 158, 11)));
                        }
                    })
                });

                // Handle drop zone for backlog header - use pointer position for reliable detection
                if is_dragging {
                    if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                        if backlog_header_response.response.rect.contains(pointer_pos) {
                            self.planning_drag_target_sprint = Some(None);
                        }
                    }
                }

                if !backlog_collapsed {
                    ui.indent("backlog", |ui| {
                        if backlog_items.is_empty() {
                            // Show drop zone indicator when empty and dragging
                            if is_dragging {
                                let empty_response = ui.label(
                                    egui::RichText::new("Drop item here to move to Backlog")
                                        .italics()
                                        .color(if is_backlog_drop_target {
                                            egui::Color32::from_rgb(245, 158, 11)
                                        } else {
                                            egui::Color32::GRAY
                                        }),
                                );
                                // Use pointer position for reliable drop detection
                                if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                                    if empty_response.rect.contains(pointer_pos) {
                                        self.planning_drag_target_sprint = Some(None);
                                    }
                                }
                            } else {
                                ui.label(
                                    egui::RichText::new("All items are assigned to Sprints")
                                        .italics()
                                        .color(egui::Color32::GRAY),
                                );
                            }
                        } else {
                            for (item_id, item_spec_id, item_title, item_status, item_type) in &backlog_items {
                                self.render_planning_item(ui, *item_id, item_spec_id, item_title, item_status, item_type, None);
                            }
                        }
                    });
                }

                // Clear drop target if not hovering over any drop zone
                if is_dragging && !ui.ui_contains_pointer() {
                    self.planning_drag_target_sprint = None;
                }
            });
    }

    /// Render a single item in the planning view
    fn render_planning_item(
        &mut self,
        ui: &mut egui::Ui,
        item_id: Uuid,
        spec_id: &str,
        title: &str,
        status: &str,
        item_type: &RequirementType,
        current_sprint: Option<Uuid>,
    ) {
        let type_icon = match item_type {
            RequirementType::Functional => "📋",
            RequirementType::NonFunctional => "⚙️",
            RequirementType::System => "🖥️",
            RequirementType::User => "👤",
            RequirementType::ChangeRequest => "🔄",
            RequirementType::Bug => "🐛",
            RequirementType::Epic => "🎯",
            RequirementType::Story => "📖",
            RequirementType::Task => "✅",
            RequirementType::Spike => "🔬",
            RequirementType::Sprint => "🏃",
            RequirementType::Folder => "📁",
        };

        let status_color = match status {
            "Draft" => egui::Color32::from_rgb(156, 163, 175),
            "In Progress" => egui::Color32::from_rgb(59, 130, 246),
            "Implemented" => egui::Color32::from_rgb(16, 185, 129),
            "Verified" => egui::Color32::from_rgb(34, 197, 94),
            "Rejected" => egui::Color32::from_rgb(239, 68, 68),
            "Completed" => egui::Color32::from_rgb(34, 197, 94),
            _ => egui::Color32::GRAY,
        };

        let is_selected = self.planning_selected_item == Some(item_id);
        let is_drag_source = self.planning_drag_source == Some(item_id);

        // Build the label text
        let label_text = format!("{} {} {}: {}", type_icon, "●", spec_id, title);

        // Calculate visual styling
        let (bg_color, stroke) = if is_drag_source {
            (
                egui::Color32::from_rgba_unmultiplied(100, 100, 200, 60),
                egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE),
            )
        } else if is_selected {
            (ui.visuals().selection.bg_fill, egui::Stroke::NONE)
        } else {
            (egui::Color32::TRANSPARENT, egui::Stroke::NONE)
        };

        // Use allocate_exact_size with click_and_drag sense for proper drag support
        let available_width = ui.available_width() - 8.0;
        let text = egui::WidgetText::from(&label_text);
        let galley = text.into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            available_width.max(50.0),
            egui::TextStyle::Body,
        );
        let desired_size = egui::vec2(available_width.max(50.0), galley.size().y) + egui::vec2(8.0, 4.0);

        let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());

        // Paint background
        if bg_color != egui::Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, 2.0, bg_color);
        }
        if stroke != egui::Stroke::NONE {
            ui.painter().rect_stroke(rect, 2.0, stroke);
        }

        // Paint text with appropriate color
        let text_pos = rect.min + egui::vec2(4.0, 2.0);
        let text_color = if is_selected {
            ui.visuals().selection.stroke.color
        } else {
            ui.visuals().text_color()
        };

        // Paint status dot separately with color
        let status_text = format!("{} ", type_icon);
        let status_galley = egui::WidgetText::from(&status_text).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            50.0,
            egui::TextStyle::Body,
        );
        ui.painter().galley(text_pos, status_galley, text_color);

        // Paint the rest
        let rest_pos = text_pos + egui::vec2(20.0, 0.0);
        let dot_galley = egui::WidgetText::from("● ").into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            20.0,
            egui::TextStyle::Body,
        );
        ui.painter().galley(rest_pos, dot_galley, status_color);

        let label_pos = rest_pos + egui::vec2(18.0, 0.0);
        let label_galley = egui::WidgetText::from(format!("{}: {}", spec_id, title)).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            (available_width - 50.0).max(50.0),
            egui::TextStyle::Body,
        );
        ui.painter().galley(label_pos, label_galley, text_color);

        // Handle click for selection
        if response.clicked() {
            self.planning_selected_item = Some(item_id);
            if let Some(idx) = self.store.requirements.iter().position(|r| r.id == item_id) {
                self.selected_idx = Some(idx);
            }
        }

        // Handle drag start
        if response.drag_started() {
            self.planning_drag_source = Some(item_id);
        }

        // Show drag indicator while dragging
        if is_drag_source && ui.input(|i| i.pointer.is_decidedly_dragging()) {
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                egui::Area::new(egui::Id::new("planning_drag_indicator"))
                    .fixed_pos(pos + egui::vec2(10.0, 10.0))
                    .order(egui::Order::Tooltip)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(format!("📎 {}", spec_id));
                        });
                    });
            }
        }

        // Handle drop zone detection for this item - if dragging and hovering over this item,
        // set the target to this item's current sprint (or backlog if None)
        if self.planning_drag_source.is_some() && self.planning_drag_source != Some(item_id) {
            if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                if rect.contains(pointer_pos) {
                    self.planning_drag_target_sprint = Some(current_sprint);
                }
            }
        }

        // Context menu for sprint assignment
        response.context_menu(|ui| {
            ui.menu_button("🏃 Assign to Sprint...", |ui| {
                // Get available sprints
                let sprints: Vec<_> = self.store.get_sprints()
                    .into_iter()
                    .filter(|s| {
                        let status = s.effective_status();
                        status != "Completed" && status != "Archived"
                    })
                    .map(|s| (s.id, s.spec_id.clone().unwrap_or_default(), s.title.clone()))
                    .collect();

                if sprints.is_empty() {
                    ui.label("No active sprints available");
                } else {
                    for (sprint_id, sprint_spec_id, sprint_title) in sprints {
                        if ui.button(format!("{}: {}", sprint_spec_id, sprint_title)).clicked() {
                            let username = self.user_settings.display_name();
                            self.store.assign_to_sprint(item_id, sprint_id, &username);
                            self.save();
                            ui.close_menu();
                        }
                    }
                }
            });

            if ui.button("📦 Move to Backlog").clicked() {
                let username = self.user_settings.display_name();
                self.store.remove_from_sprint(item_id, &username);
                self.save();
                ui.close_menu();
            }

            ui.separator();

            if ui.button("📋 View Details").clicked() {
                if let Some(idx) = self.store.requirements.iter().position(|r| r.id == item_id) {
                    self.selected_idx = Some(idx);
                    self.pending_view_change = Some(View::Detail);
                }
                ui.close_menu();
            }
        });
    }

    /// Show the detail panel for the Planning view (left side)
    fn show_planning_detail_panel(&mut self, ui: &mut egui::Ui) {
        // Get the selected item from planning_selected_item
        let selected_id = self.planning_selected_item;

        // Find the requirement index for the selected item
        let selected_req_idx = selected_id.and_then(|id| {
            self.store.requirements.iter().position(|r| r.id == id)
        });

        // Sync with selected_idx for consistency
        if let Some(idx) = selected_req_idx {
            if self.selected_idx != Some(idx) {
                self.selected_idx = Some(idx);
            }
        }

        ui.heading("Details");
        ui.separator();

        if selected_req_idx.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new("Select an item to view details")
                        .italics()
                        .color(egui::Color32::GRAY),
                );
            });
            return;
        }

        // Use the existing detail view internal, but in non-stacked mode
        self.show_detail_view_internal(ui, false, false);
    }

    /// Show the clone requirement dialog
    fn show_clone_requirement_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_clone_dialog {
            return;
        }

        let Some(source_idx) = self.clone_source_idx else {
            self.show_clone_dialog = false;
            return;
        };

        let Some(source_req) = self.store.requirements.get(source_idx).cloned() else {
            self.show_clone_dialog = false;
            return;
        };

        let mut close_dialog = false;
        let mut do_clone = false;

        egui::Window::new("Clone Requirement")
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(format!("Clone: {}", source_req.spec_id.as_deref().unwrap_or("(no ID)")));
                ui.label(egui::RichText::new(&source_req.title).strong());
                ui.add_space(10.0);

                ui.label("The cloned requirement will have a new ID and start with Draft status.");
                ui.add_space(10.0);

                ui.separator();
                ui.label(egui::RichText::new("Include in clone:").strong());
                ui.add_space(5.0);

                // Core fields are always cloned
                ui.horizontal(|ui| {
                    ui.add_enabled(false, egui::Checkbox::new(&mut true, "Title & Description"));
                    ui.label(egui::RichText::new("(always)").small().color(egui::Color32::GRAY));
                });
                ui.horizontal(|ui| {
                    ui.add_enabled(false, egui::Checkbox::new(&mut true, "Type, Priority, Owner, Feature"));
                    ui.label(egui::RichText::new("(always)").small().color(egui::Color32::GRAY));
                });

                ui.add_space(5.0);

                // Optional fields
                ui.checkbox(&mut self.clone_include_tags, "Tags");

                let has_urls = !source_req.urls.is_empty();
                let url_label = if has_urls {
                    format!("URL Links ({})", source_req.urls.len())
                } else {
                    "URL Links (none)".to_string()
                };
                ui.add_enabled(has_urls, egui::Checkbox::new(&mut self.clone_include_urls, url_label));

                let has_custom = !source_req.custom_fields.is_empty();
                let custom_label = if has_custom {
                    format!("Custom Fields ({})", source_req.custom_fields.len())
                } else {
                    "Custom Fields (none)".to_string()
                };
                ui.add_enabled(has_custom, egui::Checkbox::new(&mut self.clone_include_custom_fields, custom_label));

                ui.add_space(5.0);
                ui.separator();
                ui.add_space(5.0);

                let has_relationships = !source_req.relationships.is_empty();
                let rel_label = if has_relationships {
                    format!("Relationships ({})", source_req.relationships.len())
                } else {
                    "Relationships (none)".to_string()
                };
                ui.add_enabled(has_relationships, egui::Checkbox::new(&mut self.clone_include_relationships, rel_label));
                if has_relationships && self.clone_include_relationships {
                    ui.label(egui::RichText::new("  Note: Clone will have same relationships as original").small().italics());
                }

                let has_comments = !source_req.comments.is_empty();
                let comment_label = if has_comments {
                    format!("Comments ({})", source_req.comments.len())
                } else {
                    "Comments (none)".to_string()
                };
                ui.add_enabled(has_comments, egui::Checkbox::new(&mut self.clone_include_comments, comment_label));

                let has_history = !source_req.history.is_empty();
                let history_label = if has_history {
                    format!("History ({})", source_req.history.len())
                } else {
                    "History (none)".to_string()
                };
                ui.add_enabled(has_history, egui::Checkbox::new(&mut self.clone_include_history, history_label));

                ui.add_space(15.0);

                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close_dialog = true;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("📋 Clone").clicked() {
                            do_clone = true;
                        }
                    });
                });
            });

        if close_dialog {
            self.show_clone_dialog = false;
            self.clone_source_idx = None;
        }

        if do_clone {
            self.clone_requirement(source_idx);
            self.show_clone_dialog = false;
            self.clone_source_idx = None;
        }

        // Close on Escape
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_clone_dialog = false;
            self.clone_source_idx = None;
        }
    }

    /// Clone a requirement with the current dialog settings
    fn clone_requirement(&mut self, source_idx: usize) {
        let Some(source) = self.store.requirements.get(source_idx).cloned() else {
            return;
        };

        // Create new requirement with core fields
        let mut cloned = Requirement::new(
            format!("{} (Clone)", source.title),
            source.description.clone(),
        );

        // Copy type, priority, owner, feature
        cloned.req_type = source.req_type.clone();
        cloned.priority = source.priority.clone();
        cloned.owner = source.owner.clone();
        cloned.feature = source.feature.clone();

        // Status is always Draft for clones
        cloned.status = RequirementStatus::Draft;

        // Optional: tags
        if self.clone_include_tags {
            cloned.tags = source.tags.clone();
        }

        // Optional: URLs
        if self.clone_include_urls {
            cloned.urls = source.urls.clone();
        }

        // Optional: custom fields
        if self.clone_include_custom_fields {
            cloned.custom_fields = source.custom_fields.clone();
            cloned.custom_status = source.custom_status.clone();
            cloned.custom_priority = source.custom_priority.clone();
        }

        // Optional: relationships
        if self.clone_include_relationships {
            cloned.relationships = source.relationships.clone();
        }

        // Optional: comments
        if self.clone_include_comments {
            cloned.comments = source.comments.clone();
        }

        // Optional: history
        if self.clone_include_history {
            cloned.history = source.history.clone();
        }

        // Add creation record
        cloned.record_change(
            self.user_settings.display_name(),
            vec![aida_core::FieldChange {
                field_name: "cloned_from".to_string(),
                old_value: String::new(),
                new_value: source.spec_id.clone().unwrap_or_else(|| source.id.to_string()),
            }],
        );

        // Add to store with new spec_id
        self.store.add_requirement_with_spec_id(cloned);

        // Save
        if let Err(e) = self.storage.save(&self.store) {
            self.message = Some((format!("Failed to save: {}", e), true));
        } else {
            // Select the new requirement (last in list)
            let new_idx = self.store.requirements.len() - 1;
            self.selected_idx = Some(new_idx);

            let new_spec_id = self.store.requirements.get(new_idx)
                .and_then(|r| r.spec_id.clone())
                .unwrap_or_else(|| "new".to_string());

            self.message = Some((format!("Cloned as {}", new_spec_id), false));

            // Open the cloned requirement in edit mode
            self.load_form_from_requirement(new_idx);
            self.pending_view_change = Some(View::Edit);
        }
    }

    fn show_settings_users_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("User Management");
            ui.add_space(5.0);

            ui.horizontal(|ui| {
                if ui.button("➕ Add User").clicked() {
                    self.show_user_form = true;
                    self.editing_user_id = None;
                    self.user_form_name.clear();
                    self.user_form_email.clear();
                    self.user_form_handle.clear();
                }
                ui.checkbox(&mut self.show_archived_users, "Show Archived");
            });

            ui.add_space(5.0);

            // User form (inline)
            if self.show_user_form {
                ui.group(|ui| {
                    let title = if self.editing_user_id.is_some() {
                        "Edit User"
                    } else {
                        "Add User"
                    };
                    ui.label(title);

                    egui::Grid::new("user_form_grid")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .show(ui, |ui| {
                            ui.label("Name:");
                            ui.text_edit_singleline(&mut self.user_form_name);
                            ui.end_row();

                            ui.label("Email:");
                            ui.text_edit_singleline(&mut self.user_form_email);
                            ui.end_row();

                            ui.label("Handle:");
                            ui.text_edit_singleline(&mut self.user_form_handle);
                            ui.end_row();
                        });

                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            if self.editing_user_id.is_some() {
                                self.save_edited_user();
                            } else {
                                self.add_new_user();
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_user_form = false;
                            self.editing_user_id = None;
                        }
                    });
                });
                ui.add_space(5.0);
            }

            // Users table
            self.show_users_table(ui);
        });
    }

    fn show_settings_database_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Database Identity Section
            ui.heading("Database Identity");
            ui.add_space(5.0);

            // Name field
            ui.horizontal(|ui| {
                ui.label("Name:");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.store.name)
                        .desired_width(300.0)
                        .hint_text("Short identifier (e.g., PROJ-A)"),
                );
                if response.lost_focus() {
                    self.save();
                }
            });

            ui.add_space(5.0);

            // Title field
            ui.horizontal(|ui| {
                ui.label("Title:");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.store.title)
                        .desired_width(400.0)
                        .hint_text("One-line title for this database"),
                );
                if response.lost_focus() {
                    self.save();
                }
            });

            ui.add_space(5.0);

            // Description field (multiline)
            ui.label("Description:");
            let response = ui.add(
                egui::TextEdit::multiline(&mut self.store.description)
                    .desired_width(ui.available_width() - 20.0)
                    .desired_rows(4)
                    .hint_text("Detailed description of this requirements database..."),
            );
            if response.lost_focus() {
                self.save();
            }

            ui.add_space(5.0);
            ui.label("Window title: Name - Title (or whichever is set)");

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            // Database Management Section
            ui.heading("Database Management");
            ui.add_space(5.0);

            if ui.button("📦 Backup Database").clicked() {
                self.backup_database();
            }
            ui.label("Creates a timestamped backup of the requirements database.");

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            // Statistics Section
            ui.heading("Database Statistics");
            ui.add_space(5.0);

            let total_reqs = self.store.requirements.len();
            let archived_reqs = self
                .store
                .requirements
                .iter()
                .filter(|r| r.archived)
                .count();
            let active_reqs = total_reqs - archived_reqs;
            let total_users = self.store.users.len();
            let archived_users = self.store.users.iter().filter(|u| u.archived).count();

            egui::Grid::new("settings_stats_grid")
                .num_columns(2)
                .spacing([20.0, 5.0])
                .show(ui, |ui| {
                    ui.label("Total Requirements:");
                    ui.label(format!("{}", total_reqs));
                    ui.end_row();

                    ui.label("Active Requirements:");
                    ui.label(format!("{}", active_reqs));
                    ui.end_row();

                    ui.label("Archived Requirements:");
                    ui.label(format!("{}", archived_reqs));
                    ui.end_row();

                    ui.label("Total Users:");
                    ui.label(format!("{}", total_users));
                    ui.end_row();

                    ui.label("Archived Users:");
                    ui.label(format!("{}", archived_users));
                    ui.end_row();

                    ui.label("Database Path:");
                    ui.label(self.storage.path().display().to_string());
                    ui.end_row();
                });
        });
    }

    /// Show the AI settings tab with Prompts and Skills subtabs
    fn show_settings_ai_prompts_tab(&mut self, ui: &mut egui::Ui) {
        // Subtab selector
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.ai_subtab, AiSubTab::Prompts, "📝 Prompts");
            ui.selectable_value(&mut self.ai_subtab, AiSubTab::Skills, "🛠 Skills");
        });
        ui.separator();
        ui.add_space(5.0);

        match self.ai_subtab {
            AiSubTab::Prompts => self.show_ai_prompts_subtab(ui),
            AiSubTab::Skills => self.show_ai_skills_subtab(ui),
        }
    }

    /// Show the Skills subtab
    fn show_ai_skills_subtab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Claude Code Skills");
            ui.add_space(5.0);
            ui.label("Skills are prompt templates that extend Claude Code's capabilities.");
            ui.add_space(10.0);

            // Load skills from .claude/skills/ directory if not already loaded
            if self.loaded_skills.is_empty() {
                let project_dir = self.storage.path().parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let skills_dir = project_dir.join(".claude").join("skills");

                if skills_dir.exists() {
                    if let Ok(entries) = std::fs::read_dir(&skills_dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.extension().map_or(false, |ext| ext == "md") {
                                if let Ok(content) = std::fs::read_to_string(&path) {
                                    let filename = path.file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("unknown")
                                        .to_string();
                                    self.loaded_skills.push((filename, content));
                                }
                            }
                        }
                    }
                }
            }

            // Refresh button
            ui.horizontal(|ui| {
                if ui.button("🔄 Refresh Skills").clicked() {
                    self.loaded_skills.clear();
                }

                let project_dir = self.storage.path().parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let skills_dir = project_dir.join(".claude").join("skills");
                ui.label(format!("Path: {}", skills_dir.display()));
            });
            ui.add_space(10.0);

            if self.loaded_skills.is_empty() {
                ui.label("No skills found in .claude/skills/ directory.");
                ui.add_space(10.0);
                ui.label("Use the 'Scaffold Project' button in the Prompts tab to generate skills.");
            } else {
                // Display each skill
                for (filename, content) in &self.loaded_skills {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.strong(filename);
                            // Extract title from first markdown heading if present
                            if let Some(title_line) = content.lines().find(|l| l.starts_with("# ")) {
                                ui.label(format!("- {}", title_line.trim_start_matches("# ")));
                            }
                        });

                        ui.add_space(5.0);

                        // Show skill content in a collapsible section
                        egui::CollapsingHeader::new("View Content")
                            .id_salt(filename)
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt(format!("skill_content_{}", filename))
                                    .max_height(300.0)
                                    .show(ui, |ui| {
                                        // Render as markdown using egui_commonmark
                                        let mut cache = egui_commonmark::CommonMarkCache::default();
                                        egui_commonmark::CommonMarkViewer::new()
                                            .show(ui, &mut cache, content);
                                    });
                            });
                    });
                    ui.add_space(5.0);
                }
            }

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            // Link to scaffold
            ui.heading("Create New Skills");
            ui.add_space(5.0);
            ui.label("To generate skills for this project, go to the Prompts tab and click 'Scaffold Project'.");
        });
    }

    /// Show the Prompts subtab
    fn show_ai_prompts_subtab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("AI Prompt Configuration");
            ui.add_space(5.0);
            ui.label("Customize the prompts used by AI features. Changes are auto-saved.");
            ui.add_space(10.0);

            // Global Context Section
            ui.heading("Global Context");
            ui.add_space(5.0);
            ui.label("This context is included in ALL AI prompts. Use it to describe your project's domain, terminology, or special requirements.");
            ui.add_space(5.0);
            let response = ui.add(
                egui::TextEdit::multiline(&mut self.store.ai_prompts.global_context)
                    .desired_width(ui.available_width() - 20.0)
                    .desired_rows(4)
                    .hint_text("Example: This is a medical device project following FDA regulations. Requirements should be traceable to risk analyses..."),
            );
            if response.changed() {
                self.save();
            }

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            // Evaluation Prompt Section
            ui.heading("Evaluation Prompt");
            ui.add_space(5.0);
            ui.label("Customize how AI evaluates requirements for quality and completeness.");
            ui.add_space(5.0);

            ui.label("Additional Instructions:");
            let response = ui.add(
                egui::TextEdit::multiline(&mut self.store.ai_prompts.evaluation.additional_instructions)
                    .desired_width(ui.available_width() - 20.0)
                    .desired_rows(3)
                    .hint_text("Example: Requirements descriptions may be questions or ideas. Translate these into formal requirements..."),
            );
            if response.changed() {
                self.save();
            }

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            // Improve Description Prompt Section
            ui.heading("Improve Description Prompt");
            ui.add_space(5.0);
            ui.label("Customize how AI improves requirement descriptions.");
            ui.add_space(5.0);

            ui.label("Additional Instructions:");
            let response = ui.add(
                egui::TextEdit::multiline(&mut self.store.ai_prompts.improve.additional_instructions)
                    .desired_width(ui.available_width() - 20.0)
                    .desired_rows(3)
                    .hint_text("Example: Preserve technical terminology. Format acceptance criteria as bullet points..."),
            );
            if response.changed() {
                self.save();
            }

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            // Generate Children Prompt Section
            ui.heading("Generate Children Prompt");
            ui.add_space(5.0);
            ui.label("Customize how AI breaks down requirements into sub-requirements.");
            ui.add_space(5.0);

            ui.label("Additional Instructions:");
            let response = ui.add(
                egui::TextEdit::multiline(&mut self.store.ai_prompts.generate_children.additional_instructions)
                    .desired_width(ui.available_width() - 20.0)
                    .desired_rows(3)
                    .hint_text("Example: Consider regulatory compliance requirements. Separate UI, backend, and testing tasks..."),
            );
            if response.changed() {
                self.save();
            }

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            // Per-Type Customization Section
            ui.heading("Type-Specific Instructions");
            ui.add_space(5.0);
            ui.label("Add extra instructions for specific requirement types.");
            ui.add_space(10.0);

            // Show existing type prompts
            let type_names: Vec<String> = self.store.type_definitions.iter().map(|t| t.name.clone()).collect();
            let existing_types: Vec<String> = self.store.ai_prompts.type_prompts.iter().map(|t| t.type_name.clone()).collect();

            // Add button for new type prompt
            ui.horizontal(|ui| {
                ui.label("Add type-specific prompt for:");
                for type_name in &type_names {
                    if !existing_types.contains(type_name) {
                        if ui.button(type_name).clicked() {
                            self.store.ai_prompts.type_prompts.push(aida_core::AiTypePromptConfig {
                                type_name: type_name.clone(),
                                evaluation_extra: String::new(),
                                improve_extra: String::new(),
                                generate_children_extra: String::new(),
                            });
                            self.save();
                        }
                    }
                }
            });

            ui.add_space(10.0);

            // Show existing type-specific prompts
            let mut to_remove: Option<usize> = None;
            for (idx, type_prompt) in self.store.ai_prompts.type_prompts.iter_mut().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(&type_prompt.type_name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("🗑 Remove").clicked() {
                                to_remove = Some(idx);
                            }
                        });
                    });

                    ui.add_space(5.0);
                    ui.label("Evaluation extra:");
                    if ui.add(
                        egui::TextEdit::multiline(&mut type_prompt.evaluation_extra)
                            .desired_width(ui.available_width() - 20.0)
                            .desired_rows(2)
                            .hint_text("Extra instructions for evaluating this type..."),
                    ).changed() {
                        // Mark for save - handled below
                    }

                    ui.add_space(5.0);
                    ui.label("Improve extra:");
                    if ui.add(
                        egui::TextEdit::multiline(&mut type_prompt.improve_extra)
                            .desired_width(ui.available_width() - 20.0)
                            .desired_rows(2)
                            .hint_text("Extra instructions for improving this type..."),
                    ).changed() {
                        // Mark for save - handled below
                    }
                });
                ui.add_space(5.0);
            }

            // Handle removal
            if let Some(idx) = to_remove {
                self.store.ai_prompts.type_prompts.remove(idx);
                self.save();
            }

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            // Project Scaffolding Section (FR-0152)
            ui.heading("Claude Code Integration");
            ui.add_space(5.0);
            ui.label("Generate Claude Code artifacts (CLAUDE.md, skills, commands) for this project.");
            ui.add_space(10.0);

            if ui.button("🔧 Scaffold Project").clicked() {
                // Generate preview when opening dialog
                let project_dir = self.storage.path().parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let scaffolder = aida_core::Scaffolder::new(project_dir, self.scaffold_config.clone());
                self.scaffold_preview = Some(scaffolder.preview(&self.store));
                self.show_scaffold_dialog = true;
            }
            ui.label("Creates CLAUDE.md, .claude/commands/, and .claude/skills/ directories.");

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            // Template placeholders reference
            ui.heading("Template Placeholders");
            ui.add_space(5.0);
            ui.label("If using custom templates, these placeholders are available:");
            egui::Grid::new("ai_placeholders_grid")
                .num_columns(2)
                .spacing([20.0, 5.0])
                .show(ui, |ui| {
                    ui.code("{global_context}");
                    ui.label("Your global context text");
                    ui.end_row();

                    ui.code("{project_context}");
                    ui.label("Auto-generated project info");
                    ui.end_row();

                    ui.code("{req_context}");
                    ui.label("Current requirement details");
                    ui.end_row();

                    ui.code("{related_context}");
                    ui.label("Related requirements info");
                    ui.end_row();

                    ui.code("{req_type}");
                    ui.label("Current requirement type");
                    ui.end_row();
                });
        });
    }

    /// Show the theme editor dialog
    fn show_theme_editor_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_theme_editor {
            return;
        }

        // Apply the working theme so user sees changes live
        self.theme_editor_theme.apply(ctx);

        // Constrained dimensions for theme editor
        let max_size = modal_max_size(ctx);
        let theme_width = 850.0_f32.min(max_size.x);
        let theme_height = 500.0_f32.min(max_size.y);
        let content_height = (theme_height - 100.0).max(200.0); // Leave room for title/buttons

        egui::Window::new("🎨 Theme Editor")
            .collapsible(false)
            .resizable(true)
            .default_size([theme_width, theme_height])
            .max_width(max_size.x)
            .max_height(max_size.y)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 30.0]) // Offset down to avoid menu bar
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Left column: Categories (fixed width)
                    ui.allocate_ui_with_layout(
                        egui::vec2(140.0, content_height),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.heading("Categories");
                            ui.add_space(5.0);
                            for cat in ThemeEditorCategory::all() {
                                if ui
                                    .selectable_label(
                                        self.theme_editor_category == *cat,
                                        cat.label(),
                                    )
                                    .clicked()
                                {
                                    self.theme_editor_category = *cat;
                                }
                            }
                            ui.add_space(10.0);
                            ui.separator();
                            ui.add_space(5.0);

                            // Base theme selector
                            ui.label("Base Theme:");
                            egui::ComboBox::from_id_salt("theme_editor_base")
                                .selected_text(self.theme_editor_theme.base.label())
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(
                                            self.theme_editor_theme.base == BaseTheme::Dark,
                                            "Dark",
                                        )
                                        .clicked()
                                    {
                                        self.theme_editor_theme.base = BaseTheme::Dark;
                                        self.theme_editor_theme.dark_mode = true;
                                    }
                                    if ui
                                        .selectable_label(
                                            self.theme_editor_theme.base == BaseTheme::Light,
                                            "Light",
                                        )
                                        .clicked()
                                    {
                                        self.theme_editor_theme.base = BaseTheme::Light;
                                        self.theme_editor_theme.dark_mode = false;
                                    }
                                });

                            ui.add_space(10.0);
                            if ui.button("Reset to Base").clicked() {
                                let name = self.theme_editor_theme.name.clone();
                                self.theme_editor_theme =
                                    CustomTheme::from_base(self.theme_editor_theme.base, name);
                            }
                        },
                    );

                    ui.separator();

                    // Center column: Property editors (fixed width)
                    ui.allocate_ui_with_layout(
                        egui::vec2(340.0, content_height),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Theme Name:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.theme_editor_theme.name)
                                        .desired_width(200.0),
                                );
                            });
                            ui.add_space(10.0);
                            ui.separator();

                            egui::ScrollArea::vertical()
                                .id_salt("theme_editor_properties_scroll")
                                .max_height(content_height - 50.0)
                                .show(ui, |ui| {
                                    match self.theme_editor_category {
                                        ThemeEditorCategory::Backgrounds => {
                                            Self::show_theme_backgrounds(
                                                &mut self.theme_editor_theme,
                                                ui,
                                            );
                                        }
                                        ThemeEditorCategory::Text => {
                                            Self::show_theme_text(&mut self.theme_editor_theme, ui);
                                        }
                                        ThemeEditorCategory::Widgets => {
                                            Self::show_theme_widgets(
                                                &mut self.theme_editor_theme,
                                                ui,
                                            );
                                        }
                                        ThemeEditorCategory::Selection => {
                                            Self::show_theme_selection(
                                                &mut self.theme_editor_theme,
                                                ui,
                                            );
                                        }
                                        ThemeEditorCategory::Borders => {
                                            Self::show_theme_borders(
                                                &mut self.theme_editor_theme,
                                                ui,
                                            );
                                        }
                                        ThemeEditorCategory::Spacing => {
                                            Self::show_theme_spacing(
                                                &mut self.theme_editor_theme,
                                                ui,
                                            );
                                        }
                                    }
                                });
                        },
                    );

                    ui.separator();

                    // Right column: Live preview (fixed width)
                    ui.allocate_ui_with_layout(
                        egui::vec2(320.0, content_height),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.heading("Live Preview");
                            ui.add_space(5.0);

                            egui::ScrollArea::vertical()
                                .id_salt("theme_editor_preview_scroll")
                                .max_height(content_height - 30.0)
                                .show(ui, |ui| {
                                    Self::show_theme_preview(&self.theme_editor_theme, ui);
                                });
                        },
                    );
                });

                ui.add_space(10.0);
                ui.separator();

                ui.horizontal(|ui| {
                    if ui.button("✓ Apply & Close").clicked() {
                        // Save the custom theme to the list
                        let new_theme = self.theme_editor_theme.clone();
                        let theme_name = new_theme.name.clone();

                        // Check if a theme with this name already exists
                        if let Some(existing) = self
                            .user_settings
                            .custom_themes
                            .iter_mut()
                            .find(|t| t.name == theme_name)
                        {
                            // Update existing theme
                            *existing = new_theme.clone();
                        } else {
                            // Add new theme
                            self.user_settings.custom_themes.push(new_theme.clone());
                        }

                        // Set as current theme
                        self.settings_form_theme = Theme::Custom(Box::new(new_theme.clone()));
                        self.user_settings.theme = Theme::Custom(Box::new(new_theme));

                        // Save settings immediately
                        if let Err(e) = self.user_settings.save() {
                            self.message = Some((format!("Failed to save theme: {}", e), true));
                        }

                        self.show_theme_editor = false;
                    }

                    if ui.button("Export to File").on_hover_text(
                        format!("Save theme to {}", UserSettings::themes_dir().display())
                    ).clicked() {
                        match UserSettings::save_theme_to_file(&self.theme_editor_theme) {
                            Ok(path) => {
                                self.message = Some((
                                    format!("Theme exported to {}", path.display()),
                                    false
                                ));
                            }
                            Err(e) => {
                                self.message = Some((format!("Failed to export theme: {}", e), true));
                            }
                        }
                    }

                    if ui.button("Cancel").clicked() {
                        // Revert to the original theme before editing
                        self.settings_form_theme = self.theme_editor_original_theme.clone();
                        self.user_settings.theme = self.theme_editor_original_theme.clone();
                        self.show_theme_editor = false;
                    }
                });
            });
    }

    /// Show background color editors
    fn show_theme_backgrounds(theme: &mut CustomTheme, ui: &mut egui::Ui) {
        ui.heading("Background Colors");
        ui.add_space(10.0);

        egui::Grid::new("theme_backgrounds_grid")
            .num_columns(2)
            .spacing([20.0, 8.0])
            .show(ui, |ui| {
                ui.label("Window Fill:");
                color_picker_widget(ui, &mut theme.window_fill);
                ui.end_row();

                ui.label("Panel Fill:");
                color_picker_widget(ui, &mut theme.panel_fill);
                ui.end_row();

                ui.label("Extreme Background:");
                ui.label("(text edit backgrounds)");
                ui.end_row();
                ui.label("");
                color_picker_widget(ui, &mut theme.extreme_bg);
                ui.end_row();

                ui.label("Faint Background:");
                ui.label("(subtle separators)");
                ui.end_row();
                ui.label("");
                color_picker_widget(ui, &mut theme.faint_bg);
                ui.end_row();
            });
    }

    /// Show text color editors
    fn show_theme_text(theme: &mut CustomTheme, ui: &mut egui::Ui) {
        ui.heading("Text Colors");
        ui.add_space(10.0);

        egui::Grid::new("theme_text_grid")
            .num_columns(2)
            .spacing([20.0, 8.0])
            .show(ui, |ui| {
                ui.label("Override Text Color:");
                let mut has_override = theme.text_color.is_some();
                if ui.checkbox(&mut has_override, "Custom text color").changed() {
                    if has_override {
                        theme.text_color = Some(ThemeColor::new(200, 200, 200));
                    } else {
                        theme.text_color = None;
                    }
                }
                ui.end_row();

                if let Some(ref mut color) = theme.text_color {
                    ui.label("");
                    color_picker_widget(ui, color);
                    ui.end_row();
                }

                ui.label("Hyperlink Color:");
                color_picker_widget(ui, &mut theme.hyperlink_color);
                ui.end_row();

                ui.label("Warning Color:");
                color_picker_widget(ui, &mut theme.warn_fg);
                ui.end_row();

                ui.label("Error Color:");
                color_picker_widget(ui, &mut theme.error_fg);
                ui.end_row();
            });
    }

    /// Show widget color editors
    fn show_theme_widgets(theme: &mut CustomTheme, ui: &mut egui::Ui) {
        ui.heading("Widget Colors");
        ui.add_space(10.0);

        ui.label("Non-Interactive Widgets:");
        ui.add_space(5.0);
        egui::Grid::new("theme_widgets_noninteractive")
            .num_columns(2)
            .spacing([20.0, 8.0])
            .show(ui, |ui| {
                ui.label("Background:");
                color_picker_widget(ui, &mut theme.widget_bg);
                ui.end_row();

                ui.label("Foreground:");
                color_picker_widget(ui, &mut theme.widget_fg);
                ui.end_row();
            });

        ui.add_space(15.0);
        ui.label("Interactive Widget States:");
        ui.add_space(5.0);
        egui::Grid::new("theme_widgets_interactive")
            .num_columns(2)
            .spacing([20.0, 8.0])
            .show(ui, |ui| {
                ui.label("Inactive Background:");
                color_picker_widget(ui, &mut theme.widget_inactive_bg);
                ui.end_row();

                ui.label("Hovered Background:");
                color_picker_widget(ui, &mut theme.widget_hovered_bg);
                ui.end_row();

                ui.label("Active Background:");
                color_picker_widget(ui, &mut theme.widget_active_bg);
                ui.end_row();

                ui.label("Open Background:");
                color_picker_widget(ui, &mut theme.widget_open_bg);
                ui.end_row();
            });
    }

    /// Show selection color editors
    fn show_theme_selection(theme: &mut CustomTheme, ui: &mut egui::Ui) {
        ui.heading("Selection Colors");
        ui.add_space(10.0);

        egui::Grid::new("theme_selection_grid")
            .num_columns(2)
            .spacing([20.0, 8.0])
            .show(ui, |ui| {
                ui.label("Selection Background:");
                color_picker_widget(ui, &mut theme.selection_bg);
                ui.end_row();

                ui.label("Selection Foreground:");
                color_picker_widget(ui, &mut theme.selection_fg);
                ui.end_row();
            });
    }

    /// Show border and rounding editors
    fn show_theme_borders(theme: &mut CustomTheme, ui: &mut egui::Ui) {
        ui.heading("Borders & Rounding");
        ui.add_space(10.0);

        ui.label("Border Colors:");
        ui.add_space(5.0);
        egui::Grid::new("theme_borders_colors")
            .num_columns(2)
            .spacing([20.0, 8.0])
            .show(ui, |ui| {
                ui.label("Inactive Stroke:");
                color_picker_widget(ui, &mut theme.widget_stroke_color);
                ui.end_row();

                ui.label("Hovered Stroke:");
                color_picker_widget(ui, &mut theme.widget_hovered_stroke_color);
                ui.end_row();

                ui.label("Active Stroke:");
                color_picker_widget(ui, &mut theme.widget_active_stroke_color);
                ui.end_row();
            });

        ui.add_space(15.0);
        ui.label("Stroke Width & Rounding:");
        ui.add_space(5.0);
        egui::Grid::new("theme_borders_sizing")
            .num_columns(2)
            .spacing([20.0, 8.0])
            .show(ui, |ui| {
                ui.label("Stroke Width:");
                ui.add(
                    egui::Slider::new(&mut theme.widget_stroke_width, 0.0..=5.0)
                        .suffix("px"),
                );
                ui.end_row();

                ui.label("Widget Rounding:");
                ui.add(
                    egui::Slider::new(&mut theme.widget_rounding, 0.0..=20.0)
                        .suffix("px"),
                );
                ui.end_row();

                ui.label("Window Rounding:");
                ui.add(
                    egui::Slider::new(&mut theme.window_rounding, 0.0..=20.0)
                        .suffix("px"),
                );
                ui.end_row();
            });

        ui.add_space(15.0);
        ui.label("Shadows:");
        ui.add_space(5.0);
        ui.checkbox(&mut theme.window_shadow, "Window Shadow");
        ui.checkbox(&mut theme.popup_shadow, "Popup Shadow");
    }

    /// Show spacing and layout editors
    fn show_theme_spacing(theme: &mut CustomTheme, ui: &mut egui::Ui) {
        ui.heading("Spacing & Layout");
        ui.add_space(10.0);

        egui::Grid::new("theme_spacing_grid")
            .num_columns(2)
            .spacing([20.0, 8.0])
            .show(ui, |ui| {
                ui.label("Item Spacing (H, V):");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut theme.item_spacing.0).speed(0.5));
                    ui.add(egui::DragValue::new(&mut theme.item_spacing.1).speed(0.5));
                });
                ui.end_row();

                ui.label("Button Padding (H, V):");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut theme.button_padding.0).speed(0.5));
                    ui.add(egui::DragValue::new(&mut theme.button_padding.1).speed(0.5));
                });
                ui.end_row();

                ui.label("Window Padding:");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut theme.window_padding.0).speed(0.5));
                    ui.add(egui::DragValue::new(&mut theme.window_padding.1).speed(0.5));
                });
                ui.end_row();

                ui.label("Scroll Bar Width:");
                ui.add(
                    egui::Slider::new(&mut theme.scroll_bar_width, 4.0..=20.0)
                        .suffix("px"),
                );
                ui.end_row();

                ui.label("Indent:");
                ui.add(
                    egui::Slider::new(&mut theme.indent, 8.0..=40.0)
                        .suffix("px"),
                );
                ui.end_row();
            });
    }


    /// Show a live preview of the theme with AIDA-specific UI examples
    fn show_theme_preview(theme: &CustomTheme, ui: &mut egui::Ui) {
        ui.set_min_width(280.0);

        // === Requirements List Preview ===
        ui.strong("Requirements List");
        ui.add_space(3.0);

        // Simulated requirement list items
        let list_bg = theme.extreme_bg.to_egui();
        egui::Frame::none()
            .fill(list_bg)
            .inner_margin(4.0)
            .rounding(2.0)
            .show(ui, |ui| {
                // Selected item
                let selected_rect = ui.available_rect_before_wrap();
                let selected_rect = egui::Rect::from_min_size(
                    selected_rect.min,
                    egui::vec2(ui.available_width(), 20.0),
                );
                ui.painter()
                    .rect_filled(selected_rect, 2.0, theme.selection_bg.to_egui());
                ui.horizontal(|ui| {
                    ui.colored_label(theme.selection_fg.to_egui(), "FR-001");
                    ui.colored_label(theme.selection_fg.to_egui(), "User Authentication");
                });

                // Normal items
                ui.horizontal(|ui| {
                    ui.label("FR-002");
                    ui.label("Data Export Feature");
                });
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::GRAY, "FR-003");
                    ui.colored_label(egui::Color32::GRAY, "(filtered parent)");
                });
                ui.horizontal(|ui| {
                    ui.label("NFR-001");
                    ui.label("Performance Target");
                });
            });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(5.0);

        // === Detail View Preview ===
        ui.strong("Detail View");
        ui.add_space(3.0);

        egui::Frame::none()
            .fill(theme.panel_fill.to_egui())
            .inner_margin(6.0)
            .rounding(4.0)
            .stroke(egui::Stroke::new(1.0, theme.widget_stroke_color.to_egui()))
            .show(ui, |ui| {
                ui.heading("FR-001: User Authentication");
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label("Status:");
                    ui.colored_label(egui::Color32::from_rgb(76, 175, 80), "Approved");
                    ui.label("Priority:");
                    ui.colored_label(egui::Color32::from_rgb(244, 67, 54), "High");
                });
                ui.add_space(3.0);
                ui.label("Users must authenticate before accessing the system.");
            });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(5.0);

        // === Form Preview ===
        ui.strong("Add/Edit Form");
        ui.add_space(3.0);

        egui::Grid::new("preview_form_grid")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Title:");
                let mut title = "New Requirement".to_string();
                ui.add(egui::TextEdit::singleline(&mut title).desired_width(150.0));
                ui.end_row();

                ui.label("Type:");
                let mut type_sel = "Functional";
                egui::ComboBox::from_id_salt("preview_type")
                    .selected_text(type_sel)
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut type_sel, "Functional", "Functional");
                        ui.selectable_value(&mut type_sel, "Non-Functional", "Non-Functional");
                    });
                ui.end_row();

                ui.label("Status:");
                let mut status_sel = "Draft";
                egui::ComboBox::from_id_salt("preview_status")
                    .selected_text(status_sel)
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut status_sel, "Draft", "Draft");
                        ui.selectable_value(&mut status_sel, "Approved", "Approved");
                    });
                ui.end_row();
            });

        ui.add_space(5.0);
        ui.horizontal(|ui| {
            let _ = ui.button("Save");
            let _ = ui.button("Cancel");
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(5.0);

        // === Messages Preview ===
        ui.strong("Messages");
        ui.add_space(3.0);
        ui.colored_label(egui::Color32::from_rgb(76, 175, 80), "Saved successfully");
        ui.colored_label(theme.warn_fg.to_egui(), "Warning: Unsaved changes");
        ui.colored_label(theme.error_fg.to_egui(), "Error: Failed to load");
    }

    fn show_users_table(&mut self, ui: &mut egui::Ui) {
        // Collect user data to avoid borrow issues
        let users_data: Vec<(Uuid, Option<String>, String, String, String, bool)> = self
            .store
            .users
            .iter()
            .filter(|u| self.show_archived_users || !u.archived)
            .map(|u| {
                (
                    u.id,
                    u.spec_id.clone(),
                    u.name.clone(),
                    u.email.clone(),
                    u.handle.clone(),
                    u.archived,
                )
            })
            .collect();

        if users_data.is_empty() {
            ui.label("No users defined.");
            return;
        }

        egui::Grid::new("users_table")
            .num_columns(6)
            .striped(true)
            .spacing([10.0, 5.0])
            .show(ui, |ui| {
                // Header
                ui.strong("ID");
                ui.strong("Name");
                ui.strong("Email");
                ui.strong("Handle");
                ui.strong("Status");
                ui.strong("Actions");
                ui.end_row();

                for (id, spec_id, name, email, handle, archived) in &users_data {
                    // Show spec_id with special styling
                    if let Some(sid) = spec_id {
                        ui.colored_label(egui::Color32::from_rgb(74, 158, 255), sid);
                    } else {
                        ui.label("-");
                    }
                    ui.label(name);
                    ui.label(email);
                    ui.label(format!("@{}", handle));
                    ui.label(if *archived { "Archived" } else { "Active" });

                    ui.horizontal(|ui| {
                        if ui.small_button("✏").on_hover_text("Edit").clicked() {
                            self.editing_user_id = Some(*id);
                            self.user_form_name = name.clone();
                            self.user_form_email = email.clone();
                            self.user_form_handle = handle.clone();
                            self.show_user_form = true;
                        }

                        let archive_label = if *archived { "Unarchive" } else { "Archive" };
                        if ui
                            .small_button(if *archived { "↩" } else { "📁" })
                            .on_hover_text(archive_label)
                            .clicked()
                        {
                            if let Some(user) = self.store.get_user_by_id_mut(id) {
                                user.archived = !user.archived;
                            }
                            self.save();
                        }

                        if ui.small_button("🗑").on_hover_text("Delete").clicked() {
                            self.store.remove_user(id);
                            self.save();
                        }
                    });
                    ui.end_row();
                }
            });
    }

    fn add_new_user(&mut self) {
        if self.user_form_name.is_empty() {
            self.message = Some(("User name is required".to_string(), true));
            return;
        }

        // Use add_user_with_id to auto-generate $USER-XXX spec_id
        let spec_id = self.store.add_user_with_id(
            self.user_form_name.clone(),
            self.user_form_email.clone(),
            self.user_form_handle.clone(),
        );
        self.save();

        self.show_user_form = false;
        self.user_form_name.clear();
        self.user_form_email.clear();
        self.user_form_handle.clear();
        self.message = Some((format!("User {} added successfully", spec_id), false));
    }

    fn save_edited_user(&mut self) {
        if let Some(user_id) = self.editing_user_id {
            if let Some(user) = self.store.get_user_by_id_mut(&user_id) {
                user.name = self.user_form_name.clone();
                user.email = self.user_form_email.clone();
                user.handle = self.user_form_handle.clone();
            }
            self.save();

            self.show_user_form = false;
            self.editing_user_id = None;
            self.user_form_name.clear();
            self.user_form_email.clear();
            self.user_form_handle.clear();
            self.message = Some(("User updated successfully".to_string(), false));
        }
    }

    fn backup_database(&mut self) {
        use chrono::Local;

        let db_path = self.storage.path();
        let timestamp = Local::now().format("%Y%m%d_%H%M%S");

        // Create backup filename with timestamp
        let backup_name = if let Some(stem) = db_path.file_stem() {
            let ext = db_path
                .extension()
                .map(|e| e.to_str().unwrap_or("yaml"))
                .unwrap_or("yaml");
            format!(
                "{}_{}.{}",
                stem.to_str().unwrap_or("requirements"),
                timestamp,
                ext
            )
        } else {
            format!("requirements_backup_{}.yaml", timestamp)
        };

        let backup_path = db_path
            .parent()
            .map(|p| p.join(&backup_name))
            .unwrap_or_else(|| std::path::PathBuf::from(&backup_name));

        match std::fs::copy(db_path, &backup_path) {
            Ok(_) => {
                self.message = Some((format!("Backup created: {}", backup_name), false));
            }
            Err(e) => {
                self.message = Some((format!("Backup failed: {}", e), true));
            }
        }
    }

    /// Get indices of requirements that pass the current filters (in display order)
    /// For flat view, returns in storage order. For tree views, returns in tree traversal order.
    /// When there's an active search, always uses flat list behavior to match what's displayed.
    fn get_filtered_indices(&self) -> Vec<usize> {
        // When searching, use flat list behavior (matches show_tree_list which switches to flat list)
        let has_search_text = !self.filter_text.is_empty() && !self.search_scope.is_none();

        if has_search_text || self.perspective == Perspective::Flat {
            // Flat view or search active: simple filtered list in storage order
            self.store
                .requirements
                .iter()
                .enumerate()
                .filter(|(_, req)| self.passes_filters(req, true))
                .map(|(idx, _)| idx)
                .collect()
        } else {
            // Tree view without search: traverse in display order
            let Some((outgoing_type, _)) = self.perspective.relationship_types() else {
                return Vec::new();
            };

            let mut result = Vec::new();
            match self.perspective_direction {
                PerspectiveDirection::TopDown => {
                    let leaves = self.find_tree_leaves(&outgoing_type);
                    for leaf_idx in leaves {
                        self.collect_tree_indices_bottom_up(
                            leaf_idx,
                            &outgoing_type,
                            &mut result,
                        );
                    }
                }
                PerspectiveDirection::BottomUp => {
                    let roots = self.find_tree_roots(&outgoing_type);
                    for root_idx in roots {
                        self.collect_tree_indices_top_down(
                            root_idx,
                            &outgoing_type,
                            &mut result,
                        );
                    }
                }
            }
            result
        }
    }

    /// Get indices of requirements for the split list (List 2) that pass the split filters
    fn get_split_filtered_indices(&self) -> Vec<usize> {
        let filter_text = self.split_filter_text.to_lowercase();
        let has_search_text = !filter_text.is_empty();

        self.store
            .requirements
            .iter()
            .enumerate()
            .filter(|(_, req)| {
                // Skip archived
                if req.archived && !self.show_archived {
                    return false;
                }

                // When searching, bypass other filters and search all requirements
                if has_search_text {
                    let matches = req.title.to_lowercase().contains(&filter_text)
                        || req.description.to_lowercase().contains(&filter_text)
                        || req.spec_id.as_ref().map_or(false, |id| id.to_lowercase().contains(&filter_text));
                    return matches;
                }

                // No search text - apply all filters normally
                // Type filter
                if !self.split_filter_types.is_empty()
                    && !self.split_filter_types.contains(&req.req_type)
                {
                    return false;
                }
                // Feature filter
                if !self.split_filter_features.is_empty()
                    && !self.split_filter_features.contains(&req.feature)
                {
                    return false;
                }
                // Prefix filter
                if !self.split_filter_prefixes.is_empty() {
                    let req_prefix = req
                        .spec_id
                        .as_ref()
                        .and_then(|s| s.split('-').next())
                        .unwrap_or("");
                    if !self.split_filter_prefixes.contains(req_prefix) {
                        return false;
                    }
                }
                // Status filter
                if !self.split_filter_statuses.is_empty()
                    && !self.split_filter_statuses.contains(&req.status)
                {
                    return false;
                }
                // Priority filter
                if !self.split_filter_priorities.is_empty()
                    && !self.split_filter_priorities.contains(&req.priority)
                {
                    return false;
                }
                true
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Collect tree indices in top-down order (roots first, then children)
    fn collect_tree_indices_top_down(
        &self,
        idx: usize,
        outgoing_rel_type: &RelationshipType,
        result: &mut Vec<usize>,
    ) {
        // Don't add duplicates
        if result.contains(&idx) {
            return;
        }

        // Check if this node is collapsed
        let is_collapsed = self
            .store
            .requirements
            .get(idx)
            .map(|req| self.tree_collapsed.get(&req.id).copied().unwrap_or(false))
            .unwrap_or(false);

        result.push(idx);

        // If not collapsed, add children recursively
        if !is_collapsed {
            if let Some(req) = self.store.requirements.get(idx) {
                let children = self.get_children(&req.id, outgoing_rel_type);
                for child_idx in children {
                    self.collect_tree_indices_top_down(child_idx, outgoing_rel_type, result);
                }
            }
        }
    }

    /// Collect tree indices in bottom-up order (leaves first, then parents)
    fn collect_tree_indices_bottom_up(
        &self,
        idx: usize,
        outgoing_rel_type: &RelationshipType,
        result: &mut Vec<usize>,
    ) {
        // Don't add duplicates
        if result.contains(&idx) {
            return;
        }

        // Check if this node is collapsed
        let is_collapsed = self
            .store
            .requirements
            .get(idx)
            .map(|req| self.tree_collapsed.get(&req.id).copied().unwrap_or(false))
            .unwrap_or(false);

        result.push(idx);

        // If not collapsed, add parents recursively
        if !is_collapsed {
            if let Some(req) = self.store.requirements.get(idx) {
                let parents = self.get_parents(&req.id, outgoing_rel_type);
                for parent_idx in parents {
                    self.collect_tree_indices_bottom_up(parent_idx, outgoing_rel_type, result);
                }
            }
        }
    }

    /// Search comments recursively for a search term
    fn search_comments_recursive(&self, comments: &[Comment], search: &str) -> bool {
        for comment in comments {
            // Check comment content
            if comment.content.to_lowercase().contains(search) {
                return true;
            }
            // Check author name
            if comment.author.to_lowercase().contains(search) {
                return true;
            }
            // Recursively check replies
            if self.search_comments_recursive(&comment.replies, search) {
                return true;
            }
        }
        false
    }

    /// Check if a requirement matches the current search text
    fn matches_search(&self, req: &Requirement) -> bool {
        if self.filter_text.is_empty() || self.search_scope.is_none() {
            return false;
        }

        let search = self.filter_text.to_lowercase();

        // Check title if enabled
        if self.search_scope.title && req.title.to_lowercase().contains(&search) {
            return true;
        }

        // Check description if enabled
        if self.search_scope.description && req.description.to_lowercase().contains(&search) {
            return true;
        }

        // Check spec_id if enabled
        if self.search_scope.spec_id {
            if let Some(ref spec_id) = req.spec_id {
                if spec_id.to_lowercase().contains(&search) {
                    return true;
                }
            }
        }

        // Check comments if enabled
        if self.search_scope.comments {
            if self.search_comments_recursive(&req.comments, &search) {
                return true;
            }
        }

        false
    }

    /// Update search match indices based on current search text
    fn update_search_matches(&mut self) {
        self.search_match_indices.clear();
        self.search_current_match = None;

        if self.filter_text.is_empty() || self.search_scope.is_none() {
            return;
        }

        // Collect indices of all matching requirements
        for (idx, req) in self.store.requirements.iter().enumerate() {
            // Skip archived if not showing them
            if req.archived && !self.show_archived {
                continue;
            }
            if self.matches_search(req) {
                self.search_match_indices.push(idx);
            }
        }

        // Jump to first match if any
        if !self.search_match_indices.is_empty() {
            self.search_current_match = Some(0);
            self.selected_idx = Some(self.search_match_indices[0]);
            self.scroll_to_requirement = Some(self.store.requirements[self.search_match_indices[0]].id);
        }
    }

    /// Navigate to the next search match
    fn next_search_match(&mut self) {
        if self.search_match_indices.is_empty() {
            return;
        }

        let wrap = self.user_settings.wrap_search;
        let current = self.search_current_match.unwrap_or(0);
        let next = if current + 1 < self.search_match_indices.len() {
            current + 1
        } else if wrap {
            0 // Wrap to beginning
        } else {
            current // Stay at last match
        };

        self.search_current_match = Some(next);
        let req_idx = self.search_match_indices[next];
        self.selected_idx = Some(req_idx);
        self.scroll_to_requirement = Some(self.store.requirements[req_idx].id);
    }

    /// Navigate to the previous search match
    fn prev_search_match(&mut self) {
        if self.search_match_indices.is_empty() {
            return;
        }

        let wrap = self.user_settings.wrap_search;
        let current = self.search_current_match.unwrap_or(0);
        let prev = if current > 0 {
            current - 1
        } else if wrap {
            self.search_match_indices.len() - 1 // Wrap to end
        } else {
            0 // Stay at first match
        };

        self.search_current_match = Some(prev);
        let req_idx = self.search_match_indices[prev];
        self.selected_idx = Some(req_idx);
        self.scroll_to_requirement = Some(self.store.requirements[req_idx].id);
    }

    /// Clear search and return to normal view
    fn clear_search(&mut self) {
        self.filter_text.clear();
        self.search_match_indices.clear();
        self.search_current_match = None;
    }

    /// Check if a requirement at the given index is the current search match
    fn is_current_search_match(&self, idx: usize) -> bool {
        if let Some(current) = self.search_current_match {
            if let Some(&match_idx) = self.search_match_indices.get(current) {
                return match_idx == idx;
            }
        }
        false
    }

    /// Check if a requirement at the given index is any search match
    fn is_search_match(&self, idx: usize) -> bool {
        self.search_match_indices.contains(&idx)
    }

    /// Check if a requirement passes the current filters
    /// `is_root` indicates whether this is a root-level requirement (true) or a child (false)
    fn passes_filters(&self, req: &Requirement, is_root: bool) -> bool {
        // Text search filter - when active, search across ALL requirements (ignoring other filters)
        // This allows users to find requirements regardless of current filter state
        let has_search_text = !self.filter_text.is_empty() && !self.search_scope.is_none();

        if has_search_text {
            // In Highlight mode, show all requirements (just highlight matches)
            // In Filter mode, only show matching requirements
            if self.user_settings.search_mode == SearchMode::Highlight {
                // Still respect archive filter even in highlight mode
                if req.archived && !self.show_archived {
                    return false;
                }
                return true; // Show all requirements in highlight mode
            }

            // Filter mode: only show matching requirements
            let search = self.filter_text.to_lowercase();
            let mut found = false;

            // Check title if enabled
            if self.search_scope.title && req.title.to_lowercase().contains(&search) {
                found = true;
            }

            // Check description if enabled
            if !found
                && self.search_scope.description
                && req.description.to_lowercase().contains(&search)
            {
                found = true;
            }

            // Check spec_id if enabled
            if !found && self.search_scope.spec_id {
                if let Some(ref spec_id) = req.spec_id {
                    if spec_id.to_lowercase().contains(&search) {
                        found = true;
                    }
                }
            }

            // Check comments if enabled
            if !found && self.search_scope.comments {
                found = self.search_comments_recursive(&req.comments, &search);
            }

            // When searching, only apply archive filter (not type/feature/status/priority filters)
            // This lets users search across all requirements regardless of filter state
            if !found {
                return false;
            }

            // Still respect archive filter even when searching
            if req.archived && !self.show_archived {
                return false;
            }

            return true;
        }

        // No search text - apply all filters normally
        // Determine which filters to use based on root vs child
        let (filter_types, filter_features, filter_prefixes, filter_statuses, filter_priorities) =
            if is_root || self.children_same_as_root {
                // Root requirements or "same as root" mode: use root filters
                (
                    &self.filter_types,
                    &self.filter_features,
                    &self.filter_prefixes,
                    &self.filter_statuses,
                    &self.filter_priorities,
                )
            } else {
                // Child requirements with separate filters
                (
                    &self.child_filter_types,
                    &self.child_filter_features,
                    &self.child_filter_prefixes,
                    &self.child_filter_statuses,
                    &self.child_filter_priorities,
                )
            };

        // Type filter (empty = show all)
        if !filter_types.is_empty() && !filter_types.contains(&req.req_type) {
            return false;
        }

        // Feature filter (empty = show all)
        if !filter_features.is_empty() && !filter_features.contains(&req.feature) {
            return false;
        }

        // Prefix filter (empty = show all)
        if !filter_prefixes.is_empty() {
            // Extract prefix from spec_id (e.g., "SEC-001" -> "SEC")
            let req_prefix = req
                .spec_id
                .as_ref()
                .and_then(|s| s.split('-').next())
                .unwrap_or("");
            if !filter_prefixes.contains(req_prefix) {
                return false;
            }
        }

        // Status filter (empty = show all)
        if !filter_statuses.is_empty() && !filter_statuses.contains(&req.status) {
            return false;
        }

        // Priority filter (empty = show all)
        if !filter_priorities.is_empty() && !filter_priorities.contains(&req.priority) {
            return false;
        }

        // Archive filter (hide archived unless show_archived is true)
        if req.archived && !self.show_archived {
            return false;
        }

        true
    }

    /// Check if a requirement or any of its descendants (via the given relationship type) matches filters
    /// This is used to determine if non-matching ancestors should be shown (greyed out)
    fn has_matching_descendant(
        &self,
        req_id: &Uuid,
        outgoing_rel_type: &RelationshipType,
        visited: &mut HashSet<Uuid>,
    ) -> bool {
        // Prevent infinite recursion
        if visited.contains(req_id) {
            return false;
        }
        visited.insert(*req_id);

        // Find all children via the outgoing relationship type
        // In Parent/Child, outgoing is "Parent" - so we find requirements that have Parent pointing to us
        // That means we are their parent, they are our children
        for req in &self.store.requirements {
            for rel in &req.relationships {
                if &rel.rel_type == outgoing_rel_type && &rel.target_id == req_id {
                    // req has a Parent relationship pointing to req_id
                    // So req is a child of req_id
                    // Check if this child matches filters
                    if self.passes_filters(req, false) {
                        return true;
                    }
                    // Recursively check this child's descendants
                    if self.has_matching_descendant(&req.id, outgoing_rel_type, visited) {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Compute all ancestor requirement IDs that should be shown (greyed out) because they have matching descendants
    fn compute_ancestor_ids_to_show(&self, outgoing_rel_type: &RelationshipType) -> HashSet<Uuid> {
        let mut ancestors_to_show = HashSet::new();
        let mut visited = HashSet::new();

        // Find all requirements that match filters
        let matching_reqs: Vec<Uuid> = self
            .store
            .requirements
            .iter()
            .filter(|req| self.passes_filters(req, false)) // Check with is_root=false for general match
            .map(|req| req.id)
            .collect();

        // For each matching requirement, trace back its ancestors
        for req_id in &matching_reqs {
            self.collect_ancestors(
                req_id,
                outgoing_rel_type,
                &mut ancestors_to_show,
                &mut visited,
            );
        }

        // Remove the matching requirements themselves (they're not "ancestors to grey out")
        for req_id in &matching_reqs {
            ancestors_to_show.remove(req_id);
        }

        ancestors_to_show
    }

    /// Recursively collect all ancestors of a requirement
    fn collect_ancestors(
        &self,
        req_id: &Uuid,
        outgoing_rel_type: &RelationshipType,
        ancestors: &mut HashSet<Uuid>,
        visited: &mut HashSet<Uuid>,
    ) {
        if visited.contains(req_id) {
            return;
        }
        visited.insert(*req_id);

        // Find the requirement
        let Some(req) = self.store.requirements.iter().find(|r| &r.id == req_id) else {
            return;
        };

        // Find parents: requirements that this one points to via the outgoing relationship
        // In Parent/Child, outgoing is "Parent", so req.relationships with type Parent point to parents
        for rel in &req.relationships {
            if &rel.rel_type == outgoing_rel_type {
                // rel.target_id is a parent
                ancestors.insert(rel.target_id);
                self.collect_ancestors(&rel.target_id, outgoing_rel_type, ancestors, visited);
            }
        }
    }

    /// Get all unique feature names from requirements
    fn get_all_features(&self) -> Vec<String> {
        let mut features: Vec<String> = self
            .store
            .requirements
            .iter()
            .map(|r| r.feature.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        features.sort();
        features
    }

    /// Find root nodes for tree view (requirements that are not children of any other requirement)
    /// For Parent/Child: roots are requirements that no one has a Parent relationship pointing to
    /// If `ancestors_to_show` is provided, also include roots that are in that set (shown greyed out)
    fn find_tree_roots_with_ancestors(
        &self,
        outgoing_rel_type: &RelationshipType,
        ancestors_to_show: &HashSet<Uuid>,
    ) -> Vec<usize> {
        // Collect all requirement IDs that are targets of the outgoing relationship type
        // These are the "children" - the ones that parents point to
        let mut is_child: HashSet<Uuid> = HashSet::new();

        for req in &self.store.requirements {
            for rel in &req.relationships {
                if &rel.rel_type == outgoing_rel_type {
                    // This requirement has a Parent/outgoing relationship to target
                    // So target is a child
                    is_child.insert(rel.target_id);
                }
            }
        }

        // Return indices of requirements that are NOT children (i.e., they are roots)
        // Include if: passes filters OR is an ancestor of something that passes filters
        self.store
            .requirements
            .iter()
            .enumerate()
            .filter(|(_, req)| {
                !is_child.contains(&req.id)
                    && (self.passes_filters(req, true) || ancestors_to_show.contains(&req.id))
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Find root nodes for tree view (requirements that are not children of any other requirement)
    /// For Parent/Child: roots are requirements that no one has a Parent relationship pointing to
    fn find_tree_roots(&self, outgoing_rel_type: &RelationshipType) -> Vec<usize> {
        self.find_tree_roots_with_ancestors(outgoing_rel_type, &HashSet::new())
    }

    /// Get children of a requirement for a given relationship type
    /// If `ancestors_to_show` is provided, also include children that are in that set (shown greyed out)
    fn get_children_with_ancestors(
        &self,
        parent_id: &Uuid,
        outgoing_rel_type: &RelationshipType,
        ancestors_to_show: &HashSet<Uuid>,
    ) -> Vec<usize> {
        // Find the parent requirement
        if let Some(parent) = self.store.requirements.iter().find(|r| &r.id == parent_id) {
            // Get all target IDs where relationship type matches
            let child_ids: Vec<Uuid> = parent
                .relationships
                .iter()
                .filter(|r| &r.rel_type == outgoing_rel_type)
                .map(|r| r.target_id)
                .collect();

            // Convert to indices
            // Include if: passes filters OR is an ancestor of something that passes filters
            self.store
                .requirements
                .iter()
                .enumerate()
                .filter(|(_, req)| {
                    child_ids.contains(&req.id)
                        && (self.passes_filters(req, false) || ancestors_to_show.contains(&req.id))
                })
                .map(|(idx, _)| idx)
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get children of a requirement for a given relationship type
    fn get_children(&self, parent_id: &Uuid, outgoing_rel_type: &RelationshipType) -> Vec<usize> {
        self.get_children_with_ancestors(parent_id, outgoing_rel_type, &HashSet::new())
    }

    /// Get parents of a requirement for a given relationship type (for bottom-up view)
    /// Finds requirements that have an outgoing relationship (e.g., Parent) pointing to this child
    fn get_parents(&self, child_id: &Uuid, outgoing_rel_type: &RelationshipType) -> Vec<usize> {
        // Find all requirements that have an outgoing relationship to this child
        // e.g., find all requirements with a "Parent" relationship where target_id == child_id
        // In bottom-up view, parents are shown nested under children, so they're "children" in display terms
        self.store
            .requirements
            .iter()
            .enumerate()
            .filter(|(_, req)| {
                self.passes_filters(req, false)
                    && req
                        .relationships
                        .iter()
                        .any(|r| &r.rel_type == outgoing_rel_type && &r.target_id == child_id)
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Find leaf nodes for bottom-up tree view (requirements with no outgoing relationships of the type)
    /// These are displayed at root level in bottom-up view
    fn find_tree_leaves(&self, outgoing_rel_type: &RelationshipType) -> Vec<usize> {
        self.store
            .requirements
            .iter()
            .enumerate()
            .filter(|(_, req)| {
                self.passes_filters(req, true)
                    && !req
                        .relationships
                        .iter()
                        .any(|r| &r.rel_type == outgoing_rel_type)
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Create a relationship based on drag-drop action and current perspective
    /// Returns (source_idx, target_idx, relationship_type) where source stores the relationship to target
    fn get_relationship_for_drop(
        &self,
        dragged_idx: usize,
        drop_target_idx: usize,
    ) -> Option<(usize, usize, RelationshipType)> {
        if dragged_idx == drop_target_idx {
            return None; // Can't create relationship to self
        }

        let (outgoing_type, _incoming_type) = self.perspective.relationship_types()?;

        // In Parent/Child perspective:
        // - outgoing_type is Parent (stored on the child, pointing to the parent)
        // - When dragging in top-down: drop target becomes parent of dragged item
        //   So dragged (child) gets a Parent relationship pointing to drop_target (parent)
        // - When dragging in bottom-up: dragged item becomes parent of drop target
        //   So drop_target (child) gets a Parent relationship pointing to dragged (parent)
        match self.perspective_direction {
            PerspectiveDirection::TopDown => {
                // Drop target becomes parent of dragged
                // Dragged (child) stores Parent relationship pointing to drop_target (parent)
                Some((dragged_idx, drop_target_idx, outgoing_type))
            }
            PerspectiveDirection::BottomUp => {
                // Dragged becomes parent of drop_target
                // drop_target (child) stores Parent relationship pointing to dragged (parent)
                Some((drop_target_idx, dragged_idx, outgoing_type))
            }
        }
    }

    /// Create a relationship between two requirements
    fn create_relationship_from_drop(&mut self, dragged_idx: usize, drop_target_idx: usize) {
        if let Some((source_idx, target_idx, rel_type)) =
            self.get_relationship_for_drop(dragged_idx, drop_target_idx)
        {
            let source_id = self.store.requirements.get(source_idx).map(|r| r.id);
            let target_id = self.store.requirements.get(target_idx).map(|r| r.id);

            if let (Some(source_id), Some(target_id)) = (source_id, target_id) {
                // Validate the relationship first
                let validation = self
                    .store
                    .validate_relationship(&source_id, &rel_type, &target_id);

                // Check for errors
                if !validation.valid {
                    let error_msg = validation.errors.join("; ");
                    self.message =
                        Some((format!("Cannot create relationship: {}", error_msg), true));
                    return;
                }

                // Show warnings but proceed
                if !validation.warnings.is_empty() {
                    // We'll show the warning along with success
                    let _warning_msg = validation.warnings.join("; ");
                }

                // Set the relationship (replaces any existing relationship of same type)
                // source stores the relationship pointing to target
                // Use inverse from definitions to determine bidirectionality
                let bidirectional = self.store.get_inverse_type(&rel_type).is_some();
                match self.store.set_relationship_with_creator(
                    &source_id,
                    rel_type.clone(),
                    &target_id,
                    bidirectional,
                    Some(self.user_settings.display_name()),
                ) {
                    Ok(()) => {
                        self.save();
                        // Get display name from definition
                        let rel_name = self
                            .store
                            .get_definition_for_type(&rel_type)
                            .map(|d| d.display_name.clone())
                            .unwrap_or_else(|| format!("{:?}", rel_type));

                        let mut msg = format!("Relationship '{}' set", rel_name);
                        if !validation.warnings.is_empty() {
                            msg = format!("{} (Warning: {})", msg, validation.warnings.join("; "));
                        }
                        self.message = Some((msg, false));
                    }
                    Err(e) => {
                        self.message = Some((format!("Failed to set relationship: {}", e), true));
                    }
                }
            }
        }
    }

    /// Apply AI-suggested improved description to a requirement
    fn apply_ai_description(&mut self, req_id: Uuid, new_description: String) {
        // Find the requirement by ID
        if let Some(idx) = self.store.requirements.iter().position(|r| r.id == req_id) {
            let req = &mut self.store.requirements[idx];
            let spec_id = req.spec_id.clone().unwrap_or_else(|| req_id.to_string());
            let old_description = req.description.clone();

            // Update the description
            req.description = new_description.clone();
            req.modified_at = chrono::Utc::now();

            // Track history
            req.history.push(aida_core::models::HistoryEntry {
                id: uuid::Uuid::new_v4(),
                author: self.user_settings.name.clone(),
                timestamp: chrono::Utc::now(),
                changes: vec![aida_core::models::FieldChange {
                    field_name: "description".to_string(),
                    old_value: old_description,
                    new_value: new_description,
                }],
            });

            // Save changes
            self.save();

            // Show toast notification
            self.toast_message = Some(ToastNotification {
                message: format!("Applied AI improvement to {}", spec_id),
                is_success: true,
                show_until: std::time::Instant::now() + std::time::Duration::from_secs(3),
            });
        } else {
            // Requirement not found - show error
            self.toast_message = Some(ToastNotification {
                message: "Failed to apply: requirement not found".to_string(),
                is_success: false,
                show_until: std::time::Instant::now() + std::time::Duration::from_secs(3),
            });
        }
    }

    fn show_list_panel(&mut self, ctx: &egui::Context, in_form_view: bool, forced_width: Option<f32>) {
        let mut panel = egui::SidePanel::left("list_panel")
            .min_width(200.0)
            .default_width(400.0);

        if let Some(width) = forced_width {
            panel = panel.exact_width(width);
        }

        panel.show(ctx, |ui| {
                // Header with optional hide button (only in form view)
                ui.horizontal(|ui| {
                    ui.heading("Requirements");
                    if in_form_view {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button("▶ Hide")
                                .on_hover_text("Hide requirements list")
                                .clicked()
                            {
                                self.left_panel_collapsed = true;
                            }
                        });
                    }
                });
                ui.separator();

                // Search bar
                ui.horizontal(|ui| {
                    self.show_search_bar(ui, 150.0, false);

                    // Filter toggle button
                    let filter_active =
                        !self.filter_types.is_empty() || !self.filter_features.is_empty()
                        || !self.filter_prefixes.is_empty() || !self.filter_statuses.is_empty()
                        || !self.filter_priorities.is_empty();
                    let filter_btn_text = if filter_active {
                        "🔽 Filters ●"
                    } else {
                        "🔽 Filters"
                    };
                    if ui.button(filter_btn_text).clicked() {
                        self.show_filter_dialog_list1 = !self.show_filter_dialog_list1;
                    }
                });

                // Perspective and preset selector
                ui.horizontal(|ui| {
                    ui.label("View:");

                    // Determine what to show as selected text
                    let selected_text = if let Some(ref preset_name) = self.active_preset {
                        if self.current_view_matches_active_preset() {
                            preset_name.clone()
                        } else {
                            format!("{}*", preset_name) // Modified indicator
                        }
                    } else {
                        self.perspective.label().to_string()
                    };

                    // Clone presets for iteration
                    let presets: Vec<ViewPreset> = self.user_settings.view_presets.clone();
                    let mut preset_to_apply: Option<ViewPreset> = None;
                    let mut clear_active_preset = false;

                    egui::ComboBox::from_id_salt("perspective_combo")
                        .selected_text(&selected_text)
                        .show_ui(ui, |ui| {
                            // Built-in perspectives section
                            ui.label("Built-in Views");
                            ui.separator();

                            // Check if current view matches a built-in (for highlighting)
                            let is_flat = self.perspective == Perspective::Flat
                                && self.active_preset.is_none();
                            let is_parent_child = self.perspective == Perspective::ParentChild
                                && self.active_preset.is_none();
                            let is_verification = self.perspective == Perspective::Verification
                                && self.active_preset.is_none();
                            let is_references = self.perspective == Perspective::References
                                && self.active_preset.is_none();
                            let is_by_owner = self.perspective == Perspective::ByOwner
                                && self.active_preset.is_none();

                            if ui
                                .selectable_label(is_flat, Perspective::Flat.label())
                                .clicked()
                            {
                                self.perspective = Perspective::Flat;
                                clear_active_preset = true;
                            }
                            if ui
                                .selectable_label(is_parent_child, Perspective::ParentChild.label())
                                .clicked()
                            {
                                self.perspective = Perspective::ParentChild;
                                clear_active_preset = true;
                            }
                            if ui
                                .selectable_label(
                                    is_verification,
                                    Perspective::Verification.label(),
                                )
                                .clicked()
                            {
                                self.perspective = Perspective::Verification;
                                clear_active_preset = true;
                            }
                            if ui
                                .selectable_label(is_references, Perspective::References.label())
                                .clicked()
                            {
                                self.perspective = Perspective::References;
                                clear_active_preset = true;
                            }
                            if ui
                                .selectable_label(is_by_owner, Perspective::ByOwner.label())
                                .clicked()
                            {
                                self.perspective = Perspective::ByOwner;
                                clear_active_preset = true;
                            }

                            // User presets section (if any exist)
                            if !presets.is_empty() {
                                ui.add_space(5.0);
                                ui.label("Saved Presets");
                                ui.separator();

                                for preset in &presets {
                                    let is_selected = self.active_preset.as_ref()
                                        == Some(&preset.name)
                                        && self.current_view_matches_active_preset();

                                    ui.horizontal(|ui| {
                                        if ui.selectable_label(is_selected, &preset.name).clicked()
                                        {
                                            preset_to_apply = Some(preset.clone());
                                        }
                                        // Delete button (small X)
                                        if ui
                                            .small_button("✕")
                                            .on_hover_text("Delete preset")
                                            .clicked()
                                        {
                                            self.show_delete_preset_confirm =
                                                Some(preset.name.clone());
                                        }
                                    });
                                }
                            }
                        });

                    // Apply preset if one was selected
                    if let Some(preset) = preset_to_apply {
                        self.apply_preset(&preset);
                    }

                    // Clear active preset if built-in was selected
                    if clear_active_preset {
                        self.active_preset = None;
                    }

                    // Direction selector (only shown for non-flat perspectives)
                    if self.perspective != Perspective::Flat {
                        egui::ComboBox::from_id_salt("direction_combo")
                            .selected_text(self.perspective_direction.label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.perspective_direction,
                                    PerspectiveDirection::TopDown,
                                    "Top-down ↓",
                                );
                                ui.selectable_value(
                                    &mut self.perspective_direction,
                                    PerspectiveDirection::BottomUp,
                                    "Bottom-up ↑",
                                );
                            });
                    }

                    // Save As button (shown when view has unsaved changes)
                    if self.has_unsaved_view() {
                        if ui
                            .button("💾 Save As...")
                            .on_hover_text("Save current view as a preset")
                            .clicked()
                        {
                            // Pre-fill with active preset name if modifying, otherwise empty
                            self.preset_name_input = self.active_preset.clone().unwrap_or_default();
                            self.show_save_preset_dialog = true;
                        }
                    }

                    // Reset button (shown when not at default)
                    if self.perspective != Perspective::Flat
                        || self.perspective_direction != PerspectiveDirection::TopDown
                        || !self.filter_types.is_empty()
                        || !self.filter_features.is_empty()
                    {
                        if ui
                            .small_button("↺")
                            .on_hover_text("Reset to default view")
                            .clicked()
                        {
                            self.reset_to_default_view();
                        }
                    }
                });

                // Collapsible filter panel
                if self.show_filter_panel {
                    ui.separator();
                    self.show_filter_controls(ui);
                }

                ui.separator();

                // Check for drag auto-scroll before showing ScrollArea
                // Compute scroll delta based on pointer position during drag
                let mut scroll_delta_to_apply = 0.0;
                if self.drag_source.is_some() {
                    if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                        // We need to check against the available rect for the scroll area
                        let available_rect = ui.available_rect_before_wrap();
                        let edge_zone = 50.0; // Pixels from edge to start scrolling
                        let scroll_speed = 10.0; // Pixels per frame

                        if pointer_pos.y < available_rect.top() + edge_zone
                            && pointer_pos.y >= available_rect.top()
                        {
                            // Near top - scroll up
                            let intensity =
                                1.0 - (pointer_pos.y - available_rect.top()) / edge_zone;
                            scroll_delta_to_apply = -scroll_speed * intensity;
                        } else if pointer_pos.y > available_rect.bottom() - edge_zone
                            && pointer_pos.y <= available_rect.bottom()
                        {
                            // Near bottom - scroll down
                            let intensity =
                                1.0 - (available_rect.bottom() - pointer_pos.y) / edge_zone;
                            scroll_delta_to_apply = scroll_speed * intensity;
                        }
                    }
                }

                // Requirement list (flat or tree) with drag auto-scroll support
                // Use horizontal() for manual horizontal scrolling (no auto-scroll on selection)
                // wrapped with vertical() for vertical scrolling with auto-scroll on selection
                let mut scroll_area = egui::ScrollArea::vertical()
                    .id_salt("requirements_list_scroll")
                    .auto_shrink([false, false]); // Don't shrink to content

                // If we need to scroll due to drag, set the scroll offset
                if scroll_delta_to_apply != 0.0 {
                    let new_offset = (self.drag_scroll_delta + scroll_delta_to_apply).max(0.0);
                    self.drag_scroll_delta = new_offset;
                    scroll_area = scroll_area.vertical_scroll_offset(new_offset);
                    ui.ctx().request_repaint();
                }

                let scroll_output = scroll_area.show(ui, |ui| {
                    // Wrap content in horizontal scroll for wide titles
                    // This is separate from vertical scroll so scroll_to_me only scrolls vertically
                    egui::ScrollArea::horizontal()
                        .id_salt("requirements_list_horizontal_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            // Set minimum width for content to prevent text wrapping
                            ui.set_min_width(300.0);
                            match &self.perspective {
                                Perspective::Flat => {
                                    self.show_flat_list(ui);
                                }
                                _ => {
                                    self.show_tree_list(ui);
                                }
                            }
                        });
                });

                // Update stored offset from actual scroll state (for when user scrolls manually)
                if self.drag_source.is_some() {
                    self.drag_scroll_delta = scroll_output.state.offset.y;
                } else {
                    self.drag_scroll_delta = scroll_output.state.offset.y;
                }

                // Selection remains fixed when scrolling - user must click to change selection
            });
    }

    /// Show split panel (second requirements list) on the left side (next to main list)
    fn show_split_panel(&mut self, ctx: &egui::Context, forced_width: Option<f32>) {
        let mut panel = egui::SidePanel::left("split_panel")
            .min_width(200.0)
            .default_width(350.0);

        // Apply forced width if provided (for equal split when detail is collapsed)
        if let Some(width) = forced_width {
            panel = panel.exact_width(width);
        }

        panel.show(ctx, |ui| {
                // Header with close button
                ui.horizontal(|ui| {
                    ui.heading("Requirements");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("⊟")
                            .on_hover_text("Close split view")
                            .clicked()
                        {
                            // Return to ListDetailsSide mode
                            self.layout_mode = LayoutMode::ListDetailsSide;
                        }
                    });
                });
                ui.separator();

                // Search bar
                ui.horizontal(|ui| {
                    ui.label("🔍");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.split_filter_text)
                            .hint_text("Search...")
                            .desired_width(120.0),
                    );
                    // Clear button
                    if !self.split_filter_text.is_empty() {
                        if ui.small_button("✕").on_hover_text("Clear search").clicked() {
                            self.split_filter_text.clear();
                        }
                    }

                    // Filter toggle
                    let filter_active = !self.split_filter_types.is_empty()
                        || !self.split_filter_features.is_empty()
                        || !self.split_filter_prefixes.is_empty()
                        || !self.split_filter_statuses.is_empty()
                        || !self.split_filter_priorities.is_empty();
                    let filter_btn_text = if filter_active {
                        "🔽 ●"
                    } else {
                        "🔽"
                    };
                    if ui.button(filter_btn_text).on_hover_text("Filters").clicked() {
                        self.show_filter_dialog_list2 = !self.show_filter_dialog_list2;
                    }
                });

                // Perspective selector
                ui.horizontal(|ui| {
                    ui.label("View:");
                    let selected_text = self.split_perspective.label().to_string();

                    egui::ComboBox::from_id_salt("split_perspective_combo")
                        .selected_text(&selected_text)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(
                                    self.split_perspective == Perspective::Flat,
                                    Perspective::Flat.label(),
                                )
                                .clicked()
                            {
                                self.split_perspective = Perspective::Flat;
                            }
                            if ui
                                .selectable_label(
                                    self.split_perspective == Perspective::ParentChild,
                                    Perspective::ParentChild.label(),
                                )
                                .clicked()
                            {
                                self.split_perspective = Perspective::ParentChild;
                            }
                            if ui
                                .selectable_label(
                                    self.split_perspective == Perspective::Verification,
                                    Perspective::Verification.label(),
                                )
                                .clicked()
                            {
                                self.split_perspective = Perspective::Verification;
                            }
                            if ui
                                .selectable_label(
                                    self.split_perspective == Perspective::References,
                                    Perspective::References.label(),
                                )
                                .clicked()
                            {
                                self.split_perspective = Perspective::References;
                            }
                            if ui
                                .selectable_label(
                                    self.split_perspective == Perspective::ByOwner,
                                    Perspective::ByOwner.label(),
                                )
                                .clicked()
                            {
                                self.split_perspective = Perspective::ByOwner;
                            }
                        });

                    // Direction for hierarchical views
                    if matches!(
                        self.split_perspective,
                        Perspective::ParentChild | Perspective::Verification | Perspective::References
                    ) {
                        egui::ComboBox::from_id_salt("split_direction_combo")
                            .selected_text(self.split_perspective_direction.label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.split_perspective_direction,
                                    PerspectiveDirection::TopDown,
                                    "↓",
                                );
                                ui.selectable_value(
                                    &mut self.split_perspective_direction,
                                    PerspectiveDirection::BottomUp,
                                    "↑",
                                );
                            });
                    }
                });

                ui.separator();

                // Requirements list
                egui::ScrollArea::vertical()
                    .id_salt("split_panel_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::ScrollArea::horizontal()
                            .id_salt("split_panel_horizontal_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(250.0);
                                self.show_split_list(ui);
                            });
                    });
            });
    }

    /// Show the requirements list in the split panel
    fn show_split_list(&mut self, ui: &mut egui::Ui) {
        // Get filtered requirements for split panel using the shared filter function
        let filtered_indices = self.get_split_filtered_indices();

        // Display based on perspective - use custom rendering like List 1 (no hover effect)
        match self.split_perspective {
            Perspective::Flat => {
                for &idx in &filtered_indices {
                    let req = &self.store.requirements[idx];
                    let req_id = req.id;
                    let is_selected = self.split_selected_idx == Some(idx);
                    let should_scroll = self.split_scroll_to_requirement == Some(req_id);
                    let icon = self.get_requirement_icon(req);
                    let label = format!(
                        "{} {} - {}",
                        icon,
                        req.spec_id.as_deref().unwrap_or("?"),
                        req.title
                    );

                    // Custom rendering - constrain to available width
                    let available_width = ui.available_width() - 8.0;

                    let text = egui::WidgetText::from(&label);
                    let galley = text.into_galley(
                        ui,
                        Some(egui::TextWrapMode::Truncate),
                        available_width.max(50.0),
                        egui::TextStyle::Body,
                    );
                    let desired_size = egui::vec2(available_width.max(50.0), galley.size().y) + egui::vec2(8.0, 4.0);
                    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

                    // Scroll to center if requested
                    if should_scroll {
                        response.scroll_to_me(Some(egui::Align::Center));
                        self.split_scroll_to_requirement = None;
                    }

                    // Paint background only when selected
                    if is_selected {
                        ui.painter().rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                    }

                    // Paint text
                    let text_pos = rect.min + egui::vec2(4.0, 2.0);
                    let text_color = if is_selected {
                        ui.visuals().selection.stroke.color
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.painter().galley(text_pos, galley, text_color);

                    // Block interactions when status popup is open
                    if !self.quick_change_field.is_some() {
                        if response.clicked() {
                            self.split_selected_idx = Some(idx);
                            self.focused_list = FocusedList::List2;
                            // Sync List 1 when navigation is locked
                            if self.navigation_locked {
                                self.selected_idx = Some(idx);
                                self.scroll_to_requirement = Some(req_id);
                                self.pending_view_change = Some(View::Detail);
                            }
                            // Clear focus from any text fields to enable hotkeys
                            ui.ctx().memory_mut(|mem| mem.surrender_focus(egui::Id::NULL));
                        }
                        if response.double_clicked() {
                            // Switch to edit mode on double-click
                            self.split_selected_idx = Some(idx);
                            self.selected_idx = Some(idx);
                            self.focused_list = FocusedList::List2;
                            self.load_form_from_requirement(idx);
                            self.pending_view_change = Some(View::Edit);
                            // Clear focus from any text fields to enable hotkeys
                            ui.ctx().memory_mut(|mem| mem.surrender_focus(egui::Id::NULL));
                        }
                    }
                }
            }
            _ => {
                // For tree views, show a simplified flat list with hierarchy indication
                // Full tree implementation would require more complex state management
                for &idx in &filtered_indices {
                    let req = &self.store.requirements[idx];
                    let req_id = req.id;
                    let is_selected = self.split_selected_idx == Some(idx);
                    let should_scroll = self.split_scroll_to_requirement == Some(req_id);

                    // Show relationship count as indicator
                    let rel_count = req.relationships.len();
                    let rel_indicator = if rel_count > 0 {
                        format!(" [{}]", rel_count)
                    } else {
                        String::new()
                    };

                    let icon = self.get_requirement_icon(req);
                    let label = format!(
                        "{} {} - {}{}",
                        icon,
                        req.spec_id.as_deref().unwrap_or("?"),
                        req.title,
                        rel_indicator
                    );

                    // Custom rendering - constrain to available width
                    let available_width = ui.available_width() - 8.0;
                    let text = egui::WidgetText::from(&label);
                    let galley = text.into_galley(
                        ui,
                        Some(egui::TextWrapMode::Truncate),
                        available_width.max(50.0),
                        egui::TextStyle::Body,
                    );
                    let desired_size = egui::vec2(available_width.max(50.0), galley.size().y) + egui::vec2(8.0, 4.0);
                    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

                    // Scroll to center if requested
                    if should_scroll {
                        response.scroll_to_me(Some(egui::Align::Center));
                        self.split_scroll_to_requirement = None;
                    }

                    // Paint background only when selected
                    if is_selected {
                        ui.painter().rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                    }

                    // Paint text
                    let text_pos = rect.min + egui::vec2(4.0, 2.0);
                    let text_color = if is_selected {
                        ui.visuals().selection.stroke.color
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.painter().galley(text_pos, galley, text_color);

                    // Block interactions when status popup is open
                    if !self.quick_change_field.is_some() {
                        if response.clicked() {
                            self.split_selected_idx = Some(idx);
                            self.focused_list = FocusedList::List2;
                            // Sync List 1 when navigation is locked
                            if self.navigation_locked {
                                self.selected_idx = Some(idx);
                                self.scroll_to_requirement = Some(req_id);
                                self.pending_view_change = Some(View::Detail);
                            }
                            // Clear focus from any text fields to enable hotkeys
                            ui.ctx().memory_mut(|mem| mem.surrender_focus(egui::Id::NULL));
                        }
                        if response.double_clicked() {
                            self.split_selected_idx = Some(idx);
                            self.selected_idx = Some(idx);
                            self.focused_list = FocusedList::List2;
                            self.load_form_from_requirement(idx);
                            self.pending_view_change = Some(View::Edit);
                            // Clear focus from any text fields to enable hotkeys
                            ui.ctx().memory_mut(|mem| mem.surrender_focus(egui::Id::NULL));
                        }
                    }
                }
            }
        }

        if filtered_indices.is_empty() {
            ui.label("No matching requirements");
        }
    }

    /// Show list content (for use in various layout modes)
    fn show_list_content(&mut self, ui: &mut egui::Ui, _in_form_view: bool) {
        // Header
        ui.horizontal(|ui| {
            ui.heading("Requirements");
        });
        ui.separator();

        // Search bar with filter toggle
        ui.horizontal(|ui| {
            self.show_search_bar(ui, 250.0, false);

            // Filter toggle button
            let filter_active = !self.filter_types.is_empty() || !self.filter_features.is_empty()
                || !self.filter_prefixes.is_empty() || !self.filter_statuses.is_empty()
                || !self.filter_priorities.is_empty();
            let filter_btn_text = if filter_active {
                "🔽 Filters ●"
            } else {
                "🔽 Filters"
            };
            if ui.button(filter_btn_text).clicked() {
                self.show_filter_dialog_list1 = !self.show_filter_dialog_list1;
            }
        });

        // Perspective and preset selector
        ui.horizontal(|ui| {
            ui.label("View:");

            // Determine what to show as selected text
            let selected_text = if let Some(ref preset_name) = self.active_preset {
                if self.current_view_matches_active_preset() {
                    preset_name.clone()
                } else {
                    format!("{}*", preset_name) // Modified indicator
                }
            } else {
                self.perspective.label().to_string()
            };

            // Clone presets for iteration
            let presets: Vec<ViewPreset> = self.user_settings.view_presets.clone();
            let mut preset_to_apply: Option<ViewPreset> = None;
            let mut clear_active_preset = false;

            egui::ComboBox::from_id_salt("perspective_combo_list_only")
                .selected_text(&selected_text)
                .show_ui(ui, |ui| {
                    // Built-in perspectives section
                    ui.label("Built-in Views");
                    ui.separator();

                    let is_flat =
                        self.perspective == Perspective::Flat && self.active_preset.is_none();
                    let is_parent_child = self.perspective == Perspective::ParentChild
                        && self.active_preset.is_none();
                    let is_verification = self.perspective == Perspective::Verification
                        && self.active_preset.is_none();
                    let is_references =
                        self.perspective == Perspective::References && self.active_preset.is_none();
                    let is_by_owner =
                        self.perspective == Perspective::ByOwner && self.active_preset.is_none();

                    if ui
                        .selectable_label(is_flat, Perspective::Flat.label())
                        .clicked()
                    {
                        self.perspective = Perspective::Flat;
                        clear_active_preset = true;
                    }
                    if ui
                        .selectable_label(is_parent_child, Perspective::ParentChild.label())
                        .clicked()
                    {
                        self.perspective = Perspective::ParentChild;
                        clear_active_preset = true;
                    }
                    if ui
                        .selectable_label(is_verification, Perspective::Verification.label())
                        .clicked()
                    {
                        self.perspective = Perspective::Verification;
                        clear_active_preset = true;
                    }
                    if ui
                        .selectable_label(is_references, Perspective::References.label())
                        .clicked()
                    {
                        self.perspective = Perspective::References;
                        clear_active_preset = true;
                    }
                    if ui
                        .selectable_label(is_by_owner, Perspective::ByOwner.label())
                        .clicked()
                    {
                        self.perspective = Perspective::ByOwner;
                        clear_active_preset = true;
                    }

                    // User presets section (if any)
                    if !presets.is_empty() {
                        ui.separator();
                        ui.label("Custom Presets");
                        ui.separator();

                        for preset in &presets {
                            let is_active = self.active_preset.as_ref() == Some(&preset.name);
                            if ui.selectable_label(is_active, &preset.name).clicked() {
                                preset_to_apply = Some(preset.clone());
                            }
                        }
                    }
                });

            // Apply preset if selected
            if let Some(preset) = preset_to_apply {
                self.apply_preset(&preset);
            }
            if clear_active_preset {
                self.active_preset = None;
            }

            // Direction selector for hierarchical views
            if matches!(
                self.perspective,
                Perspective::ParentChild | Perspective::Verification | Perspective::References
            ) {
                egui::ComboBox::from_id_salt("direction_combo_list_only")
                    .selected_text(self.perspective_direction.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.perspective_direction,
                            PerspectiveDirection::TopDown,
                            "Top-down ↓",
                        );
                        ui.selectable_value(
                            &mut self.perspective_direction,
                            PerspectiveDirection::BottomUp,
                            "Bottom-up ↑",
                        );
                    });
            }

            // Save As button (shown when view has unsaved changes)
            if self.has_unsaved_view() {
                if ui
                    .button("💾 Save As...")
                    .on_hover_text("Save current view as a preset")
                    .clicked()
                {
                    self.preset_name_input = self.active_preset.clone().unwrap_or_default();
                    self.show_save_preset_dialog = true;
                }
            }

            // Reset button (shown when not at default)
            if self.perspective != Perspective::Flat
                || self.perspective_direction != PerspectiveDirection::TopDown
                || !self.filter_types.is_empty()
                || !self.filter_features.is_empty()
            {
                if ui
                    .small_button("↺")
                    .on_hover_text("Reset to default view")
                    .clicked()
                {
                    self.reset_to_default_view();
                }
            }
        });

        // Collapsible filter panel
        if self.show_filter_panel {
            ui.separator();
            self.show_filter_controls(ui);
        }

        ui.separator();

        // Requirement list (flat or tree) with drag auto-scroll support
        let mut scroll_delta_to_apply = 0.0;
        if self.drag_source.is_some() {
            if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                let available_rect = ui.available_rect_before_wrap();
                let edge_zone = 50.0;
                let scroll_speed = 10.0;

                if pointer_pos.y < available_rect.top() + edge_zone
                    && pointer_pos.y >= available_rect.top()
                {
                    let intensity = 1.0 - (pointer_pos.y - available_rect.top()) / edge_zone;
                    scroll_delta_to_apply = -scroll_speed * intensity;
                } else if pointer_pos.y > available_rect.bottom() - edge_zone
                    && pointer_pos.y <= available_rect.bottom()
                {
                    let intensity = 1.0 - (available_rect.bottom() - pointer_pos.y) / edge_zone;
                    scroll_delta_to_apply = scroll_speed * intensity;
                }
            }
        }

        let mut scroll_area = egui::ScrollArea::vertical()
            .id_salt("requirements_list_scroll_full")
            .auto_shrink([false, false]);

        if scroll_delta_to_apply != 0.0 {
            let new_offset = (self.drag_scroll_delta + scroll_delta_to_apply).max(0.0);
            self.drag_scroll_delta = new_offset;
            scroll_area = scroll_area.vertical_scroll_offset(new_offset);
            ui.ctx().request_repaint();
        }

        let scroll_output = scroll_area.show(ui, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("requirements_list_horizontal_scroll_full")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_width(400.0);
                    match &self.perspective {
                        Perspective::Flat => {
                            self.show_flat_list(ui);
                        }
                        _ => {
                            self.show_tree_list(ui);
                        }
                    }
                });
        });

        // Update stored offset from actual scroll state
        self.drag_scroll_delta = scroll_output.state.offset.y;
    }

    /// Render the search bar with mode toggle and match navigation
    /// Returns true if search text changed
    fn show_search_bar(&mut self, ui: &mut egui::Ui, desired_width: f32, compact: bool) {
        let prev_filter_text = self.filter_text.clone();

        ui.label("🔍");
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.filter_text)
                .hint_text(if compact { "Search..." } else { "Search (case-insensitive)..." })
                .desired_width(desired_width),
        );

        // Check if search text changed
        if self.filter_text != prev_filter_text {
            self.update_search_matches();
        }

        // Clear button (only when there's text)
        if !self.filter_text.is_empty() {
            if ui.small_button("✕").on_hover_text("Clear search (Esc)").clicked() {
                self.clear_search();
            }
        }

        // Search mode toggle button - always visible with label for clarity
        let mode_icon = self.user_settings.search_mode.icon();
        let mode_label = self.user_settings.search_mode.label();
        let mode_btn = ui.button(format!("{} {}", mode_icon, if compact { "" } else { mode_label }))
            .on_hover_text(format!(
                "Search Mode: {}\nClick to toggle between Highlight (show all, highlight matches) and Filter (show only matches)",
                mode_label
            ));
        if mode_btn.clicked() {
            self.user_settings.search_mode = match self.user_settings.search_mode {
                SearchMode::Highlight => SearchMode::Filter,
                SearchMode::Filter => SearchMode::Highlight,
            };
            let _ = self.user_settings.save();
        }

        // Match count and navigation (only in highlight mode with active search)
        if !self.filter_text.is_empty() && self.user_settings.search_mode == SearchMode::Highlight {
            let match_count = self.search_match_indices.len();
            let current = self.search_current_match.map(|c| c + 1).unwrap_or(0);
            ui.label(format!("{}/{}", current, match_count));

            // Previous/Next buttons
            if ui.small_button("▲").on_hover_text("Previous match (N)").clicked() {
                self.prev_search_match();
            }
            if ui.small_button("▼").on_hover_text("Next match (n)").clicked() {
                self.next_search_match();
            }
        }

        // Handle Enter in search field to go to next match
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if self.user_settings.search_mode == SearchMode::Highlight {
                self.next_search_match();
            }
        }
    }

    fn show_filter_controls(&mut self, ui: &mut egui::Ui) {
        // Search options section
        ui.label("Search Options:");
        ui.horizontal_wrapped(|ui| {
            // Search mode toggle
            let mode_label = format!("Mode: {}", self.user_settings.search_mode.label());
            if ui.button(&mode_label)
                .on_hover_text("Toggle between Highlight (shows all, highlights matches) and Filter (shows only matches)")
                .clicked()
            {
                self.user_settings.search_mode = match self.user_settings.search_mode {
                    SearchMode::Highlight => SearchMode::Filter,
                    SearchMode::Filter => SearchMode::Highlight,
                };
                let _ = self.user_settings.save();
            }

            ui.separator();

            // Wrap search option
            if ui.checkbox(&mut self.user_settings.wrap_search, "Wrap").changed() {
                let _ = self.user_settings.save();
            }
            ui.label("").on_hover_text("When navigating matches, wrap from last to first (or first to last)");
        });

        ui.add_space(4.0);

        // Search scope section
        ui.label("Search In:");
        ui.horizontal_wrapped(|ui| {
            // "Everything" toggle - when checked, enables all; when unchecked, clears all
            let mut everything = self.search_scope.is_all();
            if ui.checkbox(&mut everything, "Everything").changed() {
                if everything {
                    self.search_scope = SearchScope::all();
                } else {
                    // When unchecking "Everything", enable only Title as a sensible default
                    self.search_scope = SearchScope {
                        title: true,
                        description: false,
                        comments: false,
                        spec_id: false,
                    };
                }
            }

            ui.separator();

            // Individual scope checkboxes (only shown when not "Everything")
            if !self.search_scope.is_all() {
                ui.checkbox(&mut self.search_scope.title, "Title");
                ui.checkbox(&mut self.search_scope.description, "Description");
                ui.checkbox(&mut self.search_scope.comments, "Comments");
                ui.checkbox(&mut self.search_scope.spec_id, "ID");
            }
        });

        ui.separator();

        // Filter tabs: Root and Children
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.filter_tab == FilterTab::Root, "Root")
                .clicked()
            {
                self.filter_tab = FilterTab::Root;
            }
            if ui
                .selectable_label(self.filter_tab == FilterTab::Children, "Children")
                .clicked()
            {
                self.filter_tab = FilterTab::Children;
            }
        });

        ui.separator();

        match self.filter_tab {
            FilterTab::Root => {
                self.show_root_filter_controls(ui);
            }
            FilterTab::Children => {
                self.show_children_filter_controls(ui);
            }
        }

        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_archived, "Show Archived");
            ui.checkbox(&mut self.show_filtered_parents, "Show Parents")
                .on_hover_text(
                    "Show greyed-out parent requirements for filtered items in tree views",
                );
        });
    }

    fn show_root_filter_controls(&mut self, ui: &mut egui::Ui) {
        ui.label("Type Filters:");
        ui.horizontal_wrapped(|ui| {
            let types = [
                (RequirementType::Functional, "FR"),
                (RequirementType::NonFunctional, "NFR"),
                (RequirementType::System, "SR"),
                (RequirementType::User, "UR"),
                (RequirementType::ChangeRequest, "CR"),
                (RequirementType::Bug, "BUG"),
                (RequirementType::Epic, "Epic"),
                (RequirementType::Story, "Story"),
                (RequirementType::Task, "Task"),
                (RequirementType::Spike, "Spike"),
                (RequirementType::Sprint, "Sprint"),
            ];

            for (req_type, label) in types {
                let mut checked = self.filter_types.contains(&req_type);
                if ui.checkbox(&mut checked, label).changed() {
                    if checked {
                        self.filter_types.insert(req_type);
                    } else {
                        self.filter_types.remove(&req_type);
                    }
                }
            }

            if ui.small_button("Clear").clicked() {
                self.filter_types.clear();
            }
        });

        ui.add_space(5.0);
        ui.label("Feature Filters:");

        let features = self.get_all_features();
        ui.horizontal_wrapped(|ui| {
            for feature in &features {
                let mut checked = self.filter_features.contains(feature);
                // Truncate long feature names for display
                let display_name = if feature.len() > 15 {
                    format!("{}...", &feature[..12])
                } else {
                    feature.clone()
                };

                if ui
                    .checkbox(&mut checked, &display_name)
                    .on_hover_text(feature)
                    .changed()
                {
                    if checked {
                        self.filter_features.insert(feature.clone());
                    } else {
                        self.filter_features.remove(feature);
                    }
                }
            }

            if ui.small_button("Clear").clicked() {
                self.filter_features.clear();
            }
        });

        // Prefix filters
        let prefixes = self.store.get_all_prefixes();
        if !prefixes.is_empty() {
            ui.add_space(5.0);
            ui.label("ID Prefix Filters:");
            ui.horizontal_wrapped(|ui| {
                for prefix in &prefixes {
                    let mut checked = self.filter_prefixes.contains(prefix);
                    if ui.checkbox(&mut checked, prefix).changed() {
                        if checked {
                            self.filter_prefixes.insert(prefix.clone());
                        } else {
                            self.filter_prefixes.remove(prefix);
                        }
                    }
                }

                if ui.small_button("Clear").clicked() {
                    self.filter_prefixes.clear();
                }
            });
        }

        // Status filters
        ui.add_space(5.0);
        ui.label("Status Filters:");
        ui.horizontal_wrapped(|ui| {
            let statuses = [
                (RequirementStatus::Draft, "Draft"),
                (RequirementStatus::Approved, "Approved"),
                (RequirementStatus::Completed, "Completed"),
                (RequirementStatus::Rejected, "Rejected"),
            ];

            for (status, label) in statuses {
                let mut checked = self.filter_statuses.contains(&status);
                if ui.checkbox(&mut checked, label).changed() {
                    if checked {
                        self.filter_statuses.insert(status);
                    } else {
                        self.filter_statuses.remove(&status);
                    }
                }
            }

            if ui.small_button("Clear").clicked() {
                self.filter_statuses.clear();
            }
        });

        // Priority filters
        ui.add_space(5.0);
        ui.label("Priority Filters:");
        ui.horizontal_wrapped(|ui| {
            let priorities = [
                (RequirementPriority::High, "High"),
                (RequirementPriority::Medium, "Medium"),
                (RequirementPriority::Low, "Low"),
            ];

            for (priority, label) in priorities {
                let mut checked = self.filter_priorities.contains(&priority);
                if ui.checkbox(&mut checked, label).changed() {
                    if checked {
                        self.filter_priorities.insert(priority);
                    } else {
                        self.filter_priorities.remove(&priority);
                    }
                }
            }

            if ui.small_button("Clear").clicked() {
                self.filter_priorities.clear();
            }
        });
    }

    fn show_children_filter_controls(&mut self, ui: &mut egui::Ui) {
        // "Same as root" checkbox
        ui.checkbox(&mut self.children_same_as_root, "Same as root");
        ui.add_space(5.0);

        // Disable/grey out the controls when "Same as root" is checked
        let enabled = !self.children_same_as_root;

        ui.add_enabled_ui(enabled, |ui| {
            ui.label("Type Filters:");
            ui.horizontal_wrapped(|ui| {
                let types = [
                    (RequirementType::Functional, "FR"),
                    (RequirementType::NonFunctional, "NFR"),
                    (RequirementType::System, "SR"),
                    (RequirementType::User, "UR"),
                    (RequirementType::ChangeRequest, "CR"),
                    (RequirementType::Bug, "BUG"),
                    (RequirementType::Epic, "Epic"),
                    (RequirementType::Story, "Story"),
                    (RequirementType::Task, "Task"),
                    (RequirementType::Spike, "Spike"),
                    (RequirementType::Sprint, "Sprint"),
                ];

                for (req_type, label) in types {
                    let mut checked = self.child_filter_types.contains(&req_type);
                    if ui.checkbox(&mut checked, label).changed() {
                        if checked {
                            self.child_filter_types.insert(req_type);
                        } else {
                            self.child_filter_types.remove(&req_type);
                        }
                    }
                }

                if ui.small_button("Clear").clicked() {
                    self.child_filter_types.clear();
                }
            });

            ui.add_space(5.0);
            ui.label("Feature Filters:");

            let features = self.get_all_features();
            ui.horizontal_wrapped(|ui| {
                for feature in &features {
                    let mut checked = self.child_filter_features.contains(feature);
                    // Truncate long feature names for display
                    let display_name = if feature.len() > 15 {
                        format!("{}...", &feature[..12])
                    } else {
                        feature.clone()
                    };

                    if ui
                        .checkbox(&mut checked, &display_name)
                        .on_hover_text(feature)
                        .changed()
                    {
                        if checked {
                            self.child_filter_features.insert(feature.clone());
                        } else {
                            self.child_filter_features.remove(feature);
                        }
                    }
                }

                if ui.small_button("Clear").clicked() {
                    self.child_filter_features.clear();
                }
            });

            // Prefix filters
            let prefixes = self.store.get_all_prefixes();
            if !prefixes.is_empty() {
                ui.add_space(5.0);
                ui.label("ID Prefix Filters:");
                ui.horizontal_wrapped(|ui| {
                    for prefix in &prefixes {
                        let mut checked = self.child_filter_prefixes.contains(prefix);
                        if ui.checkbox(&mut checked, prefix).changed() {
                            if checked {
                                self.child_filter_prefixes.insert(prefix.clone());
                            } else {
                                self.child_filter_prefixes.remove(prefix);
                            }
                        }
                    }

                    if ui.small_button("Clear").clicked() {
                        self.child_filter_prefixes.clear();
                    }
                });
            }

            // Status filters
            ui.add_space(5.0);
            ui.label("Status Filters:");
            ui.horizontal_wrapped(|ui| {
                let statuses = [
                    (RequirementStatus::Draft, "Draft"),
                    (RequirementStatus::Approved, "Approved"),
                    (RequirementStatus::Completed, "Completed"),
                    (RequirementStatus::Rejected, "Rejected"),
                ];

                for (status, label) in statuses {
                    let mut checked = self.child_filter_statuses.contains(&status);
                    if ui.checkbox(&mut checked, label).changed() {
                        if checked {
                            self.child_filter_statuses.insert(status);
                        } else {
                            self.child_filter_statuses.remove(&status);
                        }
                    }
                }

                if ui.small_button("Clear").clicked() {
                    self.child_filter_statuses.clear();
                }
            });

            // Priority filters
            ui.add_space(5.0);
            ui.label("Priority Filters:");
            ui.horizontal_wrapped(|ui| {
                let priorities = [
                    (RequirementPriority::High, "High"),
                    (RequirementPriority::Medium, "Medium"),
                    (RequirementPriority::Low, "Low"),
                ];

                for (priority, label) in priorities {
                    let mut checked = self.child_filter_priorities.contains(&priority);
                    if ui.checkbox(&mut checked, label).changed() {
                        if checked {
                            self.child_filter_priorities.insert(priority);
                        } else {
                            self.child_filter_priorities.remove(&priority);
                        }
                    }
                }

                if ui.small_button("Clear").clicked() {
                    self.child_filter_priorities.clear();
                }
            });
        });
    }

    fn show_flat_list(&mut self, ui: &mut egui::Ui) {
        // Collect filtered indices first to avoid borrow issues (flat view uses root filters)
        let filtered_indices: Vec<usize> = self
            .store
            .requirements
            .iter()
            .enumerate()
            .filter(|(_, req)| self.passes_filters(req, true))
            .map(|(idx, _)| idx)
            .collect();

        for idx in filtered_indices {
            self.show_draggable_requirement(ui, idx, 0);
        }
    }

    /// Get the status icon for a requirement status string (uses user settings)
    fn get_status_icon(&self, status_string: &str) -> String {
        self.user_settings.status_icons.get_icon(status_string).to_string()
    }

    /// Get the icon for a requirement, considering both type and status
    /// For stateless types like Folder, returns a type-specific icon instead of status icon
    fn get_requirement_icon(&self, req: &Requirement) -> String {
        // For stateless types, use type-specific icons
        if self.store.is_type_stateless(&req.req_type) {
            return match req.req_type {
                RequirementType::Folder => "📁".to_string(),
                // Add other stateless types here as needed
                _ => "📄".to_string(), // Default for unknown stateless types
            };
        }
        // For stateful types, use status-based icons
        self.get_status_icon(&req.effective_status())
    }

    /// Check if a status is considered "inactive" (greyed out)
    fn is_inactive_status(status_string: &str) -> bool {
        let status_lower = status_string.to_lowercase();
        status_lower.contains("completed")
            || status_lower.contains("done")
            || status_lower.contains("rejected")
            || status_lower.contains("closed")
            || status_lower.contains("verified")
    }

    /// Render a single requirement item with drag-and-drop support
    fn show_draggable_requirement(&mut self, ui: &mut egui::Ui, idx: usize, indent: usize) {
        let Some(req) = self.store.requirements.get(idx) else {
            return;
        };

        let req_id = req.id;
        let spec_id = req.spec_id.clone();
        let title = req.title.clone();
        let status_string = req.effective_status();
        let is_inactive = Self::is_inactive_status(&status_string);
        let icon = self.get_requirement_icon(req); // Get icon before req ref is dropped
        let selected = self.selected_idx == Some(idx);
        let is_drag_source = self.drag_source == Some(idx);
        let is_drop_target = self.drop_target == Some(idx);
        let can_drag = self.perspective != Perspective::Flat; // Only allow drag in tree views
        let should_scroll_to = self.scroll_to_requirement == Some(req_id);
        let show_status_icons = self.user_settings.show_status_icons;

        // Check if this requirement matches the search (for highlight mode only)
        // In Filter mode, all visible items are matches, so no highlighting needed
        let has_search_text = !self.filter_text.is_empty();
        let is_highlight_mode = self.user_settings.search_mode == SearchMode::Highlight;

        // In Highlight mode, check if this requirement matches the search
        let is_search_match = if has_search_text && is_highlight_mode {
            self.matches_search(req)
        } else {
            false
        };

        // Only show current match highlighting in Highlight mode
        let is_current_match = has_search_text && is_highlight_mode && self.is_current_search_match(idx);

        let indent_space = indent as f32 * 20.0;

        ui.horizontal(|ui| {
            ui.add_space(indent_space);

            // Build the label with optional status icon
            let label = if show_status_icons {
                format!("{} {} - {}", icon, spec_id.as_deref().unwrap_or("N/A"), title)
            } else {
                format!("{} - {}", spec_id.as_deref().unwrap_or("N/A"), title)
            };

            // Visual feedback for drag/drop state and search matches
            let (bg_color, stroke) = if is_drop_target && can_drag {
                (
                    egui::Color32::from_rgba_unmultiplied(100, 200, 100, 60),
                    egui::Stroke::new(2.0, egui::Color32::GREEN),
                )
            } else if is_drag_source {
                (
                    egui::Color32::from_rgba_unmultiplied(100, 100, 200, 60),
                    egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE),
                )
            } else if selected && is_current_match {
                // Selected + current search match: bright orange highlight
                (
                    egui::Color32::from_rgba_unmultiplied(255, 180, 0, 180),
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 150, 0)),
                )
            } else if selected {
                (ui.visuals().selection.bg_fill, egui::Stroke::NONE)
            } else if is_current_match {
                // Current search match (not selected): bright orange highlight
                (
                    egui::Color32::from_rgba_unmultiplied(255, 180, 0, 150),
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 150, 0)),
                )
            } else if is_search_match {
                // Other search matches (Highlight mode only): visible yellow highlight
                (
                    egui::Color32::from_rgba_unmultiplied(255, 255, 0, 100),
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(200, 200, 0)),
                )
            } else {
                (egui::Color32::TRANSPARENT, egui::Stroke::NONE)
            };

            // Create an interactive area that supports both click and drag
            let sense = if can_drag {
                egui::Sense::click_and_drag()
            } else {
                egui::Sense::click()
            };

            // Calculate size for the label - constrain to available width
            // available_width already accounts for indent space since we called ui.add_space earlier
            let available_width = ui.available_width() - 8.0; // account for padding

            let text = egui::WidgetText::from(&label);
            let galley = text.into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                available_width.max(50.0), // minimum width to prevent issues
                egui::TextStyle::Body,
            );

            let desired_size = egui::vec2(available_width.max(50.0), galley.size().y) + egui::vec2(8.0, 4.0); // padding

            let (rect, response) = ui.allocate_exact_size(desired_size, sense);

            // Scroll to this item if requested (e.g., after adding a new requirement)
            if should_scroll_to {
                response.scroll_to_me(Some(egui::Align::Center));
                self.scroll_to_requirement = None; // Clear after scrolling
            }

            // Paint background
            if bg_color != egui::Color32::TRANSPARENT {
                ui.painter().rect_filled(rect, 2.0, bg_color);
            }
            if stroke != egui::Stroke::NONE {
                ui.painter().rect_stroke(rect, 2.0, stroke);
            }

            // Paint text
            let text_pos = rect.min + egui::vec2(4.0, 2.0);
            let text_color = if selected {
                ui.visuals().selection.stroke.color
            } else if is_inactive {
                // Grey out completed/rejected items
                ui.visuals().weak_text_color()
            } else {
                ui.visuals().text_color()
            };
            ui.painter().galley(text_pos, galley, text_color);

            // Handle interactions
            if response.clicked() {
                self.selected_idx = Some(idx);
                self.focused_list = FocusedList::List1;
                self.pending_view_change = Some(View::Detail);
                // Sync List 2 when navigation is locked
                if self.navigation_locked {
                    self.split_selected_idx = Some(idx);
                    self.split_scroll_to_requirement = Some(req_id);
                }
            }

            // Drag handling
            if can_drag {
                if response.drag_started() {
                    self.drag_source = Some(idx);
                }

                // Check if this is a drop target using pointer position (more reliable than hovered())
                if self.drag_source.is_some() && self.drag_source != Some(idx) {
                    if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                        if rect.contains(pointer_pos) {
                            self.drop_target = Some(idx);
                        }
                    }
                }

                // Release is handled globally in the update loop
            }

            // Show drag indicator while dragging
            if is_drag_source && ui.input(|i| i.pointer.is_decidedly_dragging()) {
                // Show a tooltip-style indicator following the cursor
                if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                    egui::Area::new(egui::Id::new("drag_indicator"))
                        .fixed_pos(pos + egui::vec2(10.0, 10.0))
                        .order(egui::Order::Tooltip)
                        .show(ui.ctx(), |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                ui.label(format!("📎 {}", spec_id.as_deref().unwrap_or("N/A")));
                            });
                        });
                }
            }
        });
    }

    fn show_tree_list(&mut self, ui: &mut egui::Ui) {
        // When there's active search text in FILTER mode, show a flat list of all matching results
        // This ensures search finds ALL matching requirements regardless of tree hierarchy
        // In HIGHLIGHT mode, keep the current tree view and just highlight matches
        let has_search_text = !self.filter_text.is_empty() && !self.search_scope.is_none();
        if has_search_text && self.user_settings.search_mode == SearchMode::Filter {
            self.show_flat_list(ui);
            return;
        }

        // Handle ByOwner perspective specially - uses Users as root nodes
        if self.perspective == Perspective::ByOwner {
            self.show_by_owner_list(ui);
            return;
        }

        // Get the relationship types for the current perspective
        let Some((outgoing_type, _incoming_type)) = self.perspective.relationship_types() else {
            // Fallback to flat list if no relationship types
            self.show_flat_list(ui);
            return;
        };

        // Compute ancestors that should be shown (greyed out) because they have matching descendants
        // Only compute if the "Show Parents" toggle is enabled
        let ancestors_to_show = if self.show_filtered_parents {
            self.compute_ancestor_ids_to_show(&outgoing_type)
        } else {
            HashSet::new()
        };

        match self.perspective_direction {
            PerspectiveDirection::TopDown => {
                // Find leaves (no outgoing relationships - they are not parents of anything)
                let leaves = self.find_tree_leaves(&outgoing_type);

                if leaves.is_empty() {
                    ui.label("No leaf requirements found for this perspective.");
                    ui.label("(All requirements have outgoing relationships)");
                    ui.add_space(10.0);
                    ui.label("Showing flat list instead:");
                    ui.separator();
                    self.show_flat_list(ui);
                } else {
                    for leaf_idx in leaves {
                        self.show_tree_node_bottom_up(ui, leaf_idx, &outgoing_type, 0);
                    }
                }
            }
            PerspectiveDirection::BottomUp => {
                // Find roots (requirements that are not children of anyone)
                // Include ancestors that have matching descendants (shown greyed out) if toggle is enabled
                let roots = self.find_tree_roots_with_ancestors(&outgoing_type, &ancestors_to_show);

                if roots.is_empty() {
                    ui.label("No root requirements found for this perspective.");
                    ui.label("(All requirements have incoming relationships)");
                    ui.add_space(10.0);
                    ui.label("Showing flat list instead:");
                    ui.separator();
                    self.show_flat_list(ui);
                } else {
                    for root_idx in roots {
                        self.show_tree_node_with_ancestors(
                            ui,
                            root_idx,
                            &outgoing_type,
                            0,
                            &ancestors_to_show,
                        );
                    }
                }
            }
        }
    }

    /// Show the By Owner list - Users as roots with their owned requirements as children
    fn show_by_owner_list(&mut self, ui: &mut egui::Ui) {
        // Get all non-archived users
        let users: Vec<_> = self.store.users.iter()
            .filter(|u| !u.archived)
            .map(|u| (u.id, u.name.clone(), u.handle.clone(), u.spec_id.clone()))
            .collect();

        if users.is_empty() {
            ui.label("No users defined. Add users in Settings → Members → Users.");
            return;
        }

        // Show mode toggle
        ui.horizontal(|ui| {
            ui.label("Mode:");
            if ui.selectable_label(self.owner_view_mode == OwnerViewMode::Flat, "Flat").clicked() {
                self.owner_view_mode = OwnerViewMode::Flat;
            }
            if ui.selectable_label(self.owner_view_mode == OwnerViewMode::Nested, "Nested").clicked() {
                self.owner_view_mode = OwnerViewMode::Nested;
            }
        });
        ui.separator();

        // For each user, show them as a root node
        for (user_id, name, handle, spec_id) in users {
            // Count owned requirements for this user
            let owned_count = self.store.requirements.iter()
                .filter(|r| !r.archived || self.show_archived)
                .filter(|r| self.matches_user_owner(r, &handle))
                .count();

            // Skip users with no owned requirements unless we want to show all
            if owned_count == 0 {
                continue;
            }

            let is_collapsed = self.tree_collapsed.get(&user_id).copied().unwrap_or(false);

            ui.horizontal(|ui| {
                // Expand/collapse button
                let toggle_text = if is_collapsed { "▶" } else { "▼" };
                if ui.small_button(toggle_text).clicked() {
                    self.tree_collapsed.insert(user_id, !is_collapsed);
                }

                // User display
                ui.label("👤");
                ui.strong(&name);
                ui.weak(format!("(@{})", handle));
                if let Some(id) = &spec_id {
                    ui.weak(format!("[{}]", id));
                }
                ui.weak(format!("({} items)", owned_count));
            });

            // Show owned requirements if expanded
            if !is_collapsed {
                match self.owner_view_mode {
                    OwnerViewMode::Flat => {
                        self.show_owned_requirements_flat(ui, &handle);
                    }
                    OwnerViewMode::Nested => {
                        self.show_owned_requirements_nested(ui, &handle);
                    }
                }
            }
        }

        // Show unassigned requirements (owner is empty or doesn't match any user)
        let unassigned_count = self.store.requirements.iter()
            .filter(|r| !r.archived || self.show_archived)
            .filter(|r| r.owner.is_empty() || !self.owner_is_known_user(&r.owner))
            .count();

        if unassigned_count > 0 {
            ui.add_space(10.0);
            let unassigned_id = Uuid::nil(); // Use nil UUID for unassigned section
            let is_collapsed = self.tree_collapsed.get(&unassigned_id).copied().unwrap_or(false);

            ui.horizontal(|ui| {
                let toggle_text = if is_collapsed { "▶" } else { "▼" };
                if ui.small_button(toggle_text).clicked() {
                    self.tree_collapsed.insert(unassigned_id, !is_collapsed);
                }
                ui.label("❓");
                ui.strong("Unassigned");
                ui.weak(format!("({} items)", unassigned_count));
            });

            if !is_collapsed {
                self.show_unassigned_requirements(ui);
            }
        }
    }

    /// Check if a requirement's owner matches a user handle (case-insensitive)
    fn matches_user_owner(&self, req: &Requirement, handle: &str) -> bool {
        let owner = req.owner.trim();
        // Match @handle or just handle
        owner.eq_ignore_ascii_case(handle) ||
        owner.strip_prefix('@').map(|s| s.eq_ignore_ascii_case(handle)).unwrap_or(false)
    }

    /// Check if an owner string matches any known user
    fn owner_is_known_user(&self, owner: &str) -> bool {
        let owner = owner.trim();
        self.store.users.iter().any(|u| {
            owner.eq_ignore_ascii_case(&u.handle) ||
            owner.strip_prefix('@').map(|s| s.eq_ignore_ascii_case(&u.handle)).unwrap_or(false)
        })
    }

    /// Show owned requirements in flat mode (direct children of user)
    fn show_owned_requirements_flat(&mut self, ui: &mut egui::Ui, owner_handle: &str) {
        // Collect matching requirements
        let matching: Vec<usize> = self.store.requirements.iter()
            .enumerate()
            .filter(|(_, r)| !r.archived || self.show_archived)
            .filter(|(_, r)| self.matches_user_owner(r, owner_handle))
            .filter(|(_, r)| self.passes_filters(r, true))
            .map(|(idx, _)| idx)
            .collect();

        for idx in matching {
            ui.horizontal(|ui| {
                ui.add_space(20.0); // Indent under user
                self.show_draggable_requirement_inline(ui, idx, false);
            });
        }
    }

    /// Show owned requirements in nested mode (preserving parent/child hierarchy)
    fn show_owned_requirements_nested(&mut self, ui: &mut egui::Ui, owner_handle: &str) {
        // Find all requirements owned by this user
        let owned_ids: HashSet<Uuid> = self.store.requirements.iter()
            .filter(|r| !r.archived || self.show_archived)
            .filter(|r| self.matches_user_owner(r, owner_handle))
            .map(|r| r.id)
            .collect();

        // Find root requirements for this owner (owned reqs that have no owned parent)
        let root_reqs: Vec<usize> = self.store.requirements.iter()
            .enumerate()
            .filter(|(_, r)| !r.archived || self.show_archived)
            .filter(|(_, r)| self.matches_user_owner(r, owner_handle))
            .filter(|(_, r)| self.passes_filters(r, true))
            .filter(|(_, r)| {
                // Check if any parent is also owned - if so, this is not a root
                !r.relationships.iter().any(|rel| {
                    rel.rel_type == RelationshipType::Parent &&
                    owned_ids.contains(&rel.target_id)
                })
            })
            .map(|(idx, _)| idx)
            .collect();

        for idx in root_reqs {
            self.show_owned_tree_node(ui, idx, &owned_ids, 1);
        }
    }

    /// Show a tree node within the owner's owned requirements
    fn show_owned_tree_node(&mut self, ui: &mut egui::Ui, idx: usize, owned_ids: &HashSet<Uuid>, depth: usize) {
        let Some(req) = self.store.requirements.get(idx) else {
            return;
        };

        let req_id = req.id;

        // Find children that are also owned
        let children: Vec<usize> = self.store.requirements.iter()
            .enumerate()
            .filter(|(_, r)| !r.archived || self.show_archived)
            .filter(|(_, r)| owned_ids.contains(&r.id))
            .filter(|(_, r)| {
                r.relationships.iter().any(|rel| {
                    rel.rel_type == RelationshipType::Parent && rel.target_id == req_id
                })
            })
            .map(|(idx, _)| idx)
            .collect();

        let has_children = !children.is_empty();
        let is_collapsed = self.tree_collapsed.get(&req_id).copied().unwrap_or(false);

        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 20.0);

            // Expand/collapse button or placeholder
            if has_children {
                let toggle_text = if is_collapsed { "▶" } else { "▼" };
                if ui.small_button(toggle_text).clicked() {
                    self.tree_collapsed.insert(req_id, !is_collapsed);
                }
            } else {
                ui.add_space(20.0); // Placeholder for alignment
            }

            self.show_draggable_requirement_inline(ui, idx, false);
        });

        // Show children if expanded
        if has_children && !is_collapsed {
            for child_idx in children {
                self.show_owned_tree_node(ui, child_idx, owned_ids, depth + 1);
            }
        }
    }

    /// Show unassigned requirements (no owner or unknown owner)
    fn show_unassigned_requirements(&mut self, ui: &mut egui::Ui) {
        let matching: Vec<usize> = self.store.requirements.iter()
            .enumerate()
            .filter(|(_, r)| !r.archived || self.show_archived)
            .filter(|(_, r)| r.owner.is_empty() || !self.owner_is_known_user(&r.owner))
            .filter(|(_, r)| self.passes_filters(r, true))
            .map(|(idx, _)| idx)
            .collect();

        for idx in matching {
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                self.show_draggable_requirement_inline(ui, idx, false);
            });
        }
    }

    fn show_tree_node(
        &mut self,
        ui: &mut egui::Ui,
        idx: usize,
        outgoing_rel_type: &RelationshipType,
        depth: usize,
    ) {
        // Call the version with no ancestors set (old behavior for backward compatibility)
        self.show_tree_node_with_ancestors(ui, idx, outgoing_rel_type, depth, &HashSet::new());
    }

    fn show_tree_node_with_ancestors(
        &mut self,
        ui: &mut egui::Ui,
        idx: usize,
        outgoing_rel_type: &RelationshipType,
        depth: usize,
        ancestors_to_show: &HashSet<Uuid>,
    ) {
        let Some(req) = self.store.requirements.get(idx) else {
            return;
        };

        let req_id = req.id;
        // Check if this item is dimmed (shown only because it's an ancestor of a matching item)
        let is_dimmed =
            ancestors_to_show.contains(&req_id) && !self.passes_filters(req, depth == 0);

        let children =
            self.get_children_with_ancestors(&req_id, outgoing_rel_type, ancestors_to_show);
        let has_children = !children.is_empty();

        let is_collapsed = self.tree_collapsed.get(&req_id).copied().unwrap_or(false);

        // Show expand/collapse button and requirement on same line
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 20.0);

            // Expand/collapse button or placeholder of same size
            let btn_size = egui::vec2(18.0, 18.0);
            if has_children {
                let btn_text = if is_collapsed { "+" } else { "-" };
                if ui
                    .add_sized(btn_size, egui::Button::new(btn_text))
                    .clicked()
                {
                    self.tree_collapsed.insert(req_id, !is_collapsed);
                }
            } else {
                // Placeholder with same size to maintain alignment
                ui.add_space(btn_size.x + 4.0); // Button width + spacing
            }

            // Show the draggable requirement inline (dimmed if it's an ancestor-only item)
            self.show_draggable_requirement_inline(ui, idx, is_dimmed);
        });

        // Show children if expanded
        if has_children && !is_collapsed {
            for child_idx in children {
                self.show_tree_node_with_ancestors(
                    ui,
                    child_idx,
                    outgoing_rel_type,
                    depth + 1,
                    ancestors_to_show,
                );
            }
        }
    }

    /// Render requirement item inline (without indent, for use in tree nodes)
    /// If `dimmed` is true, the item is shown greyed out (ancestor shown for context)
    fn show_draggable_requirement_inline(&mut self, ui: &mut egui::Ui, idx: usize, dimmed: bool) {
        let Some(req) = self.store.requirements.get(idx) else {
            return;
        };

        let req_id = req.id;
        let spec_id = req.spec_id.clone();
        let title = req.title.clone();
        let status_string = req.effective_status();
        let is_inactive = Self::is_inactive_status(&status_string);
        let icon = self.get_requirement_icon(req); // Get icon before req ref is dropped
        let selected = self.selected_idx == Some(idx);
        let is_drag_source = self.drag_source == Some(idx);
        let is_drop_target = self.drop_target == Some(idx);
        let should_scroll_to = self.scroll_to_requirement == Some(req_id);
        let show_status_icons = self.user_settings.show_status_icons;

        // Check if this requirement matches the search (for highlight mode only)
        // In Filter mode, all visible items are matches, so no highlighting needed
        let has_search_text = !self.filter_text.is_empty();
        let is_highlight_mode = self.user_settings.search_mode == SearchMode::Highlight;
        let is_search_match = if has_search_text && is_highlight_mode {
            self.matches_search(req)
        } else {
            false
        };
        // Only show current match highlighting in Highlight mode
        let is_current_match = has_search_text && is_highlight_mode && self.is_current_search_match(idx);

        // Build the label with optional status icon
        let label = if show_status_icons {
            format!("{} {} - {}", icon, spec_id.as_deref().unwrap_or("N/A"), title)
        } else {
            format!("{} - {}", spec_id.as_deref().unwrap_or("N/A"), title)
        };

        // Visual feedback for drag/drop state and search matches
        let (bg_color, stroke) = if is_drop_target {
            (
                egui::Color32::from_rgba_unmultiplied(100, 200, 100, 60),
                egui::Stroke::new(2.0, egui::Color32::GREEN),
            )
        } else if is_drag_source {
            (
                egui::Color32::from_rgba_unmultiplied(100, 100, 200, 60),
                egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE),
            )
        } else if selected && is_current_match {
            // Selected + current search match: bright orange highlight
            (
                egui::Color32::from_rgba_unmultiplied(255, 180, 0, 180),
                egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 150, 0)),
            )
        } else if selected {
            (ui.visuals().selection.bg_fill, egui::Stroke::NONE)
        } else if is_current_match {
            // Current search match (not selected): bright orange highlight
            (
                egui::Color32::from_rgba_unmultiplied(255, 180, 0, 150),
                egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 150, 0)),
            )
        } else if is_search_match {
            // Other search matches (Highlight mode only): visible yellow highlight
            (
                egui::Color32::from_rgba_unmultiplied(255, 255, 0, 100),
                egui::Stroke::new(1.0, egui::Color32::from_rgb(200, 200, 0)),
            )
        } else {
            (egui::Color32::TRANSPARENT, egui::Stroke::NONE)
        };

        // Calculate size for the label
        let text = egui::WidgetText::from(&label);
        let galley = text.into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        );
        let desired_size = galley.size() + egui::vec2(8.0, 4.0);

        let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());

        // Scroll to this item if requested (e.g., after adding a new requirement)
        if should_scroll_to {
            response.scroll_to_me(Some(egui::Align::Center));
            self.scroll_to_requirement = None;
        }

        // Paint background
        if bg_color != egui::Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, 2.0, bg_color);
        }
        if stroke != egui::Stroke::NONE {
            ui.painter().rect_stroke(rect, 2.0, stroke);
        }

        // Paint text - dimmed items use a lighter/greyed color
        let text_pos = rect.min + egui::vec2(4.0, 2.0);
        let text_color = if dimmed {
            // Greyed out for non-matching ancestors
            egui::Color32::from_gray(140)
        } else if selected {
            ui.visuals().selection.stroke.color
        } else if is_inactive {
            // Grey out completed/rejected items
            ui.visuals().weak_text_color()
        } else {
            ui.visuals().text_color()
        };
        ui.painter().galley(text_pos, galley, text_color);

        // Handle interactions (block when status popup is open to prevent click pass-through)
        if !self.quick_change_field.is_some() {
            if response.double_clicked() {
                // Double-click opens for editing
                eprintln!("DEBUG: Double-click on list item (line 9443)");
                self.selected_idx = Some(idx);
                self.focused_list = FocusedList::List1;
                self.load_form_from_requirement(idx);
                self.pending_view_change = Some(View::Edit);
                // Clear focus from any text fields to enable hotkeys
                ui.ctx().memory_mut(|mem| mem.surrender_focus(egui::Id::NULL));
            } else if response.clicked() {
                self.selected_idx = Some(idx);
                self.focused_list = FocusedList::List1;
                self.pending_view_change = Some(View::Detail);
                // Clear focus from any text fields to enable hotkeys
                ui.ctx().memory_mut(|mem| mem.surrender_focus(egui::Id::NULL));
            }
        }

        if response.drag_started() {
            self.drag_source = Some(idx);
        }

        // Check if this is a drop target using pointer position
        if self.drag_source.is_some() && self.drag_source != Some(idx) {
            if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                if rect.contains(pointer_pos) {
                    self.drop_target = Some(idx);
                }
            }
        }

        // Release is handled globally in the update loop

        // Show drag indicator while dragging
        if is_drag_source && ui.input(|i| i.pointer.is_decidedly_dragging()) {
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                egui::Area::new(egui::Id::new("drag_indicator_inline"))
                    .fixed_pos(pos + egui::vec2(10.0, 10.0))
                    .order(egui::Order::Tooltip)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(format!("📎 {}", spec_id.as_deref().unwrap_or("N/A")));
                        });
                    });
            }
        }
    }

    fn show_tree_node_bottom_up(
        &mut self,
        ui: &mut egui::Ui,
        idx: usize,
        outgoing_rel_type: &RelationshipType,
        depth: usize,
    ) {
        let Some(req) = self.store.requirements.get(idx) else {
            return;
        };

        let req_id = req.id;
        let parents = self.get_parents(&req_id, outgoing_rel_type);
        let has_parents = !parents.is_empty();

        let is_collapsed = self.tree_collapsed.get(&req_id).copied().unwrap_or(false);

        // Show expand/collapse button and requirement on same line
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 20.0);

            // Expand/collapse button or placeholder of same size
            let btn_size = egui::vec2(18.0, 18.0);
            if has_parents {
                let btn_text = if is_collapsed { "+" } else { "-" };
                if ui
                    .add_sized(btn_size, egui::Button::new(btn_text))
                    .clicked()
                {
                    self.tree_collapsed.insert(req_id, !is_collapsed);
                }
            } else {
                // Placeholder with same size to maintain alignment
                ui.add_space(btn_size.x + 4.0); // Button width + spacing
            }

            // Show the draggable requirement inline (not dimmed for bottom-up view for now)
            self.show_draggable_requirement_inline(ui, idx, false);
        });

        // Show parents if expanded (going up the tree)
        if has_parents && !is_collapsed {
            for parent_idx in parents {
                self.show_tree_node_bottom_up(ui, parent_idx, outgoing_rel_type, depth + 1);
            }
        }
    }

    /// Show split list content (for use in horizontal split layout)
    fn show_split_list_content(&mut self, ui: &mut egui::Ui) {
        // Header
        ui.horizontal(|ui| {
            ui.heading("Requirements (Split)");
        });
        ui.separator();

        // Search bar
        ui.horizontal(|ui| {
            ui.label("🔍");
            ui.add(
                egui::TextEdit::singleline(&mut self.split_filter_text)
                    .hint_text("Search...")
                    .desired_width(200.0),
            );
            // Clear button
            if !self.split_filter_text.is_empty() {
                if ui.small_button("✕").on_hover_text("Clear search").clicked() {
                    self.split_filter_text.clear();
                }
            }

            // Filter toggle
            let filter_active = !self.split_filter_types.is_empty()
                || !self.split_filter_features.is_empty();
            let filter_btn_text = if filter_active {
                "🔽 Filters ●"
            } else {
                "🔽 Filters"
            };
            if ui.button(filter_btn_text).clicked() {
                self.split_show_filter_panel = !self.split_show_filter_panel;
            }
        });

        // Perspective selector for split panel
        ui.horizontal(|ui| {
            ui.label("View:");
            egui::ComboBox::from_id_salt("split_perspective_combo_content")
                .selected_text(self.split_perspective.label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.split_perspective, Perspective::Flat, Perspective::Flat.label());
                    ui.selectable_value(&mut self.split_perspective, Perspective::ParentChild, Perspective::ParentChild.label());
                    ui.selectable_value(&mut self.split_perspective, Perspective::Verification, Perspective::Verification.label());
                    ui.selectable_value(&mut self.split_perspective, Perspective::References, Perspective::References.label());
                    ui.selectable_value(&mut self.split_perspective, Perspective::ByOwner, Perspective::ByOwner.label());
                });
        });
        ui.separator();

        // Scrollable list content
        egui::ScrollArea::vertical()
            .id_salt("split_list_content_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.show_split_list(ui);
            });
    }

    /// Show list panel with close button (for layouts where panel can be closed)
    fn show_list_panel_with_close(&mut self, ctx: &egui::Context, in_form_view: bool) {
        // Get screen width to calculate 50% default
        let screen_width = ctx.screen_rect().width();
        let default_width = (screen_width / 2.0).max(200.0);

        egui::SidePanel::left("list_panel")
            .min_width(150.0)
            .default_width(default_width)
            .resizable(true)
            .show(ctx, |ui| {
                // Set clip rect to prevent content overflow
                let clip_rect = ui.available_rect_before_wrap();
                ui.set_clip_rect(clip_rect);

                // Header with close button (only if we have more than just this panel)
                ui.horizontal(|ui| {
                    ui.heading("Requirements");
                    // Only show close button if we're in a split layout
                    if self.layout_mode.has_split() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✕").on_hover_text("Close this list").clicked() {
                                self.layout_mode = self.layout_mode.without_split();
                            }
                        });
                    } else if in_form_view {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("▶ Hide").on_hover_text("Hide requirements list").clicked() {
                                self.left_panel_collapsed = true;
                            }
                        });
                    }
                });
                ui.separator();

                // Rest of list content (search, filters, etc.)
                self.show_list_panel_content(ui);
            });
    }

    /// Show simplified list panel for form views (matches Detail View layout)
    /// Only shows search + filter button, no perspective/preset controls
    fn show_list_panel_simple(&mut self, ctx: &egui::Context) {
        // Calculate max width as 50% of screen to match Detail View proportions
        let screen_width = ctx.screen_rect().width();
        let max_panel_width = (screen_width * 0.5).max(350.0);

        // Use same panel ID as Detail View ("list_panel") for seamless width transition
        egui::SidePanel::left("list_panel")
            .min_width(150.0)
            .default_width(350.0)
            .max_width(max_panel_width)
            .resizable(true)
            .show(ctx, |ui| {
                // Set clip rect to prevent content overflow
                let clip_rect = ui.available_rect_before_wrap();
                ui.set_clip_rect(clip_rect);

                // Header with Hide button
                ui.horizontal(|ui| {
                    ui.heading("Requirements");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("▶ Hide")
                            .on_hover_text("Hide requirements list")
                            .clicked()
                        {
                            self.left_panel_collapsed = true;
                        }
                    });
                });
                ui.separator();

                // Simple search bar (matching Detail View)
                ui.horizontal(|ui| {
                    self.show_search_bar(ui, 120.0, true);

                    // Filter toggle button
                    let filter_active = !self.filter_types.is_empty()
                        || !self.filter_features.is_empty()
                        || !self.filter_prefixes.is_empty()
                        || !self.filter_statuses.is_empty()
                        || !self.filter_priorities.is_empty();
                    let filter_btn_text = if filter_active { "🔽 ●" } else { "🔽" };
                    if ui.button(filter_btn_text).on_hover_text("Filters").clicked() {
                        self.show_filter_dialog_list1 = !self.show_filter_dialog_list1;
                    }
                });
                ui.separator();

                // Scrollable list
                egui::ScrollArea::vertical()
                    .id_salt("list_panel_simple_scroll")
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width());
                        self.show_tree_list(ui);
                    });
            });
    }

    /// Show split panel with close button
    fn show_split_panel_with_close(&mut self, ctx: &egui::Context, forced_width: Option<f32>) {
        let mut panel = egui::SidePanel::left("split_panel")
            .min_width(200.0)
            .default_width(350.0);

        if let Some(width) = forced_width {
            panel = panel.exact_width(width);
        }

        panel.show(ctx, |ui| {
            // Header with close button
            ui.horizontal(|ui| {
                ui.heading("Requirements");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✕").on_hover_text("Close this list").clicked() {
                        self.layout_mode = self.layout_mode.without_split();
                    }
                });
            });
            ui.separator();

            // Split panel content
            self.show_split_panel_content(ui);
        });
    }

    /// Show list content with close button (for stacked layouts)
    fn show_list_content_with_close(&mut self, ui: &mut egui::Ui, _in_form_view: bool) {
        // Header with close button (only if we're in a split layout)
        ui.horizontal(|ui| {
            ui.heading("Requirements");
            if self.layout_mode.has_split() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✕").on_hover_text("Close this list").clicked() {
                        self.layout_mode = self.layout_mode.without_split();
                    }
                });
            }
        });
        ui.separator();

        // Search bar and rest of list content
        self.show_list_panel_content(ui);
    }

    /// Show list content in a column (for side-by-side splits)
    fn show_list_content_in_column(&mut self, ui: &mut egui::Ui, _in_form_view: bool) {
        // Header with close button
        ui.horizontal(|ui| {
            ui.heading("List 1");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("✕").on_hover_text("Close this list").clicked() {
                    self.layout_mode = self.layout_mode.without_split();
                }
            });
        });
        ui.separator();

        // Search bar with filter button
        ui.horizontal(|ui| {
            self.show_search_bar(ui, 120.0, true);

            // Filter toggle button
            let filter_active = !self.filter_types.is_empty()
                || !self.filter_features.is_empty()
                || !self.filter_prefixes.is_empty()
                || !self.filter_statuses.is_empty()
                || !self.filter_priorities.is_empty();
            let filter_btn_text = if filter_active { "🔽 ●" } else { "🔽" };
            if ui.button(filter_btn_text).on_hover_text("Filters").clicked() {
                self.show_filter_dialog_list1 = !self.show_filter_dialog_list1;
            }
        });

        // Scrollable list
        egui::ScrollArea::vertical()
            .id_salt("list_column_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.show_tree_list(ui);
            });
    }

    /// Show split list content in a column (for side-by-side splits)
    fn show_split_list_content_in_column(&mut self, ui: &mut egui::Ui) {
        // Header with lock and close buttons
        ui.horizontal(|ui| {
            ui.heading("List 2");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("✕").on_hover_text("Close this list").clicked() {
                    self.layout_mode = self.layout_mode.without_split();
                }
                // Navigation lock toggle
                let lock_icon = if self.navigation_locked { "🔒" } else { "🔓" };
                let lock_tooltip = if self.navigation_locked {
                    "Navigation locked: lists scroll together. Click to unlock."
                } else {
                    "Navigation unlocked: lists scroll independently. Click to lock."
                };
                if ui.button(lock_icon).on_hover_text(lock_tooltip).clicked() {
                    self.navigation_locked = !self.navigation_locked;
                    if self.navigation_locked {
                        // When locking, trigger scroll to center
                        self.scroll_to_center = true;
                    }
                }
            });
        });
        ui.separator();

        // Search bar with filter button
        ui.horizontal(|ui| {
            ui.label("🔍");
            ui.add(
                egui::TextEdit::singleline(&mut self.split_filter_text)
                    .hint_text("Search...")
                    .desired_width(120.0),
            );
            // Clear button
            if !self.split_filter_text.is_empty() {
                if ui.small_button("✕").on_hover_text("Clear search").clicked() {
                    self.split_filter_text.clear();
                }
            }

            // Filter toggle button
            let filter_active = !self.split_filter_types.is_empty()
                || !self.split_filter_features.is_empty()
                || !self.split_filter_prefixes.is_empty()
                || !self.split_filter_statuses.is_empty()
                || !self.split_filter_priorities.is_empty();
            let filter_btn_text = if filter_active { "🔽 ●" } else { "🔽" };
            if ui.button(filter_btn_text).on_hover_text("Filters").clicked() {
                self.show_filter_dialog_list2 = !self.show_filter_dialog_list2;
            }
        });

        // Scrollable list
        egui::ScrollArea::vertical()
            .id_salt("split_column_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.show_split_list(ui);
            });
    }

    /// Show detail view with close button
    fn show_detail_view_with_close(&mut self, ui: &mut egui::Ui) {
        // Pass show_close flag and stacked orientation to show_detail_view_internal
        let stacked = self.layout_mode.is_details_stacked();
        self.show_detail_view_internal(ui, self.layout_mode.has_details(), stacked);
    }

    /// Helper to show list panel content (search, filters, list)
    fn show_list_panel_content(&mut self, ui: &mut egui::Ui) {
        // Search bar
        ui.horizontal(|ui| {
            self.show_search_bar(ui, 150.0, true);

            // Filter toggle button
            let filter_active = !self.filter_types.is_empty() || !self.filter_features.is_empty()
                || !self.filter_prefixes.is_empty() || !self.filter_statuses.is_empty()
                || !self.filter_priorities.is_empty();
            let filter_btn_text = if filter_active { "🔽 ●" } else { "🔽" };
            if ui.button(filter_btn_text).on_hover_text("Filters").clicked() {
                self.show_filter_dialog_list1 = !self.show_filter_dialog_list1;
            }
        });

        // Scrollable list - vertical only to prevent content from expanding panel width
        egui::ScrollArea::vertical()
            .id_salt("list_panel_scroll")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show(ui, |ui| {
                // Constrain content width to prevent horizontal expansion
                ui.set_max_width(ui.available_width());
                self.show_tree_list(ui);
            });
    }

    /// Helper to show split panel content (search, filters, list)
    fn show_split_panel_content(&mut self, ui: &mut egui::Ui) {
        // Search bar
        ui.horizontal(|ui| {
            ui.label("🔍");
            ui.add(
                egui::TextEdit::singleline(&mut self.split_filter_text)
                    .hint_text("Search...")
                    .desired_width(120.0),
            );
            // Clear button
            if !self.split_filter_text.is_empty() {
                if ui.small_button("✕").on_hover_text("Clear search").clicked() {
                    self.split_filter_text.clear();
                }
            }

            // Filter toggle
            let filter_active = !self.split_filter_types.is_empty() || !self.split_filter_features.is_empty()
                || !self.split_filter_prefixes.is_empty() || !self.split_filter_statuses.is_empty()
                || !self.split_filter_priorities.is_empty();
            let filter_btn_text = if filter_active { "🔽 ●" } else { "🔽" };
            if ui.button(filter_btn_text).on_hover_text("Filters").clicked() {
                self.show_filter_dialog_list2 = !self.show_filter_dialog_list2;
            }
        });

        // Scrollable list
        egui::ScrollArea::vertical()
            .id_salt("split_panel_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.show_split_list(ui);
            });
    }

    /// Show layout menu popup (triggered by long-press on layout button)
    fn show_layout_menu_popup(&mut self, ctx: &egui::Context) {
        if !self.show_layout_menu {
            return;
        }

        // Get the button rect for positioning
        let button_rect = match self.layout_button_rect {
            Some(rect) => rect,
            None => return,
        };

        // Create a popup area below the button
        let popup_pos = egui::pos2(button_rect.left(), button_rect.bottom() + 2.0);

        egui::Area::new(egui::Id::new("layout_menu_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(popup_pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .show(ui, |ui| {
                        ui.set_min_width(200.0);
                        for mode in LayoutMode::all_modes() {
                            let is_current = *mode == self.layout_mode;
                            let label = if is_current {
                                format!("✓ {}", mode.label())
                            } else {
                                format!("   {}", mode.label())
                            };
                            if ui.selectable_label(is_current, label).clicked() {
                                self.layout_mode = *mode;
                                self.show_layout_menu = false;
                            }
                        }
                    });
            });

        // Close menu on click outside
        if ctx.input(|i| i.pointer.any_click()) {
            // Check if click was inside the popup area - approximate bounds check
            let popup_rect = egui::Rect::from_min_size(
                popup_pos,
                egui::vec2(210.0, 120.0), // Approximate popup size
            );
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                if !popup_rect.contains(pos) && !button_rect.contains(pos) {
                    self.show_layout_menu = false;
                }
            }
        }
    }

    /// Show quick change popup for status, priority, or owner (triggered by 's'/'p'/'o' key in list view)
    fn show_quick_change_popup(&mut self, ctx: &egui::Context) {
        let field = match self.quick_change_field {
            Some(f) => f,
            None => return,
        };

        // Owner field uses a different UI with fuzzy search
        if field == QuickChangeField::Owner {
            self.show_owner_picker_popup(ctx);
            return;
        }

        // Sprint field uses its own popup with sprint list
        if field == QuickChangeField::Sprint {
            self.show_sprint_picker_popup(ctx);
            return;
        }

        // Feature field uses a different UI with fuzzy search
        if field == QuickChangeField::Feature {
            self.show_feature_picker_popup(ctx);
            return;
        }

        // Get the target requirement to determine its type
        let target_req = self.quick_change_target_id
            .and_then(|id| self.store.requirements.iter().find(|r| r.id == id));

        // Find the type definition for this requirement
        let type_def = target_req.and_then(|req| {
            let type_name = req.req_type.to_string();
            self.store.type_definitions.iter().find(|t| t.name == type_name)
        });

        // Get the options based on field type and type definition
        let (title, options): (&str, Vec<String>) = match field {
            QuickChangeField::Status => {
                let statuses = type_def
                    .map(|t| t.get_statuses())
                    .unwrap_or_else(|| vec![
                        "Draft".to_string(),
                        "Approved".to_string(),
                        "Completed".to_string(),
                        "Rejected".to_string(),
                    ]);
                ("Change Status", statuses)
            }
            QuickChangeField::Priority => {
                let priorities = type_def
                    .map(|t| t.get_priorities())
                    .unwrap_or_else(|| vec![
                        "High".to_string(),
                        "Medium".to_string(),
                        "Low".to_string(),
                    ]);
                ("Change Priority", priorities)
            }
            QuickChangeField::Owner => unreachable!(),
            QuickChangeField::Sprint => unreachable!(), // Handled separately via show_sprint_picker_popup
            QuickChangeField::Feature => unreachable!(), // Handled separately via show_feature_picker_popup
        };
        let num_options = options.len();

        // Handle keyboard input for the popup
        let mut close_popup = false;
        let mut apply_change = false;

        ctx.input(|i| {
            // Escape to close
            if i.key_pressed(egui::Key::Escape) {
                close_popup = true;
            }
            // Enter to apply
            if i.key_pressed(egui::Key::Enter) {
                apply_change = true;
            }
            // Up/Down or j/k to navigate
            if i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::K) {
                if self.quick_change_selected > 0 {
                    self.quick_change_selected -= 1;
                } else {
                    self.quick_change_selected = num_options - 1; // Wrap to bottom
                }
            }
            if i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::J) {
                if self.quick_change_selected < num_options - 1 {
                    self.quick_change_selected += 1;
                } else {
                    self.quick_change_selected = 0; // Wrap to top
                }
            }
        });

        if close_popup {
            self.quick_change_field = None;
            self.quick_change_target_id = None;
            self.quick_change_consumed_action = true;
            return;
        }

        // Apply the change
        if apply_change {
            self.apply_quick_change(field, self.quick_change_selected);
            self.quick_change_field = None;
            self.quick_change_target_id = None;
            self.quick_change_consumed_action = true;
            return;
        }

        // Center the popup on screen
        let screen_rect = ctx.screen_rect();
        let popup_size = egui::vec2(180.0, 130.0);
        let popup_pos = egui::pos2(
            (screen_rect.width() - popup_size.x) / 2.0,
            (screen_rect.height() - popup_size.y) / 2.0,
        );

        let popup_id = match field {
            QuickChangeField::Status => "quick_change_status_popup",
            QuickChangeField::Priority => "quick_change_priority_popup",
            QuickChangeField::Owner => unreachable!(),
            QuickChangeField::Sprint => unreachable!(), // Handled separately
            QuickChangeField::Feature => unreachable!(), // Handled separately
        };

        egui::Area::new(egui::Id::new(popup_id))
            .order(egui::Order::Foreground)
            .fixed_pos(popup_pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_min_width(popup_size.x - 16.0);
                        ui.label(egui::RichText::new(title).strong());
                        ui.separator();

                        for (idx, option) in options.iter().enumerate() {
                            let is_selected = idx == self.quick_change_selected;

                            let response = ui.selectable_label(is_selected, option);
                            if response.clicked() {
                                self.quick_change_selected = idx;
                                // Apply immediately on click
                                self.apply_quick_change(field, idx);
                                self.quick_change_field = None;
                                self.quick_change_target_id = None;
                                self.quick_change_consumed_action = true;
                            }
                        }

                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.small("↑↓/jk Navigate  Enter Select  Esc Cancel");
                        });
                    });
            });

        // Close on click outside
        if ctx.input(|i| i.pointer.any_click()) {
            let popup_rect = egui::Rect::from_min_size(popup_pos, popup_size);
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                if !popup_rect.contains(pos) {
                    self.quick_change_field = None;
                    self.quick_change_target_id = None;
                    self.quick_change_consumed_action = true;
                }
            }
        }
    }

    /// Show owner picker popup with fuzzy search (triggered by 'o' key in list view)
    fn show_owner_picker_popup(&mut self, ctx: &egui::Context) {
        // Collect all unique owners from requirements + users
        let mut all_owners: Vec<String> = self.store.users.iter()
            .filter(|u| !u.archived)
            .map(|u| u.handle.clone())
            .collect();

        // Add any owners that aren't users
        for req in &self.store.requirements {
            if !req.owner.is_empty() && !all_owners.contains(&req.owner) {
                all_owners.push(req.owner.clone());
            }
        }
        all_owners.sort();

        // Filter based on search text
        let search_lower = self.quick_change_owner_search.to_lowercase();
        let filtered_owners: Vec<&String> = if search_lower.is_empty() {
            all_owners.iter().collect()
        } else {
            all_owners.iter()
                .filter(|o| o.to_lowercase().contains(&search_lower))
                .collect()
        };

        let num_options = filtered_owners.len();

        // Handle keyboard input for the popup
        let mut close_popup = false;
        let mut apply_change = false;

        ctx.input(|i| {
            // Escape to close
            if i.key_pressed(egui::Key::Escape) {
                close_popup = true;
            }
            // Enter to apply (if we have options)
            if i.key_pressed(egui::Key::Enter) && num_options > 0 {
                apply_change = true;
            }
            // Up/Down to navigate
            if i.key_pressed(egui::Key::ArrowUp) {
                if self.quick_change_selected > 0 {
                    self.quick_change_selected -= 1;
                } else if num_options > 0 {
                    self.quick_change_selected = num_options - 1; // Wrap to bottom
                }
            }
            if i.key_pressed(egui::Key::ArrowDown) {
                if num_options > 0 {
                    if self.quick_change_selected < num_options - 1 {
                        self.quick_change_selected += 1;
                    } else {
                        self.quick_change_selected = 0; // Wrap to top
                    }
                }
            }
        });

        if close_popup {
            self.quick_change_field = None;
            self.quick_change_target_id = None;
            self.quick_change_owner_search.clear();
            self.quick_change_consumed_action = true;
            return;
        }

        // Apply the change
        if apply_change {
            if let Some(owner) = filtered_owners.get(self.quick_change_selected) {
                self.apply_owner_change((*owner).clone());
            }
            self.quick_change_field = None;
            self.quick_change_target_id = None;
            self.quick_change_owner_search.clear();
            self.quick_change_consumed_action = true;
            return;
        }

        // Ensure selected index is valid after filtering
        if self.quick_change_selected >= num_options && num_options > 0 {
            self.quick_change_selected = num_options - 1;
        }

        // Center the popup on screen
        let screen_rect = ctx.screen_rect();
        let popup_size = egui::vec2(250.0, 220.0);
        let popup_pos = egui::pos2(
            (screen_rect.width() - popup_size.x) / 2.0,
            (screen_rect.height() - popup_size.y) / 2.0,
        );

        egui::Area::new(egui::Id::new("quick_change_owner_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(popup_pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_min_width(popup_size.x - 16.0);
                        ui.label(egui::RichText::new("Assign Owner").strong());
                        ui.separator();

                        // Search input
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.quick_change_owner_search)
                                .hint_text("Type to search...")
                                .desired_width(ui.available_width())
                        );
                        // Auto-focus the search field
                        response.request_focus();

                        ui.separator();

                        // Scrollable list of owners
                        egui::ScrollArea::vertical()
                            .max_height(120.0)
                            .show(ui, |ui| {
                                if filtered_owners.is_empty() {
                                    ui.weak("No matching owners");
                                } else {
                                    for (idx, owner) in filtered_owners.iter().enumerate() {
                                        let is_selected = idx == self.quick_change_selected;
                                        let response = ui.selectable_label(is_selected, *owner);
                                        if response.clicked() {
                                            self.apply_owner_change((*owner).clone());
                                            self.quick_change_field = None;
                                            self.quick_change_target_id = None;
                                            self.quick_change_owner_search.clear();
                                            self.quick_change_consumed_action = true;
                                        }
                                    }
                                }
                            });

                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.small("↑↓ Navigate  Enter Select  Esc Cancel");
                        });
                    });
            });

        // Close on click outside
        if ctx.input(|i| i.pointer.any_click()) {
            let popup_rect = egui::Rect::from_min_size(popup_pos, popup_size);
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                if !popup_rect.contains(pos) {
                    self.quick_change_field = None;
                    self.quick_change_target_id = None;
                    self.quick_change_owner_search.clear();
                    self.quick_change_consumed_action = true;
                }
            }
        }
    }

    /// Show sprint picker popup for assigning requirement to sprint
    fn show_sprint_picker_popup(&mut self, ctx: &egui::Context) {
        // Get available sprints (active only)
        let sprints: Vec<(Uuid, String, String)> = self.store.get_sprints()
            .into_iter()
            .filter(|s| {
                let status = s.effective_status();
                status != "Completed" && status != "Archived"
            })
            .map(|s| (s.id, s.spec_id.clone().unwrap_or_default(), s.title.clone()))
            .collect();

        // Include "Backlog" as first option (None = remove from sprint)
        let num_options = sprints.len() + 1; // +1 for Backlog option

        // Handle keyboard input for the popup
        let mut close_popup = false;
        let mut apply_change = false;

        ctx.input(|i| {
            // Escape to close
            if i.key_pressed(egui::Key::Escape) {
                close_popup = true;
            }
            // Enter to apply (if we have options)
            if i.key_pressed(egui::Key::Enter) && num_options > 0 {
                apply_change = true;
            }
            // Up/Down to navigate
            if i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::K) {
                if self.quick_change_selected > 0 {
                    self.quick_change_selected -= 1;
                } else if num_options > 0 {
                    self.quick_change_selected = num_options - 1; // Wrap to bottom
                }
            }
            if i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::J) {
                if num_options > 0 {
                    if self.quick_change_selected < num_options - 1 {
                        self.quick_change_selected += 1;
                    } else {
                        self.quick_change_selected = 0; // Wrap to top
                    }
                }
            }
        });

        if close_popup {
            self.quick_change_field = None;
            self.quick_change_target_id = None;
            self.quick_change_consumed_action = true;
            return;
        }

        // Apply the change
        if apply_change {
            if self.quick_change_selected == 0 {
                // Move to Backlog (remove from sprint)
                if let Some(target_id) = self.quick_change_target_id {
                    let username = self.user_settings.display_name();
                    self.store.remove_from_sprint(target_id, &username);
                    self.save();
                }
            } else if let Some((sprint_id, _, _)) = sprints.get(self.quick_change_selected - 1) {
                // Assign to selected sprint
                if let Some(target_id) = self.quick_change_target_id {
                    let username = self.user_settings.display_name();
                    self.store.assign_to_sprint(target_id, *sprint_id, &username);
                    self.save();
                }
            }
            self.quick_change_field = None;
            self.quick_change_target_id = None;
            self.quick_change_consumed_action = true;
            return;
        }

        // Ensure selected index is valid
        if self.quick_change_selected >= num_options && num_options > 0 {
            self.quick_change_selected = num_options - 1;
        }

        // Center the popup on screen
        let screen_rect = ctx.screen_rect();
        let popup_size = egui::vec2(280.0, 220.0);
        let popup_pos = egui::pos2(
            (screen_rect.width() - popup_size.x) / 2.0,
            (screen_rect.height() - popup_size.y) / 2.0,
        );

        egui::Area::new(egui::Id::new("quick_change_sprint_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(popup_pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_min_width(popup_size.x - 16.0);

                        ui.heading("🏃 Assign to Sprint");
                        ui.separator();

                        // Scrollable list of sprints
                        egui::ScrollArea::vertical()
                            .max_height(140.0)
                            .show(ui, |ui| {
                                // Backlog option first
                                let is_selected = self.quick_change_selected == 0;
                                let response = ui.selectable_label(is_selected, "📦 Backlog (no sprint)");
                                if response.clicked() {
                                    if let Some(target_id) = self.quick_change_target_id {
                                        let username = self.user_settings.display_name();
                                        self.store.remove_from_sprint(target_id, &username);
                                        self.save();
                                    }
                                    self.quick_change_field = None;
                                    self.quick_change_target_id = None;
                                    self.quick_change_consumed_action = true;
                                }

                                // Sprint options
                                if sprints.is_empty() {
                                    ui.weak("No active sprints available");
                                } else {
                                    for (idx, (sprint_id, spec_id, title)) in sprints.iter().enumerate() {
                                        let is_selected = idx + 1 == self.quick_change_selected;
                                        let label = format!("{}: {}", spec_id, title);
                                        let response = ui.selectable_label(is_selected, label);
                                        if response.clicked() {
                                            if let Some(target_id) = self.quick_change_target_id {
                                                let username = self.user_settings.display_name();
                                                self.store.assign_to_sprint(target_id, *sprint_id, &username);
                                                self.save();
                                            }
                                            self.quick_change_field = None;
                                            self.quick_change_target_id = None;
                                            self.quick_change_consumed_action = true;
                                        }
                                    }
                                }
                            });

                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.small("↑↓/jk Navigate  Enter Select  Esc Cancel");
                        });
                    });
            });

        // Close on click outside
        if ctx.input(|i| i.pointer.any_click()) {
            let popup_rect = egui::Rect::from_min_size(popup_pos, popup_size);
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                if !popup_rect.contains(pos) {
                    self.quick_change_field = None;
                    self.quick_change_target_id = None;
                    self.quick_change_consumed_action = true;
                }
            }
        }
    }

    /// Apply owner change to the selected requirement
    fn apply_owner_change(&mut self, new_owner: String) {
        let Some(target_id) = self.quick_change_target_id else { return };
        let Some(req) = self.store.requirements.iter_mut().find(|r| r.id == target_id) else { return };

        let old_owner = req.owner.clone();
        if old_owner != new_owner {
            req.owner = new_owner.clone();
            req.modified_at = chrono::Utc::now();
            req.history.push(aida_core::models::HistoryEntry {
                id: uuid::Uuid::new_v4(),
                author: self.user_settings.name.clone(),
                timestamp: chrono::Utc::now(),
                changes: vec![aida_core::models::FieldChange {
                    field_name: "owner".to_string(),
                    old_value: old_owner,
                    new_value: new_owner,
                }],
            });
            if let Err(e) = self.storage.save(&self.store) {
                eprintln!("Failed to save after owner change: {}", e);
            }
        }
    }

    /// Show feature picker popup for assigning feature to requirement
    fn show_feature_picker_popup(&mut self, ctx: &egui::Context) {
        // Collect all unique features from requirements
        let all_features = self.get_all_features();

        // Filter based on search text
        let search_lower = self.quick_change_feature_search.to_lowercase();
        let filtered_features: Vec<&String> = if search_lower.is_empty() {
            all_features.iter().collect()
        } else {
            all_features.iter()
                .filter(|f| f.to_lowercase().contains(&search_lower))
                .collect()
        };

        let num_options = filtered_features.len();

        // Handle keyboard input for the popup
        let mut close_popup = false;
        let mut apply_change = false;

        ctx.input(|i| {
            // Escape to close
            if i.key_pressed(egui::Key::Escape) {
                close_popup = true;
            }
            // Enter to apply (if we have options)
            if i.key_pressed(egui::Key::Enter) && num_options > 0 {
                apply_change = true;
            }
            // Up/Down to navigate
            if i.key_pressed(egui::Key::ArrowUp) {
                if self.quick_change_selected > 0 {
                    self.quick_change_selected -= 1;
                } else if num_options > 0 {
                    self.quick_change_selected = num_options - 1; // Wrap to bottom
                }
            }
            if i.key_pressed(egui::Key::ArrowDown) {
                if num_options > 0 {
                    if self.quick_change_selected < num_options - 1 {
                        self.quick_change_selected += 1;
                    } else {
                        self.quick_change_selected = 0; // Wrap to top
                    }
                }
            }
        });

        if close_popup {
            self.quick_change_field = None;
            self.quick_change_target_id = None;
            self.quick_change_feature_search.clear();
            self.quick_change_consumed_action = true;
            return;
        }

        // Apply the change
        if apply_change {
            if let Some(feature) = filtered_features.get(self.quick_change_selected) {
                self.apply_feature_change((*feature).clone());
            }
            self.quick_change_field = None;
            self.quick_change_target_id = None;
            self.quick_change_feature_search.clear();
            self.quick_change_consumed_action = true;
            return;
        }

        // Ensure selected index is valid after filtering
        if self.quick_change_selected >= num_options && num_options > 0 {
            self.quick_change_selected = num_options - 1;
        }

        // Center the popup on screen
        let screen_rect = ctx.screen_rect();
        let popup_size = egui::vec2(300.0, 400.0);
        let popup_pos = egui::pos2(
            (screen_rect.width() - popup_size.x) / 2.0,
            (screen_rect.height() - popup_size.y) / 2.0,
        );

        egui::Area::new(egui::Id::new("quick_change_feature_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(popup_pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_min_width(popup_size.x - 16.0);
                        ui.label(egui::RichText::new("Assign Feature").strong());
                        ui.separator();

                        // Search input
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.quick_change_feature_search)
                                .hint_text("Type to search...")
                                .desired_width(ui.available_width())
                        );
                        // Auto-focus the search field
                        response.request_focus();

                        ui.separator();

                        // Scrollable list of features
                        let scroll_id = egui::Id::new("feature_picker_scroll");
                        egui::ScrollArea::vertical()
                            .id_salt(scroll_id)
                            .max_height(300.0)
                            .show(ui, |ui| {
                                if filtered_features.is_empty() {
                                    ui.weak("No matching features");
                                } else {
                                    for (idx, feature) in filtered_features.iter().enumerate() {
                                        let is_selected = idx == self.quick_change_selected;
                                        let response = ui.selectable_label(is_selected, *feature);

                                        // Scroll to the selected item when navigating
                                        if is_selected {
                                            response.scroll_to_me(Some(egui::Align::Center));
                                        }

                                        if response.clicked() {
                                            self.apply_feature_change((*feature).clone());
                                            self.quick_change_field = None;
                                            self.quick_change_target_id = None;
                                            self.quick_change_feature_search.clear();
                                            self.quick_change_consumed_action = true;
                                        }
                                    }
                                }
                            });

                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.small("↑↓ Navigate  Enter Select  Esc Cancel");
                        });
                    });
            });

        // Close on click outside
        if ctx.input(|i| i.pointer.any_click()) {
            let popup_rect = egui::Rect::from_min_size(popup_pos, popup_size);
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                if !popup_rect.contains(pos) {
                    self.quick_change_field = None;
                    self.quick_change_target_id = None;
                    self.quick_change_feature_search.clear();
                    self.quick_change_consumed_action = true;
                }
            }
        }
    }

    /// Apply feature change to the selected requirement
    fn apply_feature_change(&mut self, new_feature: String) {
        let Some(target_id) = self.quick_change_target_id else { return };
        let Some(req) = self.store.requirements.iter_mut().find(|r| r.id == target_id) else { return };

        let old_feature = req.feature.clone();
        if old_feature != new_feature {
            req.feature = new_feature.clone();
            req.modified_at = chrono::Utc::now();
            req.history.push(aida_core::models::HistoryEntry {
                id: uuid::Uuid::new_v4(),
                author: self.user_settings.name.clone(),
                timestamp: chrono::Utc::now(),
                changes: vec![aida_core::models::FieldChange {
                    field_name: "feature".to_string(),
                    old_value: old_feature,
                    new_value: new_feature,
                }],
            });
            if let Err(e) = self.storage.save(&self.store) {
                eprintln!("Failed to save after feature change: {}", e);
            }
        }
    }

    /// Apply a quick change to the selected requirement
    fn apply_quick_change(&mut self, field: QuickChangeField, selected_idx: usize) {
        let Some(target_id) = self.quick_change_target_id else { return };

        // Get the requirement's type to look up type-specific options
        let req_type_name = self.store.requirements.iter()
            .find(|r| r.id == target_id)
            .map(|r| r.req_type.to_string());

        // Find the type definition for this requirement's type
        let type_def = req_type_name.as_ref().and_then(|type_name| {
            self.store.type_definitions.iter().find(|t| t.name == *type_name)
        });

        let Some(req) = self.store.requirements.iter_mut().find(|r| r.id == target_id) else { return };

        match field {
            QuickChangeField::Status => {
                // Get type-specific statuses or default
                let statuses = type_def
                    .map(|t| t.get_statuses())
                    .unwrap_or_else(|| vec![
                        "Draft".to_string(),
                        "Approved".to_string(),
                        "Completed".to_string(),
                        "Rejected".to_string(),
                    ]);

                let new_status_str = statuses.get(selected_idx)
                    .cloned()
                    .unwrap_or_else(|| "Draft".to_string());
                let old_status_str = req.effective_status();

                if old_status_str != new_status_str {
                    req.set_status_from_str(&new_status_str);
                    req.modified_at = chrono::Utc::now();
                    req.history.push(aida_core::models::HistoryEntry {
                        id: uuid::Uuid::new_v4(),
                        author: self.user_settings.name.clone(),
                        timestamp: chrono::Utc::now(),
                        changes: vec![aida_core::models::FieldChange {
                            field_name: "status".to_string(),
                            old_value: old_status_str,
                            new_value: new_status_str,
                        }],
                    });
                    if let Err(e) = self.storage.save(&self.store) {
                        eprintln!("Failed to save after status change: {}", e);
                    }
                }
            }
            QuickChangeField::Priority => {
                // Get type-specific priorities or default
                let priorities = type_def
                    .map(|t| t.get_priorities())
                    .unwrap_or_else(|| vec![
                        "High".to_string(),
                        "Medium".to_string(),
                        "Low".to_string(),
                    ]);

                let new_priority_str = priorities.get(selected_idx)
                    .cloned()
                    .unwrap_or_else(|| "Medium".to_string());
                let old_priority_str = req.effective_priority();

                if old_priority_str != new_priority_str {
                    req.set_priority_from_str(&new_priority_str);
                    req.modified_at = chrono::Utc::now();
                    req.history.push(aida_core::models::HistoryEntry {
                        id: uuid::Uuid::new_v4(),
                        author: self.user_settings.name.clone(),
                        timestamp: chrono::Utc::now(),
                        changes: vec![aida_core::models::FieldChange {
                            field_name: "priority".to_string(),
                            old_value: old_priority_str,
                            new_value: new_priority_str,
                        }],
                    });
                    if let Err(e) = self.storage.save(&self.store) {
                        eprintln!("Failed to save after priority change: {}", e);
                    }
                }
            }
            QuickChangeField::Owner => {
                // Owner is handled separately via apply_owner_change
                // This branch should never be reached
            }
            QuickChangeField::Sprint => {
                // Sprint is handled separately via show_sprint_picker_popup
                // This branch should never be reached
            }
            QuickChangeField::Feature => {
                // Feature is handled separately via show_feature_picker_popup
                // This branch should never be reached
            }
        }
    }

    fn show_detail_view(&mut self, ui: &mut egui::Ui) {
        self.show_detail_view_internal(ui, false, false);
    }

    fn show_detail_view_internal(&mut self, ui: &mut egui::Ui, show_close: bool, stacked: bool) {
        // Track if close was clicked
        let mut close_details = false;

        if let Some(idx) = self.selected_idx {
            if let Some(req) = self.store.requirements.get(idx).cloned() {
                // Buttons need mutable access, so handle them separately
                let mut load_edit = false;
                let mut delete_req = false;
                let mut toggle_archive = false;
                let is_archived = req.archived;

                // Track actions from Quick Actions menu
                let mut new_priority: Option<RequirementPriority> = None;
                let mut new_status: Option<RequirementStatus> = None;
                let mut trigger_ai_action: Option<AiAction> = None;
                let mut copy_for_claude_code_idx: Option<usize> = None;
                let mut clone_req = false;
                let current_priority = req.priority.clone();
                let current_status = req.status.clone();

                // Get title bar styling from theme
                let title_bar_bg = self.user_settings.theme.title_bar_bg();
                let title_bar_text = self.user_settings.theme.title_bar_text();
                let title_bar_font_size = self.user_settings.theme.title_bar_font_size();

                // Title bar with styled background
                egui::Frame::none()
                    .fill(title_bar_bg)
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                    .rounding(2.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Reserve space for buttons: Actions (~90px) + Edit (~60px) + Close (~30px) + spacing
                            let buttons_width = if show_close { 220.0 } else { 180.0 };
                            let available_width = ui.available_width();
                            let title_max_width = (available_width - buttons_width).max(100.0);

                            // Title text with truncation to ensure buttons remain visible
                            ui.allocate_ui_with_layout(
                                egui::vec2(title_max_width, ui.available_height()),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_clip_rect(ui.available_rect_before_wrap());
                                    let title_text = egui::RichText::new(&req.title)
                                        .size(18.0 * title_bar_font_size)
                                        .strong();
                                    let title_text = if let Some(color) = title_bar_text {
                                        title_text.color(color)
                                    } else {
                                        title_text
                                    };
                                    ui.add(egui::Label::new(title_text).truncate());

                                    if is_archived {
                                        ui.label("(Archived)");
                                    }
                                },
                            );

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                // Close button (rightmost)
                                if show_close {
                                    if ui.button("✕").on_hover_text("Close details").clicked() {
                                        close_details = true;
                                    }
                                }

                                if ui.button("✏ Edit").clicked() {
                                    load_edit = true;
                                }

                                // Quick Actions dropdown menu
                                ui.menu_button("⚡ Actions", |ui| {
                                    // Priority submenu
                                    ui.menu_button("Priority", |ui| {
                                        let priorities = [
                                            RequirementPriority::High,
                                            RequirementPriority::Medium,
                                            RequirementPriority::Low,
                                        ];
                                        for priority in priorities {
                                            let label = if priority == current_priority {
                                                format!("✓ {}", priority)
                                            } else {
                                                format!("  {}", priority)
                                            };
                                            if ui.button(label).clicked() {
                                                new_priority = Some(priority);
                                                ui.close_menu();
                                            }
                                        }
                                    });

                                    // Status submenu
                                    ui.menu_button("Status", |ui| {
                                        let statuses = [
                                            RequirementStatus::Draft,
                                            RequirementStatus::Approved,
                                            RequirementStatus::Completed,
                                            RequirementStatus::Rejected,
                                        ];
                                        for status in statuses {
                                            let label = if status == current_status {
                                                format!("✓ {}", status)
                                            } else {
                                                format!("  {}", status)
                                            };
                                            if ui.button(label).clicked() {
                                                new_status = Some(status);
                                                ui.close_menu();
                                            }
                                        }
                                    });

                                    ui.separator();

                                    // AI Actions submenu
                                    let req_uuid = req.id;
                                    ui.menu_button("🤖 AI", |ui| {
                                        if ui.button("📊 Evaluate Requirement  (ae)").clicked() {
                                            trigger_ai_action = Some(AiAction::Evaluate(req_uuid));
                                            ui.close_menu();
                                        }

                                        if ui.button("🔍 Find Duplicates       (ad)").clicked() {
                                            trigger_ai_action = Some(AiAction::FindDuplicates(req_uuid));
                                            ui.close_menu();
                                        }

                                        if ui.button("🔗 Suggest Relationships (ar)").clicked() {
                                            trigger_ai_action = Some(AiAction::SuggestRelationships(req_uuid));
                                            ui.close_menu();
                                        }

                                        ui.separator();

                                        if ui.button("✨ Improve Description   (ai)").clicked() {
                                            trigger_ai_action = Some(AiAction::ImproveDescription(req_uuid));
                                            ui.close_menu();
                                        }

                                        if ui.button("📝 Generate Children     (ag)").clicked() {
                                            trigger_ai_action = Some(AiAction::GenerateChildren(req_uuid));
                                            ui.close_menu();
                                        }

                                        ui.separator();

                                        // Copy for Claude Code - only enable if status is Approved
                                        let is_approved = req.status == RequirementStatus::Approved;
                                        let copy_button = egui::Button::new("📋 Copy for Claude Code");
                                        let response = ui.add_enabled(is_approved, copy_button);
                                        let response = if !is_approved {
                                            response.on_disabled_hover_text("Requirement must be Approved to implement")
                                        } else {
                                            response
                                        };
                                        if response.clicked() {
                                            copy_for_claude_code_idx = Some(idx);
                                            ui.close_menu();
                                        }
                                    });

                                    ui.separator();

                                    // Clone action
                                    if ui.button("📋 Clone").clicked() {
                                        clone_req = true;
                                        ui.close_menu();
                                    }

                                    // Archive/Unarchive action
                                    let archive_label = if is_archived {
                                        "↩ Unarchive"
                                    } else {
                                        "📁 Archive"
                                    };
                                    if ui.button(archive_label).clicked() {
                                        toggle_archive = true;
                                        ui.close_menu();
                                    }

                                    // Delete action
                                    if ui.button("🗑 Delete").clicked() {
                                        delete_req = true;
                                        ui.close_menu();
                                    }
                                });
                            });
                        });
                    });

                if load_edit {
                    eprintln!("DEBUG: Context menu Edit (line 10259)");
                    self.load_form_from_requirement(idx);
                    self.pending_view_change = Some(View::Edit);
                }
                if delete_req {
                    self.pending_delete = Some(idx);
                }
                if toggle_archive {
                    self.toggle_archive(idx);
                }
                if clone_req {
                    self.show_clone_dialog = true;
                    self.clone_source_idx = Some(idx);
                }

                // Apply priority change
                if let Some(priority) = new_priority {
                    if let Some(req) = self.store.requirements.get_mut(idx) {
                        let req_id = req.id;
                        let old_priority = req.priority.clone();
                        req.priority = priority.clone();
                        let change = Requirement::field_change(
                            "priority",
                            old_priority.to_string(),
                            priority.to_string(),
                        );
                        req.record_change(self.user_settings.display_name(), vec![change]);
                        self.mark_requirement_modified(req_id);
                        self.save();
                    }
                }

                // Apply status change
                if let Some(status) = new_status {
                    if let Some(req) = self.store.requirements.get_mut(idx) {
                        let req_id = req.id;
                        let old_status = req.status.clone();
                        req.status = status.clone();
                        let change = Requirement::field_change(
                            "status",
                            old_status.to_string(),
                            status.to_string(),
                        );
                        req.record_change(self.user_settings.display_name(), vec![change]);
                        self.mark_requirement_modified(req_id);
                        self.save();
                    }
                }

                // Handle copy for Claude Code
                if let Some(copy_idx) = copy_for_claude_code_idx {
                    if let Some(req) = self.store.requirements.get(copy_idx) {
                        let prompt_text = format_requirement_for_claude_code(req);
                        ui.ctx().copy_text(prompt_text.clone());
                        // Also copy to primary selection on Linux
                        copy_to_primary_selection(&prompt_text);
                        let req_id = req.spec_id.as_deref().unwrap_or("requirement");
                        self.toast_message = Some(ToastNotification {
                            message: format!("Copied {} to clipboard - paste into Claude Code", req_id),
                            is_success: true,
                            show_until: Instant::now() + std::time::Duration::from_secs(4),
                        });
                    }
                }

                // Handle AI action trigger
                if let Some(action) = trigger_ai_action {
                    eprintln!("AI action triggered: {:?}", action);

                    // Check if AI is available
                    if !self.ai_client.is_available() {
                        self.ai_last_result = Some(AiResult {
                            action_name: action.name().to_string(),
                            success: false,
                            message: format!(
                                "AI not available. Mode: {}. Install Claude CLI or set ANTHROPIC_API_KEY.",
                                self.ai_client.mode_description()
                            ),
                            details: None,
                        });
                        self.show_ai_results_panel = true;
                    } else {
                        // Get the requirement UUID from the action
                        let req_uuid = action.requirement_id();

                        // Look up the requirement
                        if let Some(req) = self.store.requirements.iter().find(|r| r.id == req_uuid) {
                            let req = req.clone(); // Clone to avoid borrow issues
                            let result = match &action {
                                AiAction::Evaluate(_) => {
                                    // Check if already evaluating
                                    if self.ai_eval_in_progress.is_some() {
                                        self.toast_message = Some(ToastNotification {
                                            message: "AI evaluation already in progress...".to_string(),
                                            is_success: false,
                                            show_until: Instant::now() + std::time::Duration::from_secs(3),
                                        });
                                        None // Skip, return no result
                                    } else {
                                        // Spawn background thread for evaluation
                                        let (tx, rx) = mpsc::channel();
                                        let ai_client = self.ai_client.clone();
                                        let store_clone = self.store.clone();
                                        let req_clone = req.clone();
                                        let req_id = req.id;
                                        let spec_id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
                                        let content_hash = req.content_hash();

                                        self.ai_eval_receiver = Some(rx);
                                        self.ai_eval_in_progress = Some((req_id, spec_id.clone()));

                                        // Show toast that evaluation started
                                        self.toast_message = Some(ToastNotification {
                                            message: format!("Evaluating {}...", spec_id),
                                            is_success: true,
                                            show_until: Instant::now() + std::time::Duration::from_secs(3),
                                        });

                                        thread::spawn(move || {
                                            let result = match ai_client.evaluate_requirement(&req_clone, &store_clone) {
                                                Ok(response) => Ok(response),
                                                Err(e) => Err(e.to_string()),
                                            };
                                            let _ = tx.send(BackgroundAiResult {
                                                req_id,
                                                spec_id,
                                                content_hash,
                                                result,
                                            });
                                        });

                                        None // No immediate result, will be polled later
                                    }
                                },
                                AiAction::FindDuplicates(_) => {
                                    // Check if already finding duplicates
                                    if self.find_duplicates_in_progress.is_some() {
                                        self.toast_message = Some(ToastNotification {
                                            message: "Find duplicates already in progress...".to_string(),
                                            is_success: false,
                                            show_until: Instant::now() + std::time::Duration::from_secs(3),
                                        });
                                        None // Skip, return no result
                                    } else {
                                        // Spawn background thread for find duplicates
                                        let (tx, rx) = mpsc::channel();
                                        let ai_client = self.ai_client.clone();
                                        let store_clone = self.store.clone();
                                        let req_clone = req.clone();
                                        let req_id = req.id;
                                        let spec_id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());

                                        self.find_duplicates_receiver = Some(rx);
                                        self.find_duplicates_in_progress = Some((req_id, spec_id.clone()));

                                        // Show toast that find duplicates started
                                        self.toast_message = Some(ToastNotification {
                                            message: format!("Finding duplicates for {}...", spec_id),
                                            is_success: true,
                                            show_until: Instant::now() + std::time::Duration::from_secs(3),
                                        });

                                        thread::spawn(move || {
                                            let result = match ai_client.find_duplicates(&req_clone, &store_clone) {
                                                Ok(response) => {
                                                    let duplicates: Vec<DuplicateInfo> = response.potential_duplicates
                                                        .into_iter()
                                                        .map(|d| DuplicateInfo {
                                                            spec_id: d.spec_id,
                                                            similarity: d.similarity,
                                                            reason: d.reason,
                                                            recommendation: d.recommendation,
                                                        })
                                                        .collect();
                                                    Ok(duplicates)
                                                },
                                                Err(e) => Err(e.to_string()),
                                            };
                                            let _ = tx.send(BackgroundFindDuplicatesResult {
                                                req_id,
                                                spec_id,
                                                result,
                                            });
                                        });

                                        None // No immediate result, will be polled later
                                    }
                                },
                                AiAction::SuggestRelationships(_) => {
                                    Some(match self.ai_client.suggest_relationships(&req, &self.store) {
                                        Ok(response) => {
                                            let msg = if response.suggested_relationships.is_empty() {
                                                "No relationship suggestions found.".to_string()
                                            } else {
                                                response.suggested_relationships.iter()
                                                    .map(|r| format!(
                                                        "• {} → {} ({}% confidence)\n  {}",
                                                        r.rel_type,
                                                        r.target_spec_id,
                                                        (r.confidence * 100.0) as i32,
                                                        r.rationale
                                                    ))
                                                    .collect::<Vec<_>>()
                                                    .join("\n\n")
                                            };
                                            AiResult {
                                                action_name: "Suggest Relationships".to_string(),
                                                success: true,
                                                message: msg,
                                                details: None,
                                            }
                                        },
                                        Err(e) => AiResult {
                                            action_name: "Suggest Relationships".to_string(),
                                            success: false,
                                            message: format!("AI error: {}", e),
                                            details: None,
                                        },
                                    })
                                },
                                AiAction::ImproveDescription(_) => {
                                    Some(match self.ai_client.improve_description(&req, &self.store) {
                                        Ok(response) => AiResult {
                                            action_name: "Improve Description".to_string(),
                                            success: true,
                                            message: format!(
                                                "Improved Description:\n\n{}\n\nChanges Made:\n{}\n\nRationale: {}",
                                                response.improved_description,
                                                response.changes_made.iter()
                                                    .map(|c| format!("• {}", c))
                                                    .collect::<Vec<_>>()
                                                    .join("\n"),
                                                response.rationale
                                            ),
                                            details: Some(response.improved_description.clone()),
                                        },
                                        Err(e) => AiResult {
                                            action_name: "Improve Description".to_string(),
                                            success: false,
                                            message: format!("AI error: {}", e),
                                            details: None,
                                        },
                                    })
                                },
                                AiAction::GenerateChildren(_) => {
                                    Some(match self.ai_client.generate_children(&req, &self.store) {
                                        Ok(response) => {
                                            let msg = if response.suggested_children.is_empty() {
                                                "No child requirements suggested.".to_string()
                                            } else {
                                                response.suggested_children.iter()
                                                    .map(|c| format!(
                                                        "• {} [{}]\n  {}\n  Rationale: {}",
                                                        c.title,
                                                        c.req_type,
                                                        c.description,
                                                        c.rationale
                                                    ))
                                                    .collect::<Vec<_>>()
                                                    .join("\n\n")
                                            };
                                            AiResult {
                                                action_name: "Generate Children".to_string(),
                                                success: true,
                                                message: msg,
                                                details: None,
                                            }
                                        },
                                        Err(e) => AiResult {
                                            action_name: "Generate Children".to_string(),
                                            success: false,
                                            message: format!("AI error: {}", e),
                                            details: None,
                                        },
                                    })
                                },
                            };
                            // Only set result and show panel if we got an immediate result
                            if let Some(result) = result {
                                self.ai_last_result = Some(result);
                                self.show_ai_results_panel = true;
                            }
                        } else {
                            self.ai_last_result = Some(AiResult {
                                action_name: action.name().to_string(),
                                success: false,
                                message: format!("Requirement not found: {}", req_uuid),
                                details: None,
                            });
                            self.show_ai_results_panel = true;
                        }
                    }
                }

                ui.separator();

                if stacked {
                    // Stacked layout: horizontal split with fields on left (25%), tabs on right (75%)
                    let req_id = req.id;
                    let available_width = ui.available_width();
                    let default_left_width = available_width * 0.25; // 25% for metadata, 75% for tabs

                    // LEFT SIDE: Metadata fields in a resizable panel
                    egui::SidePanel::left("detail_metadata_panel")
                        .resizable(true)
                        .default_width(default_left_width)
                        .min_width(150.0)
                        .max_width(available_width * 0.5)
                        .show_inside(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("detail_metadata_scroll")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    egui::Grid::new("detail_grid_stacked")
                                        .num_columns(2)
                                        .spacing([10.0, 6.0])
                                        .striped(true)
                                        .show(ui, |ui| {
                                            ui.label("ID:");
                                            ui.label(req.spec_id.as_deref().unwrap_or("N/A"));
                                            ui.end_row();

                                            ui.label("Type:");
                                            ui.label(format!("{:?}", req.req_type));
                                            ui.end_row();

                                            // Only show status/priority for stateful types
                                            if !self.store.is_type_stateless(&req.req_type) {
                                                ui.label("Status:");
                                                ui.label(format!("{:?}", req.status));
                                                ui.end_row();

                                                ui.label("Priority:");
                                                ui.label(format!("{:?}", req.priority));
                                                ui.end_row();
                                            }

                                            ui.label("Feature:");
                                            ui.label(&req.feature);
                                            ui.end_row();

                                            ui.label("Owner:");
                                            ui.label(&req.owner);
                                            ui.end_row();

                                            if !req.tags.is_empty() {
                                                ui.label("Tags:");
                                                let tags_vec: Vec<String> = req.tags.iter().cloned().collect();
                                                ui.label(tags_vec.join(", "));
                                                ui.end_row();
                                            }

                                            if let Some(ref created_by) = req.created_by {
                                                ui.label("Created By:");
                                                ui.label(created_by);
                                                ui.end_row();
                                            }

                                            ui.label("Created:");
                                            ui.label(req.created_at.format("%Y-%m-%d %H:%M").to_string());
                                            ui.end_row();

                                            ui.label("Modified:");
                                            ui.label(req.modified_at.format("%Y-%m-%d %H:%M").to_string());
                                            ui.end_row();
                                        });
                                });
                        });

                    // RIGHT SIDE: Tabs and tab content (takes remaining space)
                    egui::CentralPanel::default()
                        .frame(egui::Frame::none())
                        .show_inside(ui, |ui| {
                            // Tabs
                            ui.horizontal(|ui| {
                                // AI tab with indicator showing evaluation status
                                let ai_label = if req.ai_evaluation.is_some() {
                                    if req.needs_ai_evaluation() {
                                        "🤖 AI ⚠"  // Stale evaluation
                                    } else {
                                        "🤖 AI ✓"  // Fresh evaluation
                                    }
                                } else {
                                    "🤖 AI"  // No evaluation yet
                                };
                                ui.selectable_value(
                                    &mut self.active_tab,
                                    DetailTab::Ai,
                                    ai_label,
                                );
                                ui.selectable_value(
                                    &mut self.active_tab,
                                    DetailTab::Description,
                                    "📄 Description",
                                );
                                ui.selectable_value(
                                    &mut self.active_tab,
                                    DetailTab::Comments,
                                    format!("💬 Comments ({})", req.comments.len()),
                                );
                                ui.selectable_value(
                                    &mut self.active_tab,
                                    DetailTab::Links,
                                    format!("🔗 Links ({})", req.relationships.len() + req.urls.len()),
                                );
                                ui.selectable_value(
                                    &mut self.active_tab,
                                    DetailTab::History,
                                    format!("📜 History ({})", req.history.len()),
                                );
                            });

                            ui.separator();

                            // Tab content
                            egui::ScrollArea::vertical()
                                .id_salt("detail_tab_content_scroll")
                                .auto_shrink([false, false])
                                .show(ui, |ui| match &self.active_tab {
                                    DetailTab::Ai => {
                                        self.show_ai_tab(ui, &req, req_id);
                                    }
                                    DetailTab::Description => {
                                        self.show_description_tab(ui, &req, idx);
                                    }
                                    DetailTab::Comments => {
                                        self.show_comments_tab(ui, &req, idx);
                                    }
                                    DetailTab::Links => {
                                        self.show_links_tab(ui, &req, req_id);
                                    }
                                    DetailTab::History => {
                                        self.show_history_tab(ui, &req);
                                    }
                                });
                        });
                } else {
                    // Side layout (vertical): metadata at top, tabs below
                    // Metadata grid (always shown)
                    egui::Grid::new("detail_grid")
                        .num_columns(2)
                        .spacing([40.0, 8.0])
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label("ID:");
                            ui.label(req.spec_id.as_deref().unwrap_or("N/A"));
                            ui.end_row();

                            ui.label("Type:");
                            ui.label(format!("{:?}", req.req_type));
                            ui.end_row();

                            // Only show status/priority for stateful types
                            if !self.store.is_type_stateless(&req.req_type) {
                                ui.label("Status:");
                                ui.label(format!("{:?}", req.status));
                                ui.end_row();

                                ui.label("Priority:");
                                ui.label(format!("{:?}", req.priority));
                                ui.end_row();
                            }

                            ui.label("Feature:");
                            ui.label(&req.feature);
                            ui.end_row();

                            ui.label("Owner:");
                            ui.label(&req.owner);
                            ui.end_row();

                            if !req.tags.is_empty() {
                                ui.label("Tags:");
                                let tags_vec: Vec<String> = req.tags.iter().cloned().collect();
                                ui.label(tags_vec.join(", "));
                                ui.end_row();
                            }

                            // Created by and timestamps
                            if let Some(ref created_by) = req.created_by {
                                ui.label("Created By:");
                                ui.label(created_by);
                                ui.end_row();
                            }

                            ui.label("Created:");
                            ui.label(req.created_at.format("%Y-%m-%d %H:%M").to_string());
                            ui.end_row();

                            ui.label("Modified:");
                            ui.label(req.modified_at.format("%Y-%m-%d %H:%M").to_string());
                            ui.end_row();
                        });

                    ui.separator();

                    // Tabbed content
                    ui.horizontal(|ui| {
                        // AI tab with indicator for evaluation status
                        let ai_label = if req.ai_evaluation.is_some() {
                            if req.needs_ai_evaluation() {
                                "🤖 AI ⚠️" // Has evaluation but stale
                            } else {
                                "🤖 AI ✓" // Has current evaluation
                            }
                        } else {
                            "🤖 AI" // No evaluation yet
                        };
                        ui.selectable_value(&mut self.active_tab, DetailTab::Ai, ai_label);
                        ui.selectable_value(
                            &mut self.active_tab,
                            DetailTab::Description,
                            "📄 Description",
                        );
                        ui.selectable_value(
                            &mut self.active_tab,
                            DetailTab::Comments,
                            format!("💬 Comments ({})", req.comments.len()),
                        );
                        ui.selectable_value(
                            &mut self.active_tab,
                            DetailTab::Links,
                            format!("🔗 Links ({})", req.relationships.len() + req.urls.len()),
                        );
                        ui.selectable_value(
                            &mut self.active_tab,
                            DetailTab::History,
                            format!("📜 History ({})", req.history.len()),
                        );
                    });

                    ui.separator();

                    // Tab content
                    let req_id = req.id;
                    egui::ScrollArea::vertical().show(ui, |ui| match &self.active_tab {
                        DetailTab::Ai => {
                            self.show_ai_tab(ui, &req, req_id);
                        }
                        DetailTab::Description => {
                            self.show_description_tab(ui, &req, idx);
                        }
                        DetailTab::Comments => {
                            self.show_comments_tab(ui, &req, idx);
                        }
                        DetailTab::Links => {
                            self.show_links_tab(ui, &req, req_id);
                        }
                        DetailTab::History => {
                            self.show_history_tab(ui, &req);
                        }
                    });
                }

                // Double-click on background area to edit (when no modal is open)
                if !self.show_url_form
                    && !self.show_reference_picker
                    && !self.show_add_comment
                {
                    #[allow(deprecated)]
                    let bg_response = ui.interact_bg(egui::Sense::click());
                    if bg_response.double_clicked() {
                        eprintln!("DEBUG: Background double-click (line 10529)");
                        self.load_form_from_requirement(idx);
                        self.pending_view_change = Some(View::Edit);
                    }
                }
            }
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.heading("Select a requirement from the list");
            });
        }

        // Handle close button click (must be done outside the nested closures)
        if close_details {
            self.layout_mode = self.layout_mode.without_details();
        }
    }

    fn show_ai_tab(&mut self, ui: &mut egui::Ui, req: &Requirement, req_id: Uuid) {
        ui.heading("AI Evaluation");
        ui.add_space(10.0);

        if let Some(ref stored_eval) = req.ai_evaluation {
            let eval = &stored_eval.evaluation;
            let is_stale = req.needs_ai_evaluation();

            // Show staleness warning
            if is_stale {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("⚠️ Evaluation may be outdated")
                            .color(egui::Color32::from_rgb(255, 165, 0)),
                    );
                    ui.label("- requirement has been modified since last evaluation");
                });
                ui.add_space(5.0);
            }

            // Evaluation timestamp
            ui.horizontal(|ui| {
                ui.label("Evaluated:");
                ui.label(
                    stored_eval
                        .evaluated_at
                        .format("%Y-%m-%d %H:%M")
                        .to_string(),
                );
            });
            ui.add_space(10.0);

            // Quality Score with visual indicator
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Quality Score:").strong());
                let score = eval.quality_score;
                let score_color = if score >= 8 {
                    egui::Color32::from_rgb(76, 175, 80) // Green
                } else if score >= 5 {
                    egui::Color32::from_rgb(255, 193, 7) // Amber
                } else {
                    egui::Color32::from_rgb(244, 67, 54) // Red
                };
                ui.label(
                    egui::RichText::new(format!("{}/10", score))
                        .color(score_color)
                        .strong()
                        .size(18.0),
                );

                // Visual bar
                let bar_width = 100.0;
                let filled_width = bar_width * (score as f32 / 10.0);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(bar_width, 12.0), egui::Sense::hover());
                let painter = ui.painter();
                painter.rect_filled(rect, 3.0, egui::Color32::from_gray(60));
                let filled_rect =
                    egui::Rect::from_min_size(rect.min, egui::vec2(filled_width, 12.0));
                painter.rect_filled(filled_rect, 3.0, score_color);
            });

            ui.add_space(15.0);

            // Strengths section
            if !eval.strengths.is_empty() {
                ui.collapsing(
                    egui::RichText::new(format!("✅ Strengths ({})", eval.strengths.len()))
                        .strong(),
                    |ui| {
                        for strength in &eval.strengths {
                            ui.horizontal(|ui| {
                                ui.label("•");
                                ui.label(strength);
                            });
                        }
                    },
                );
                ui.add_space(10.0);
            }

            // Issues section
            if !eval.issues.is_empty() {
                ui.collapsing(
                    egui::RichText::new(format!("⚠️ Issues ({})", eval.issues.len())).strong(),
                    |ui| {
                        for issue in &eval.issues {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    // Severity indicator
                                    let severity_color = match issue.severity.to_lowercase().as_str()
                                    {
                                        "high" | "critical" => egui::Color32::from_rgb(244, 67, 54),
                                        "medium" => egui::Color32::from_rgb(255, 152, 0),
                                        _ => egui::Color32::from_rgb(255, 235, 59),
                                    };
                                    ui.label(
                                        egui::RichText::new(&issue.severity)
                                            .color(severity_color)
                                            .strong(),
                                    );
                                    ui.label(format!("[{}]", issue.issue_type));
                                });
                                ui.label(&issue.text);
                                if !issue.suggestion.is_empty() {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("Suggestion:").italics());
                                        ui.label(&issue.suggestion);
                                    });
                                }
                            });
                            ui.add_space(5.0);
                        }
                    },
                );
                ui.add_space(10.0);
            }

            // Suggested improvements section
            if let Some(ref improvement) = eval.suggested_improvements {
                ui.collapsing(
                    egui::RichText::new("💡 Suggested Improvements").strong(),
                    |ui| {
                        if let Some(ref desc) = improvement.description {
                            ui.label(egui::RichText::new("Improved description:").strong());
                            ui.add_space(5.0);
                            egui::Frame::default()
                                .fill(ui.visuals().extreme_bg_color)
                                .inner_margin(8.0)
                                .rounding(4.0)
                                .show(ui, |ui| {
                                    ui.label(desc);
                                });
                            ui.add_space(5.0);
                            // Apply button to replace description with AI suggestion
                            if ui
                                .button("✓ Apply Improved Description")
                                .on_hover_text("Replace the current description with the AI-suggested improvement")
                                .clicked()
                            {
                                self.pending_apply_ai_description = Some((req_id, desc.clone()));
                            }
                            ui.add_space(10.0);
                        }
                        ui.label(egui::RichText::new("Rationale:").italics());
                        ui.label(&improvement.rationale);
                    },
                );
            }
        } else {
            // No evaluation yet
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    egui::RichText::new("🤖 No AI evaluation yet")
                        .size(16.0)
                        .color(egui::Color32::GRAY),
                );
                ui.add_space(10.0);
                ui.label("This requirement has not been evaluated by AI.");
                ui.label("Evaluations run automatically in the background,");
                ui.label("or you can trigger one manually from Actions → AI → Evaluate.");
            });
        }
    }

    fn show_description_tab(&mut self, ui: &mut egui::Ui, req: &Requirement, idx: usize) {
        ui.horizontal(|ui| {
            ui.heading("Description");
            ui.label("(double-click to edit)")
                .on_hover_text("Double-click anywhere on the description to edit it");
        });
        ui.add_space(10.0);

        // Wrap description in a frame that detects double-click
        let frame_response = egui::Frame::none().show(ui, |ui| {
            // Make the frame respond to clicks
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), ui.available_height()),
                egui::Sense::click(),
            );

            // Render description as markdown inside the allocated area
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
                CommonMarkViewer::new().show(ui, &mut self.markdown_cache, &req.description);
            });

            response
        });

        // Check for double-click on the description area
        if frame_response.inner.double_clicked() {
            eprintln!("DEBUG: Description double-click (line 10574)");
            self.load_form_from_requirement(idx);
            self.focus_description = true;
            self.pending_view_change = Some(View::Edit);
        }
    }

    fn show_comments_tab(&mut self, ui: &mut egui::Ui, req: &Requirement, idx: usize) {
        ui.horizontal(|ui| {
            ui.heading("Comments");
            if ui.button("➕ Add Comment").clicked() {
                self.show_add_comment = true;
                self.reply_to_comment = None;
                // Pre-fill author from user settings
                self.comment_author = self.user_settings.display_name();
                self.comment_content.clear();
            }
        });

        ui.add_space(10.0);

        if self.show_add_comment {
            self.show_comment_form(ui, idx);
        }

        if req.comments.is_empty() {
            ui.label("No comments yet");
        } else {
            for comment in &req.comments {
                self.show_comment_tree(ui, comment, idx, 0);
            }
        }
    }

    fn show_links_tab(&mut self, ui: &mut egui::Ui, req: &Requirement, req_id: Uuid) {
        // Show URL form modal if active
        if self.show_url_form {
            self.show_url_form_modal(ui, req_id);
            return;
        }

        // Show reference picker modal if active
        if self.show_reference_picker {
            self.show_reference_picker_modal(ui, req_id);
            return;
        }

        // URLs section
        ui.horizontal(|ui| {
            ui.heading("External Links");
            if ui.button("➕ New URL").clicked() {
                self.editing_url_id = None;
                self.url_form_url.clear();
                self.url_form_title.clear();
                self.url_form_description.clear();
                self.url_verification_status = None;
                self.url_verification_in_progress = false;
                self.show_url_form = true;
            }
        });
        ui.add_space(5.0);

        if req.urls.is_empty() {
            ui.label("No external links");
        } else {
            let urls = req.urls.clone();
            let mut url_to_remove: Option<Uuid> = None;
            let mut url_to_edit: Option<Uuid> = None;

            for url_link in &urls {
                ui.horizontal(|ui| {
                    // Remove button
                    if ui.small_button("x").on_hover_text("Remove link").clicked() {
                        url_to_remove = Some(url_link.id);
                    }

                    // Edit button
                    if ui.small_button("✏").on_hover_text("Edit link").clicked() {
                        url_to_edit = Some(url_link.id);
                    }

                    // Verification status indicator
                    if let Some(ok) = url_link.last_verified_ok {
                        if ok {
                            ui.label("✅").on_hover_text(format!(
                                "Verified: {}",
                                url_link
                                    .last_verified
                                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                                    .unwrap_or_default()
                            ));
                        } else {
                            ui.label("❌").on_hover_text("Last verification failed");
                        }
                    }

                    // Clickable link
                    let link_text = if url_link.title.is_empty() {
                        &url_link.url
                    } else {
                        &url_link.title
                    };

                    if ui.link(link_text).on_hover_text(&url_link.url).clicked() {
                        if let Err(e) = open::that(&url_link.url) {
                            self.message = Some((format!("Failed to open URL: {}", e), true));
                        }
                    }

                    // Show description if present
                    if let Some(ref desc) = url_link.description {
                        ui.label(format!("- {}", desc));
                    }
                });
            }

            // Handle edit
            if let Some(id) = url_to_edit {
                if let Some(url_link) = urls.iter().find(|u| u.id == id) {
                    self.editing_url_id = Some(id);
                    self.url_form_url = url_link.url.clone();
                    self.url_form_title = url_link.title.clone();
                    self.url_form_description = url_link.description.clone().unwrap_or_default();
                    self.url_verification_status = None;
                    self.url_verification_in_progress = false;
                    self.show_url_form = true;
                }
            }

            // Handle remove
            if let Some(id) = url_to_remove {
                if let Some(idx) = self.selected_idx {
                    if let Some(req) = self.store.requirements.get_mut(idx) {
                        req.urls.retain(|u| u.id != id);
                        self.save();
                        self.message = Some(("URL removed".to_string(), false));
                    }
                }
            }
        }

        ui.add_space(15.0);
        ui.separator();
        ui.add_space(10.0);

        // Relationships section
        ui.horizontal(|ui| {
            ui.heading("Relationships");
            if ui.button("➕ Reference").clicked() {
                self.reference_picker_search.clear();
                self.reference_picker_selected = None;
                self.show_reference_picker = true;
            }
            ui.add_space(20.0);
            ui.checkbox(&mut self.show_recursive_relationships, "Recursive")
                .on_hover_text(
                    "Show relationships as a tree, recursively expanding related requirements",
                );
        });
        ui.add_space(10.0);

        if req.relationships.is_empty() {
            ui.label("No relationships defined");
        } else if self.show_recursive_relationships {
            // Recursive tree view
            self.show_relationships_tree(ui, req_id, 0, &mut Vec::new(), None);
        } else {
            // Immediate relationships view (original behavior)
            self.show_immediate_relationships(ui, req_id);
        }
    }

    /// Show immediate (non-recursive) relationships for a requirement
    fn show_immediate_relationships(&mut self, ui: &mut egui::Ui, req_id: Uuid) {
        let Some(req) = self.store.requirements.iter().find(|r| r.id == req_id) else {
            return;
        };

        // Collect relationship info first to avoid borrow issues
        let rel_info: Vec<_> = req
            .relationships
            .iter()
            .map(|rel| {
                let target_idx = self
                    .store
                    .requirements
                    .iter()
                    .position(|r| r.id == rel.target_id);
                let target_label = self
                    .store
                    .requirements
                    .iter()
                    .find(|r| r.id == rel.target_id)
                    .and_then(|r| r.spec_id.as_ref())
                    .cloned()
                    .unwrap_or_else(|| "Unknown".to_string());
                let target_title = self
                    .store
                    .requirements
                    .iter()
                    .find(|r| r.id == rel.target_id)
                    .map(|r| r.title.clone())
                    .unwrap_or_else(|| "(not found)".to_string());

                // Get display name and color from relationship definition
                let (display_name, color) = self
                    .store
                    .get_definition_for_type(&rel.rel_type)
                    .map(|def| (def.display_name.clone(), def.color.clone()))
                    .unwrap_or_else(|| (format!("{}", rel.rel_type), None));

                // Get created_by and created_at for tooltip
                let rel_created_by = rel.created_by.clone();
                let rel_created_at = rel.created_at;

                (
                    rel.rel_type.clone(),
                    rel.target_id,
                    target_idx,
                    target_label,
                    target_title,
                    display_name,
                    color,
                    rel_created_by,
                    rel_created_at,
                )
            })
            .collect();

        let mut relationship_to_remove: Option<(RelationshipType, Uuid)> = None;

        for (rel_type, target_id, target_idx, target_label, target_title, display_name, color, rel_created_by, rel_created_at) in
            rel_info
        {
            ui.horizontal(|ui| {
                // Break link button
                if ui
                    .small_button("x")
                    .on_hover_text("Remove relationship")
                    .clicked()
                {
                    relationship_to_remove = Some((rel_type.clone(), target_id));
                }

                // Show color indicator if defined
                if let Some(ref hex_color) = color {
                    if let Some(c) = parse_hex_color(hex_color) {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, c);
                    }
                }

                // Use display name from definition
                let label = format!("{} {} - {}", display_name, target_label, target_title);

                let response = ui.add(egui::Label::new(&label).sense(egui::Sense::click()));

                // Build tooltip with creator info
                let mut tooltip = "Double-click to view".to_string();
                if let Some(ref created_by) = rel_created_by {
                    tooltip.push_str(&format!("\nCreated by: {}", created_by));
                }
                if let Some(created_at) = rel_created_at {
                    tooltip.push_str(&format!("\nCreated: {}", created_at.format("%Y-%m-%d %H:%M")));
                }

                // Show hover cursor and tooltip
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                response.clone().on_hover_text(tooltip);

                // Navigate on double-click
                if response.double_clicked() {
                    if let Some(idx) = target_idx {
                        self.selected_idx = Some(idx);
                        self.pending_view_change = Some(View::Detail);
                    }
                }
            });
        }

        // Remove relationship if requested
        if let Some((rel_type, target_id)) = relationship_to_remove {
            // Check if the relationship has an inverse defined
            let bidirectional = self.store.get_inverse_type(&rel_type).is_some();
            if let Err(e) =
                self.store
                    .remove_relationship(&req_id, &rel_type, &target_id, bidirectional)
            {
                self.message = Some((format!("Failed to remove relationship: {}", e), true));
            } else {
                self.save();
                self.message = Some(("Relationship removed".to_string(), false));
            }
        }
    }

    /// Show relationships as a recursive tree structure
    /// `ancestor_path` tracks the path of requirement IDs from root to current node
    /// `follow_rel_type` if Some, only show relationships of this type (for consistent traversal direction)
    fn show_relationships_tree(
        &mut self,
        ui: &mut egui::Ui,
        req_id: Uuid,
        depth: usize,
        ancestor_path: &mut Vec<Uuid>,
        follow_rel_type: Option<&RelationshipType>,
    ) {
        // Get the root requirement ID (first in path, or current if at root)
        let root_id = ancestor_path.first().copied().unwrap_or(req_id);

        let Some(req) = self.store.requirements.iter().find(|r| r.id == req_id) else {
            return;
        };

        // Filter relationships based on whether we're following a specific type
        let relationships_to_show: Vec<_> = if let Some(rel_type) = follow_rel_type {
            // When recursing, only show relationships of the same type
            req.relationships
                .iter()
                .filter(|r| &r.rel_type == rel_type)
                .collect()
        } else {
            // At root (depth 0), show all relationships
            req.relationships.iter().collect()
        };

        if relationships_to_show.is_empty() {
            if depth == 0 {
                ui.label("No relationships defined");
            }
            return;
        }

        // Collect relationship info
        let rel_info: Vec<_> = relationships_to_show
            .iter()
            .map(|rel| {
                let target_idx = self
                    .store
                    .requirements
                    .iter()
                    .position(|r| r.id == rel.target_id);
                let target_label = self
                    .store
                    .requirements
                    .iter()
                    .find(|r| r.id == rel.target_id)
                    .and_then(|r| r.spec_id.as_ref())
                    .cloned()
                    .unwrap_or_else(|| "Unknown".to_string());
                let target_title = self
                    .store
                    .requirements
                    .iter()
                    .find(|r| r.id == rel.target_id)
                    .map(|r| r.title.clone())
                    .unwrap_or_else(|| "(not found)".to_string());
                let target_req = self
                    .store
                    .requirements
                    .iter()
                    .find(|r| r.id == rel.target_id);

                // Get display name and color from relationship definition
                let (display_name, color) = self
                    .store
                    .get_definition_for_type(&rel.rel_type)
                    .map(|def| (def.display_name.clone(), def.color.clone()))
                    .unwrap_or_else(|| (format!("{}", rel.rel_type), None));

                // Check how many expandable children the target actually has
                // (relationships of same type that don't point back to ancestors or current req)
                let mut path_with_current = ancestor_path.clone();
                path_with_current.push(req_id);
                let expandable_children_count = target_req
                    .map(|t| {
                        t.relationships
                            .iter()
                            // Must be same relationship type
                            .filter(|tr| tr.rel_type == rel.rel_type)
                            // Must not point back to any ancestor
                            .filter(|tr| !path_with_current.contains(&tr.target_id))
                            .count()
                    })
                    .unwrap_or(0);

                // Check if target is shared (has multiple parents via same relationship type)
                // Count how many requirements have this target as a child
                let parent_count = self
                    .store
                    .requirements
                    .iter()
                    .filter(|r| {
                        r.relationships.iter().any(|rel2| {
                            rel2.target_id == rel.target_id && rel2.rel_type == rel.rel_type
                        })
                    })
                    .count();
                let is_shared = parent_count > 1;

                // Check for cross-relationship: target has a relationship to root via different rel type
                let cross_relationship = if rel.target_id != root_id {
                    target_req.and_then(|t| {
                        t.relationships
                            .iter()
                            .find(|tr| tr.target_id == root_id && tr.rel_type != rel.rel_type)
                            .map(|tr| tr.rel_type.clone())
                    })
                } else {
                    None
                };

                (
                    rel.rel_type.clone(),
                    rel.target_id,
                    target_idx,
                    target_label,
                    target_title,
                    display_name,
                    color,
                    expandable_children_count,
                    is_shared,
                    cross_relationship,
                )
            })
            .collect();

        let indent = depth as f32 * 20.0;

        for (
            rel_type,
            target_id,
            target_idx,
            target_label,
            target_title,
            display_name,
            color,
            expandable_children_count,
            is_shared,
            cross_relationship,
        ) in rel_info
        {
            // Skip if this would show the same requirement that's already in our path
            if ancestor_path.contains(&target_id) {
                continue;
            }

            // Check if this node is collapsed (default: collapsed)
            let collapse_key = (req_id, target_id);
            let is_collapsed = *self
                .relationship_tree_collapsed
                .get(&collapse_key)
                .unwrap_or(&true);

            // Determine if we should allow recursion and show expand button
            // expandable_children_count already accounts for same rel type and ancestor filtering
            let has_expandable_children = expandable_children_count > 0;

            ui.horizontal(|ui| {
                ui.add_space(indent);

                // Show expand/collapse button only if there are expandable children
                if has_expandable_children {
                    let icon = if is_collapsed { "▶" } else { "▼" };
                    if ui.small_button(icon).clicked() {
                        self.relationship_tree_collapsed
                            .insert(collapse_key, !is_collapsed);
                    }
                } else {
                    // Placeholder for alignment
                    ui.add_space(20.0);
                }

                // Show color indicator if defined
                if let Some(ref hex_color) = color {
                    if let Some(c) = parse_hex_color(hex_color) {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, c);
                    }
                }

                // Use display name from definition
                let label = format!("{} {} - {}", display_name, target_label, target_title);

                let response = ui.add(egui::Label::new(&label).sense(egui::Sense::click()));

                // Show hover cursor and tooltip
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                response.clone().on_hover_text("Double-click to view");

                // Navigate on double-click
                if response.double_clicked() {
                    if let Some(idx) = target_idx {
                        self.selected_idx = Some(idx);
                        self.pending_view_change = Some(View::Detail);
                    }
                }

                // Show indicators for special cases
                if is_shared {
                    let shared_response = ui.add(
                        egui::Label::new(egui::RichText::new("↑ shared").small().weak())
                            .sense(egui::Sense::click()),
                    );
                    shared_response.on_hover_text("This requirement has multiple parents");
                }

                // Show cross-relationship indicator (bidirectional via different rel type)
                if let Some(ref cross_rel_type) = cross_relationship {
                    let cross_display = self
                        .store
                        .get_definition_for_type(cross_rel_type)
                        .map(|def| def.display_name.clone())
                        .unwrap_or_else(|| format!("{}", cross_rel_type));

                    let cross_response = ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("↔ {}", cross_display))
                                .small()
                                .color(egui::Color32::LIGHT_BLUE),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if cross_response
                        .on_hover_text(format!(
                            "Also {} root. Double-click to view.",
                            cross_display.to_lowercase()
                        ))
                        .double_clicked()
                    {
                        if let Some(idx) = target_idx {
                            self.selected_idx = Some(idx);
                            self.pending_view_change = Some(View::Detail);
                        }
                    }
                }
            });

            // Recursively show children if expanded and there are expandable children
            if has_expandable_children && !is_collapsed {
                ancestor_path.push(req_id);
                self.show_relationships_tree(
                    ui,
                    target_id,
                    depth + 1,
                    ancestor_path,
                    Some(&rel_type),
                );
                ancestor_path.pop();
            }
        }
    }

    fn show_url_form_modal(&mut self, ui: &mut egui::Ui, req_id: Uuid) {
        let is_editing = self.editing_url_id.is_some();
        let title = if is_editing {
            "Edit URL Link"
        } else {
            "Add URL Link"
        };

        ui.heading(title);
        ui.add_space(10.0);

        egui::Grid::new("url_form_grid")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("URL:");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.url_form_url)
                            .hint_text("https://example.com/...")
                            .desired_width(350.0),
                    );

                    // Verify button
                    let verify_enabled =
                        !self.url_form_url.is_empty() && !self.url_verification_in_progress;
                    if ui
                        .add_enabled(verify_enabled, egui::Button::new("🔍 Verify"))
                        .clicked()
                    {
                        self.verify_url();
                    }
                });
                ui.end_row();

                // Show verification status
                if let Some((success, ref msg)) = self.url_verification_status {
                    ui.label("");
                    if success {
                        ui.colored_label(
                            egui::Color32::from_rgb(100, 200, 100),
                            format!("✅ {}", msg),
                        );
                    } else {
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 100, 100),
                            format!("❌ {}", msg),
                        );
                    }
                    ui.end_row();
                }

                if self.url_verification_in_progress {
                    ui.label("");
                    ui.label("⏳ Verifying...");
                    ui.end_row();
                }

                ui.label("Title:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.url_form_title)
                        .hint_text("Display title (optional)")
                        .desired_width(350.0),
                );
                ui.end_row();

                ui.label("Description:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.url_form_description)
                        .hint_text("Brief description (optional)")
                        .desired_width(350.0),
                );
                ui.end_row();
            });

        ui.add_space(15.0);

        // Save/Cancel buttons
        ui.horizontal(|ui| {
            let can_save = !self.url_form_url.is_empty();

            if ui
                .add_enabled(can_save, egui::Button::new("💾 Save"))
                .clicked()
            {
                self.save_url_link(req_id);
            }

            if ui.button("Cancel").clicked() {
                self.show_url_form = false;
            }
        });

        if self.url_form_url.is_empty() {
            ui.small("URL is required.");
        }
    }

    fn verify_url(&mut self) {
        let url = self.url_form_url.trim();

        // Basic URL validation
        if url.is_empty() {
            self.url_verification_status = Some((false, "URL is empty".to_string()));
            return;
        }

        // Check if it looks like a valid URL
        if !url.starts_with("http://") && !url.starts_with("https://") {
            self.url_verification_status =
                Some((false, "URL must start with http:// or https://".to_string()));
            return;
        }

        // Try to parse URL
        match url::Url::parse(url) {
            Ok(parsed) => {
                // Perform a HEAD request to verify the URL is accessible
                // For now, just do basic validation since we don't have async HTTP in egui easily
                // In a real app, you'd use reqwest or similar

                // Check that it has a host
                if parsed.host().is_none() {
                    self.url_verification_status =
                        Some((false, "Invalid URL: no host".to_string()));
                    return;
                }

                // For now, mark as "valid format" - actual HTTP check would need async
                self.url_verification_status = Some((
                    true,
                    format!(
                        "Valid URL format (host: {})",
                        parsed.host_str().unwrap_or("unknown")
                    ),
                ));
            }
            Err(e) => {
                self.url_verification_status = Some((false, format!("Invalid URL: {}", e)));
            }
        }
    }

    /// Show the reference picker modal for adding References relationships
    fn show_reference_picker_modal(&mut self, ui: &mut egui::Ui, source_req_id: Uuid) {
        ui.heading("Add Reference");
        ui.add_space(5.0);
        ui.label("Search for a requirement to reference:");
        ui.add_space(10.0);

        // Search box with focus
        let search_response = ui.add(
            egui::TextEdit::singleline(&mut self.reference_picker_search)
                .hint_text("Type to search by ID, title, or description...")
                .desired_width(400.0),
        );

        // Request focus on first frame
        if !search_response.has_focus() && self.reference_picker_search.is_empty() {
            search_response.request_focus();
        }

        ui.add_space(10.0);

        // Get existing reference targets from the source requirement
        let existing_refs: std::collections::HashSet<Uuid> = self
            .store
            .requirements
            .iter()
            .find(|r| r.id == source_req_id)
            .map(|r| {
                r.relationships
                    .iter()
                    .filter(|rel| rel.rel_type == RelationshipType::References)
                    .map(|rel| rel.target_id)
                    .collect()
            })
            .unwrap_or_default();

        // Perform fuzzy search and collect matching requirements
        let search_lower = self.reference_picker_search.to_lowercase();
        let matches: Vec<(Uuid, String, String, Option<String>)> = self
            .store
            .requirements
            .iter()
            .filter(|r| {
                // Don't show the source requirement itself
                if r.id == source_req_id {
                    return false;
                }
                // Don't show already-referenced requirements
                if existing_refs.contains(&r.id) {
                    return false;
                }
                // If search is empty, show all (up to a limit)
                if search_lower.is_empty() {
                    return true;
                }
                // Fuzzy match on spec_id, title, and description
                let spec_id_match = r
                    .spec_id
                    .as_ref()
                    .map(|s| s.to_lowercase().contains(&search_lower))
                    .unwrap_or(false);
                let title_match = r.title.to_lowercase().contains(&search_lower);
                let desc_match = r.description.to_lowercase().contains(&search_lower);
                spec_id_match || title_match || desc_match
            })
            .take(50) // Limit results
            .map(|r| {
                let label = r
                    .spec_id
                    .as_ref()
                    .map(|s| format!("{}: {}", s, r.title))
                    .unwrap_or_else(|| r.title.clone());
                let status = r.effective_status();
                let desc = if r.description.len() > 100 {
                    Some(format!("{}...", &r.description[..100]))
                } else if !r.description.is_empty() {
                    Some(r.description.clone())
                } else {
                    None
                };
                (r.id, label, status, desc)
            })
            .collect();

        // Show results in a scrollable area
        let available_height = ui.available_height() - 60.0; // Leave room for buttons
        egui::ScrollArea::vertical()
            .max_height(available_height.max(200.0))
            .show(ui, |ui| {
                if matches.is_empty() {
                    if search_lower.is_empty() {
                        ui.label("No requirements available to reference.");
                    } else {
                        ui.label("No matching requirements found.");
                    }
                } else {
                    for (id, label, status, desc) in &matches {
                        let is_selected = self.reference_picker_selected == Some(*id);
                        let response = ui.selectable_label(is_selected, label);

                        // Check clicked/double_clicked before consuming response with on_hover_text
                        let was_clicked = response.clicked();
                        let was_double_clicked = response.double_clicked();

                        // Show status and description on hover
                        let hover_text = if let Some(d) = desc {
                            format!("Status: {}\n{}", status, d)
                        } else {
                            format!("Status: {}", status)
                        };
                        response.on_hover_text(hover_text);

                        if was_clicked {
                            self.reference_picker_selected = Some(*id);
                        }

                        // Double-click to immediately link
                        if was_double_clicked {
                            self.reference_picker_selected = Some(*id);
                            self.create_reference_relationship(source_req_id, *id);
                            self.show_reference_picker = false;
                            self.reference_picker_search.clear();
                            self.reference_picker_selected = None;
                            return;
                        }
                    }
                }
            });

        ui.add_space(15.0);

        // Buttons
        ui.horizontal(|ui| {
            let can_link = self.reference_picker_selected.is_some();
            if ui
                .add_enabled(can_link, egui::Button::new("🔗 Link"))
                .on_hover_text("Create a References relationship")
                .clicked()
            {
                if let Some(target_id) = self.reference_picker_selected {
                    self.create_reference_relationship(source_req_id, target_id);
                    self.show_reference_picker = false;
                    self.reference_picker_search.clear();
                    self.reference_picker_selected = None;
                }
            }
            if ui.button("Cancel").clicked() {
                self.show_reference_picker = false;
                self.reference_picker_search.clear();
                self.reference_picker_selected = None;
            }
        });
    }

    /// Create a References relationship between two requirements
    fn create_reference_relationship(&mut self, source_id: Uuid, target_id: Uuid) {
        match self.store.add_relationship_with_creator(
            &source_id,
            RelationshipType::References,
            &target_id,
            false, // Not bidirectional for References
            Some(self.user_settings.display_name()),
        ) {
            Ok(_) => {
                self.save();
                self.message = Some(("Reference relationship created".to_string(), false));
            }
            Err(e) => {
                self.message = Some((format!("Failed to create relationship: {}", e), true));
            }
        }
    }

    fn save_url_link(&mut self, req_id: Uuid) {
        let author = if self.user_settings.name.is_empty() {
            "Unknown".to_string()
        } else {
            self.user_settings.name.clone()
        };

        if let Some(idx) = self.selected_idx {
            if let Some(req) = self.store.requirements.get_mut(idx) {
                if req.id == req_id {
                    if let Some(editing_id) = self.editing_url_id {
                        // Update existing URL
                        if let Some(url_link) = req.urls.iter_mut().find(|u| u.id == editing_id) {
                            url_link.url = self.url_form_url.clone();
                            url_link.title = self.url_form_title.clone();
                            url_link.description = if self.url_form_description.is_empty() {
                                None
                            } else {
                                Some(self.url_form_description.clone())
                            };
                            // Update verification status if we just verified
                            if let Some((success, _)) = &self.url_verification_status {
                                url_link.last_verified = Some(chrono::Utc::now());
                                url_link.last_verified_ok = Some(*success);
                            }
                        }
                    } else {
                        // Add new URL
                        let mut url_link = UrlLink::new(
                            self.url_form_url.clone(),
                            self.url_form_title.clone(),
                            author,
                        );
                        if !self.url_form_description.is_empty() {
                            url_link.description = Some(self.url_form_description.clone());
                        }
                        // Set verification status if we just verified
                        if let Some((success, _)) = &self.url_verification_status {
                            url_link.last_verified = Some(chrono::Utc::now());
                            url_link.last_verified_ok = Some(*success);
                        }
                        req.urls.push(url_link);
                    }

                    self.save();
                    self.message = Some(("URL saved".to_string(), false));
                }
            }
        }

        self.show_url_form = false;
    }

    fn show_markdown_help_modal(&mut self, ctx: &egui::Context) {
        let max_size = modal_max_size(ctx);
        let modal_width = 700.0_f32.min(max_size.x);
        let modal_height = 500.0_f32.min(max_size.y);

        // Draw a dark overlay behind the modal
        let screen_rect = ctx.screen_rect();
        ctx.layer_painter(egui::LayerId::new(egui::Order::PanelResizeLine, egui::Id::new("modal_overlay")))
            .rect_filled(screen_rect, 0.0, egui::Color32::from_black_alpha(128));

        // Use egui::Area for direct control over positioning and sizing
        // This avoids the auto-sizing animation behavior of Window
        egui::Area::new(egui::Id::new("markdown_help_area"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::window(ui.style())
                    .show(ui, |ui| {
                        // Force the exact size we want
                        ui.set_min_size(egui::vec2(modal_width, modal_height));
                        ui.set_max_size(egui::vec2(modal_width, modal_height));

                        // Title bar
                        ui.horizontal(|ui| {
                            ui.heading("Markdown Help");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("✕").clicked() {
                                    self.show_markdown_help = false;
                                }
                            });
                        });
                        ui.separator();

                        // Content area with scrolling
                        let content_height = modal_height - 80.0; // Reserve space for title and close button
                        egui::ScrollArea::vertical()
                            .max_height(content_height)
                            .show(ui, |ui| {
                                ui.set_min_width(modal_width - 30.0);

                                let available_width = ui.available_width();
                                let panel_width = (available_width - 20.0) / 2.0;

                                ui.horizontal(|ui| {
                                    // Left panel: Syntax reference
                                    ui.vertical(|ui| {
                                        ui.set_width(panel_width);
                                        ui.heading("Syntax");
                                        ui.separator();
                                        ui.add_space(4.0);

                                        ui.group(|ui| {
                                            ui.strong("Headers");
                                            ui.code("# Heading 1\n## Heading 2\n### Heading 3");
                                        });
                                        ui.add_space(6.0);
                                        ui.group(|ui| {
                                            ui.strong("Text Formatting");
                                            ui.code("**bold**\n*italic*\n~~strikethrough~~\n`inline code`");
                                        });
                                        ui.add_space(6.0);
                                        ui.group(|ui| {
                                            ui.strong("Lists");
                                            ui.code("- Item 1\n- Item 2\n  - Nested item\n\n1. First\n2. Second");
                                        });
                                        ui.add_space(6.0);
                                        ui.group(|ui| {
                                            ui.strong("Links");
                                            ui.code("[Link text](https://example.com)");
                                        });
                                        ui.add_space(6.0);
                                        ui.group(|ui| {
                                            ui.strong("Code Blocks");
                                            ui.code("```\ncode block\n```\n\n```rust\nfn main() { }\n```");
                                        });
                                        ui.add_space(6.0);
                                        ui.group(|ui| {
                                            ui.strong("Blockquotes");
                                            ui.code("> This is a quote");
                                        });
                                        ui.add_space(6.0);
                                        ui.group(|ui| {
                                            ui.strong("Horizontal Rule");
                                            ui.code("---");
                                        });
                                        ui.add_space(6.0);
                                        ui.group(|ui| {
                                            ui.strong("Tables");
                                            ui.code("| H1 | H2 |\n|----|----|\n| A  | B  |");
                                        });
                                        ui.add_space(6.0);
                                        ui.group(|ui| {
                                            ui.strong("Task Lists");
                                            ui.code("- [ ] Todo\n- [x] Done");
                                        });
                                    });

                                    ui.separator();

                                    // Right panel: Live preview
                                    ui.vertical(|ui| {
                                        ui.set_width(panel_width);
                                        ui.heading("Preview");
                                        ui.separator();
                                        ui.add_space(4.0);

                                        let sample_markdown = r#"# Heading 1
## Heading 2
### Heading 3

**bold** and *italic* and ~~strikethrough~~

`inline code` looks like this

- Item 1
- Item 2
  - Nested item

1. First
2. Second

[Link text](https://example.com)

```rust
fn main() {
    println!("Hello");
}
```

> This is a quote

---

| H1 | H2 |
|----|----|
| A  | B  |

- [ ] Todo
- [x] Done
"#;
                                        CommonMarkViewer::new()
                                            .show(ui, &mut self.markdown_cache, sample_markdown);
                                    });
                                });
                            });

                        ui.add_space(8.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("Close").clicked() {
                                self.show_markdown_help = false;
                            }
                        });
                    });
            });
    }

    fn show_ai_results_modal(&mut self, ctx: &egui::Context) {
        let max_size = modal_max_size(ctx);
        let modal_width = 500.0_f32.min(max_size.x);
        let modal_height = 300.0_f32.min(max_size.y);

        // Draw a dark overlay behind the modal
        let screen_rect = ctx.screen_rect();
        ctx.layer_painter(egui::LayerId::new(egui::Order::PanelResizeLine, egui::Id::new("ai_modal_overlay")))
            .rect_filled(screen_rect, 0.0, egui::Color32::from_black_alpha(128));

        egui::Area::new(egui::Id::new("ai_results_area"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::window(ui.style())
                    .show(ui, |ui| {
                        ui.set_min_size(egui::vec2(modal_width, modal_height));
                        ui.set_max_size(egui::vec2(modal_width, modal_height));

                        // Title bar
                        ui.horizontal(|ui| {
                            ui.heading("🤖 AI Analysis");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("✕").clicked() {
                                    self.show_ai_results_panel = false;
                                    self.ai_last_result = None;
                                }
                            });
                        });
                        ui.separator();

                        // Content
                        if let Some(result) = &self.ai_last_result {
                            ui.add_space(10.0);

                            // Status indicator
                            let status_text = if result.success {
                                egui::RichText::new("✅ Action: ").color(egui::Color32::from_rgb(100, 200, 100))
                            } else {
                                egui::RichText::new("❌ Action: ").color(egui::Color32::from_rgb(200, 100, 100))
                            };
                            ui.horizontal(|ui| {
                                ui.label(status_text);
                                ui.label(&result.action_name);
                            });

                            ui.add_space(10.0);
                            ui.separator();
                            ui.add_space(10.0);

                            // Message
                            egui::ScrollArea::vertical()
                                .max_height(modal_height - 150.0)
                                .show(ui, |ui| {
                                    ui.label(&result.message);

                                    if let Some(details) = &result.details {
                                        ui.add_space(10.0);
                                        ui.separator();
                                        ui.add_space(5.0);
                                        ui.label(egui::RichText::new("Details:").strong());
                                        ui.label(details);
                                    }
                                });
                        } else if self.ai_loading {
                            ui.add_space(20.0);
                            ui.centered_and_justified(|ui| {
                                ui.spinner();
                            });
                            ui.add_space(10.0);
                            ui.centered_and_justified(|ui| {
                                ui.label("Processing...");
                            });
                        } else {
                            ui.label("No AI results available.");
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("Close").clicked() {
                                self.show_ai_results_panel = false;
                                self.ai_last_result = None;
                            }
                        });
                    });
            });
    }

    fn show_history_tab(&self, ui: &mut egui::Ui, req: &Requirement) {
        ui.heading("Change History");
        ui.add_space(10.0);

        if req.history.is_empty() {
            ui.label("No changes recorded yet");
        } else {
            for entry in req.history.iter().rev() {
                // Show newest first
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "🕒 {}",
                            entry.timestamp.format("%Y-%m-%d %H:%M:%S")
                        ));
                        ui.label(format!("👤 {}", entry.author));
                    });

                    ui.add_space(5.0);

                    for change in &entry.changes {
                        ui.horizontal(|ui| {
                            ui.label(format!("  📝 {}", change.field_name));
                        });
                        ui.horizontal(|ui| {
                            ui.label("    ❌");
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 100, 100),
                                &change.old_value,
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("    ✅");
                            ui.colored_label(
                                egui::Color32::from_rgb(100, 200, 100),
                                &change.new_value,
                            );
                        });
                    }
                });
                ui.add_space(10.0);
            }
        }
    }

    fn show_form(&mut self, ui: &mut egui::Ui, is_edit: bool) {
        let title = if is_edit {
            "Edit Requirement"
        } else {
            "Add Requirement"
        };
        ui.heading(title);
        ui.separator();

        // Calculate available width for text fields
        let available_width = ui.available_width();

        // Title field - full width with context menu
        ui.label("Title:");
        // Track title before TextEdit to detect manual edits (for auto-sync feature)
        let title_before = self.form_title.clone();
        let title_output = egui::TextEdit::singleline(&mut self.form_title)
            .desired_width(available_width)
            .show(ui);
        show_text_context_menu(
            ui,
            &title_output.response,
            &mut self.form_title,
            title_output.response.id,
            &mut self.last_text_selection,
        );
        // Detect manual title edits - if user changes title while it has focus, disable auto-sync
        // (In Add mode, we sync description's first line to title automatically)
        if !is_edit && self.form_title_auto_synced && title_output.response.has_focus() {
            // If title changed and it wasn't from our sync (description didn't change)
            if self.form_title != title_before && self.form_description == self.form_last_description {
                self.form_title_auto_synced = false;
            }
        }
        ui.add_space(8.0);

        // Metadata row - Type first (affects available statuses), then Status, Priority
        let mut type_changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Type:");
            let old_type = self.form_type.clone();
            egui::ComboBox::new("type_combo", "")
                .selected_text(format!("{:?}", self.form_type))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.form_type,
                        RequirementType::Functional,
                        "Functional",
                    );
                    ui.selectable_value(
                        &mut self.form_type,
                        RequirementType::NonFunctional,
                        "NonFunctional",
                    );
                    ui.selectable_value(&mut self.form_type, RequirementType::System, "System");
                    ui.selectable_value(&mut self.form_type, RequirementType::User, "User");
                    ui.selectable_value(
                        &mut self.form_type,
                        RequirementType::ChangeRequest,
                        "Change Request",
                    );
                    ui.selectable_value(&mut self.form_type, RequirementType::Bug, "Bug");
                    ui.separator();
                    ui.selectable_value(&mut self.form_type, RequirementType::Epic, "Epic");
                    ui.selectable_value(&mut self.form_type, RequirementType::Story, "Story");
                    ui.selectable_value(&mut self.form_type, RequirementType::Task, "Task");
                    ui.selectable_value(&mut self.form_type, RequirementType::Spike, "Spike");
                    ui.selectable_value(&mut self.form_type, RequirementType::Sprint, "🏃 Sprint");
                    ui.separator();
                    ui.selectable_value(&mut self.form_type, RequirementType::Folder, "📁 Folder");
                });
            type_changed = old_type != self.form_type;

            // Only show status/priority for stateful types
            if !self.store.is_type_stateless(&self.form_type) {
                ui.add_space(16.0);
                ui.label("Status:");
                // Get statuses for current type
                let statuses = self.store.get_statuses_for_type(&self.form_type);
                egui::ComboBox::new("status_combo", "")
                    .selected_text(&self.form_status_string)
                    .show_ui(ui, |ui| {
                        for status in &statuses {
                            if ui
                                .selectable_label(self.form_status_string == *status, status)
                                .clicked()
                            {
                                self.form_status_string = status.clone();
                            }
                        }
                    });

                ui.add_space(16.0);
                ui.label("Priority:");
                // Get priorities for current type
                let priorities = self.store.get_priorities_for_type(&self.form_type);
                egui::ComboBox::new("priority_combo", "")
                    .selected_text(&self.form_priority_string)
                    .show_ui(ui, |ui| {
                        for priority in &priorities {
                            if ui
                                .selectable_label(self.form_priority_string == *priority, priority)
                                .clicked()
                            {
                                self.form_priority_string = priority.clone();
                            }
                        }
                    });
            }
        });

        // If type changed, check if current status/priority is valid for new type
        if type_changed {
            let statuses = self.store.get_statuses_for_type(&self.form_type);
            if !statuses.contains(&self.form_status_string) {
                // Reset to first available status for new type
                self.form_status_string = statuses
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Draft".to_string());
            }
            let priorities = self.store.get_priorities_for_type(&self.form_type);
            if !priorities.contains(&self.form_priority_string) {
                // Reset to first available priority for new type
                self.form_priority_string = priorities
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Medium".to_string());
            }
            // Clear custom fields when type changes (they may not be relevant)
            self.form_custom_fields.clear();
        }
        ui.add_space(4.0);

        ui.horizontal_wrapped(|ui| {
            ui.label("Owner:");
            ui.add(egui::TextEdit::singleline(&mut self.form_owner).desired_width(150.0));

            ui.add_space(16.0);
            ui.label("Feature:");
            // Feature dropdown with fuzzy search
            {
                let popup_id = ui.make_persistent_id("feature_combo_popup");
                let is_popup_open = ui.memory(|mem| mem.is_popup_open(popup_id));

                // Get all features and filter based on current text
                let all_features = self.get_all_features();
                let search_lower = self.form_feature.to_lowercase();
                let filtered: Vec<String> = if search_lower.is_empty() {
                    all_features.clone()
                } else {
                    all_features.iter()
                        .filter(|f| f.to_lowercase().contains(&search_lower))
                        .cloned()
                        .collect()
                };
                let num_options = filtered.len();

                // Handle keyboard BEFORE the TextEdit to consume arrow/enter keys
                let mut close_and_select = false;
                if is_popup_open {
                    ui.ctx().input_mut(|i| {
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                            if num_options > 0 && self.form_feature_selected_idx < num_options - 1 {
                                self.form_feature_selected_idx += 1;
                            } else if num_options > 0 {
                                self.form_feature_selected_idx = 0; // Wrap to top
                            }
                        }
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                            if self.form_feature_selected_idx > 0 {
                                self.form_feature_selected_idx -= 1;
                            } else if num_options > 0 {
                                self.form_feature_selected_idx = num_options - 1; // Wrap to bottom
                            }
                        }
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                            close_and_select = true;
                        }
                        if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                            // Just close popup, don't select
                            ui.memory_mut(|mem| mem.close_popup());
                        }
                    });
                }

                // Apply selection if Enter was pressed
                if close_and_select && !filtered.is_empty() {
                    if let Some(feature) = filtered.get(self.form_feature_selected_idx) {
                        self.form_feature = feature.clone();
                    }
                    ui.memory_mut(|mem| mem.close_popup());
                }

                let feature_edit = ui.add(
                    egui::TextEdit::singleline(&mut self.form_feature)
                        .desired_width(150.0)
                        .hint_text("Type to search...")
                );

                // Open popup when text field gains focus or user is typing
                if feature_edit.gained_focus() || (feature_edit.has_focus() && feature_edit.changed()) {
                    ui.memory_mut(|mem| mem.open_popup(popup_id));
                    self.form_feature_selected_idx = 0;
                }

                // Clamp selected index
                if self.form_feature_selected_idx >= num_options && num_options > 0 {
                    self.form_feature_selected_idx = num_options - 1;
                }

                // Show popup with filtered features
                egui::popup_below_widget(
                    ui,
                    popup_id,
                    &feature_edit,
                    egui::PopupCloseBehavior::CloseOnClickOutside,
                    |ui| {
                        ui.set_min_width(150.0);
                        egui::ScrollArea::vertical()
                            .max_height(150.0)
                            .show(ui, |ui| {
                                if filtered.is_empty() {
                                    ui.weak("No matching features");
                                } else {
                                    for (idx, feature) in filtered.iter().enumerate() {
                                        let is_selected = idx == self.form_feature_selected_idx;
                                        let response = ui.selectable_label(is_selected, feature);
                                        if response.clicked() {
                                            self.form_feature = feature.clone();
                                            ui.memory_mut(|mem| mem.close_popup());
                                        }
                                    }
                                }
                            });
                    }
                );
            }

            ui.add_space(16.0);
            ui.label("Tags:");
            ui.add(
                egui::TextEdit::singleline(&mut self.form_tags)
                    .desired_width(200.0)
                    .hint_text("comma-separated"),
            );
        });
        ui.add_space(4.0);

        ui.horizontal_wrapped(|ui| {
            ui.label("ID Prefix:");

            if self.store.restrict_prefixes && !self.store.allowed_prefixes.is_empty() {
                // Restricted mode: show dropdown
                let current_display = if self.form_prefix.is_empty() {
                    "(default)".to_string()
                } else {
                    self.form_prefix.clone()
                };
                egui::ComboBox::new("prefix_combo", "")
                    .selected_text(&current_display)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(self.form_prefix.is_empty(), "(default)").clicked() {
                            self.form_prefix.clear();
                        }
                        for prefix in &self.store.allowed_prefixes.clone() {
                            if ui.selectable_label(self.form_prefix == *prefix, prefix).clicked() {
                                self.form_prefix = prefix.clone();
                            }
                        }
                    });
                ui.label("or");
                ui.add(egui::TextEdit::singleline(&mut self.form_prefix)
                    .desired_width(80.0)
                    .hint_text("e.g., SEC"))
                    .on_hover_text("Custom prefix (A-Z only). Leave blank to use default.");

                // Show validation status
                let prefix_trimmed = self.form_prefix.trim();
                if !prefix_trimmed.is_empty() {
                    if Requirement::validate_prefix(prefix_trimmed).is_some() {
                        ui.label("✓").on_hover_text("Valid prefix");
                    } else {
                        ui.colored_label(egui::Color32::RED, "✗")
                            .on_hover_text("Prefix must contain only uppercase letters (A-Z)");
                    }
                }
                ui.label("ⓘ").on_hover_text("Dropdown shows allowed prefixes; text box allows custom entry");
            } else {
                // Unrestricted mode: show text input with optional dropdown
                let all_prefixes = self.store.get_all_prefixes();
                if !all_prefixes.is_empty() {
                    // Show combo box with existing prefixes + ability to type new ones
                    egui::ComboBox::new("prefix_combo", "")
                        .selected_text(if self.form_prefix.is_empty() { "(default)" } else { &self.form_prefix })
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(self.form_prefix.is_empty(), "(default)").clicked() {
                                self.form_prefix.clear();
                            }
                            for prefix in &all_prefixes {
                                if ui.selectable_label(self.form_prefix == *prefix, prefix).clicked() {
                                    self.form_prefix = prefix.clone();
                                }
                            }
                        });
                    ui.label("or");
                }

                ui.add(egui::TextEdit::singleline(&mut self.form_prefix)
                    .desired_width(80.0)
                    .hint_text("e.g., SEC"))
                    .on_hover_text("Optional custom prefix (A-Z only). Leave blank to use default from feature/type.");

                // Show validation status
                let prefix_trimmed = self.form_prefix.trim();
                if !prefix_trimmed.is_empty() {
                    if Requirement::validate_prefix(prefix_trimmed).is_some() {
                        ui.label("✓").on_hover_text("Valid prefix");
                    } else {
                        ui.colored_label(egui::Color32::RED, "✗")
                            .on_hover_text("Prefix must contain only uppercase letters (A-Z)");
                    }
                }
            }
        });
        ui.add_space(4.0);

        // Show parent relationship for new requirements (not edit)
        if !is_edit {
            if let Some(parent_id) = self.form_parent_id {
                let parent_info = self
                    .store
                    .requirements
                    .iter()
                    .find(|r| r.id == parent_id)
                    .map(|r| {
                        let spec = r.spec_id.as_deref().unwrap_or("N/A");
                        format!("{} - {}", spec, r.title)
                    });

                if let Some(parent_label) = parent_info {
                    ui.horizontal(|ui| {
                        ui.label("Parent:");
                        ui.label(&parent_label);
                        if ui
                            .small_button("x")
                            .on_hover_text("Remove parent")
                            .clicked()
                        {
                            self.form_parent_id = None;
                        }
                    });
                }
            }
        }

        // Show custom fields for the current type
        let custom_fields = self.store.get_custom_fields_for_type(&self.form_type);
        if !custom_fields.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            ui.label("Type-specific Fields:");
            ui.add_space(4.0);

            // Sort fields by order
            let mut sorted_fields = custom_fields;
            sorted_fields.sort_by_key(|f| f.order);

            for field in sorted_fields {
                ui.horizontal(|ui| {
                    let label = if field.required {
                        format!("{}*:", field.label)
                    } else {
                        format!("{}:", field.label)
                    };
                    ui.label(&label);

                    // Get current value or default
                    let current_value = self
                        .form_custom_fields
                        .get(&field.name)
                        .cloned()
                        .or_else(|| field.default_value.clone())
                        .unwrap_or_default();

                    match field.field_type {
                        CustomFieldType::Text => {
                            let mut value = current_value;
                            if ui
                                .add(egui::TextEdit::singleline(&mut value).desired_width(200.0))
                                .changed()
                            {
                                self.form_custom_fields.insert(field.name.clone(), value);
                            }
                        }
                        CustomFieldType::TextArea => {
                            let mut value = current_value;
                            if ui
                                .add(
                                    egui::TextEdit::multiline(&mut value)
                                        .desired_width(300.0)
                                        .desired_rows(3),
                                )
                                .changed()
                            {
                                self.form_custom_fields.insert(field.name.clone(), value);
                            }
                        }
                        CustomFieldType::Select => {
                            egui::ComboBox::new(&field.name, "")
                                .selected_text(&current_value)
                                .show_ui(ui, |ui| {
                                    for option in &field.options {
                                        if ui
                                            .selectable_label(current_value == *option, option)
                                            .clicked()
                                        {
                                            self.form_custom_fields
                                                .insert(field.name.clone(), option.clone());
                                        }
                                    }
                                });
                        }
                        CustomFieldType::Boolean => {
                            let mut checked = current_value == "true";
                            if ui.checkbox(&mut checked, "").changed() {
                                self.form_custom_fields
                                    .insert(field.name.clone(), checked.to_string());
                            }
                        }
                        CustomFieldType::Number => {
                            let mut value = current_value;
                            if ui
                                .add(egui::TextEdit::singleline(&mut value).desired_width(80.0))
                                .changed()
                            {
                                // Basic numeric validation
                                if value.is_empty() || value.parse::<f64>().is_ok() {
                                    self.form_custom_fields.insert(field.name.clone(), value);
                                }
                            }
                        }
                        CustomFieldType::Date => {
                            let mut value = current_value;
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut value)
                                        .desired_width(120.0)
                                        .hint_text("YYYY-MM-DD"),
                                )
                                .changed()
                            {
                                self.form_custom_fields.insert(field.name.clone(), value);
                            }
                        }
                        CustomFieldType::User => {
                            // Show a dropdown of available users
                            let active_users: Vec<_> =
                                self.store.users.iter().filter(|u| !u.archived).collect();
                            let display_text = if current_value.is_empty() {
                                "(select user)".to_string()
                            } else {
                                active_users
                                    .iter()
                                    .find(|u| {
                                        u.id.to_string() == current_value
                                            || u.spec_id.as_deref() == Some(&current_value)
                                    })
                                    .map(|u| u.name.clone())
                                    .unwrap_or_else(|| current_value.clone())
                            };
                            egui::ComboBox::new(format!("{}_user", field.name), "")
                                .selected_text(&display_text)
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(current_value.is_empty(), "(none)")
                                        .clicked()
                                    {
                                        self.form_custom_fields
                                            .insert(field.name.clone(), String::new());
                                    }
                                    for user in &active_users {
                                        let user_id = user
                                            .spec_id
                                            .clone()
                                            .unwrap_or_else(|| user.id.to_string());
                                        if ui
                                            .selectable_label(current_value == user_id, &user.name)
                                            .clicked()
                                        {
                                            self.form_custom_fields
                                                .insert(field.name.clone(), user_id);
                                        }
                                    }
                                });
                        }
                        CustomFieldType::Requirement => {
                            // Show a dropdown of requirements
                            let display_name = if current_value.is_empty() {
                                "(select requirement)".to_string()
                            } else {
                                self.store
                                    .requirements
                                    .iter()
                                    .find(|r| {
                                        r.id.to_string() == current_value
                                            || r.spec_id.as_deref() == Some(&current_value)
                                    })
                                    .map(|r| {
                                        let spec = r.spec_id.as_deref().unwrap_or("N/A");
                                        format!("{} - {}", spec, r.title)
                                    })
                                    .unwrap_or_else(|| current_value.clone())
                            };
                            egui::ComboBox::new(format!("{}_req", field.name), "")
                                .selected_text(&display_name)
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(current_value.is_empty(), "(none)")
                                        .clicked()
                                    {
                                        self.form_custom_fields
                                            .insert(field.name.clone(), String::new());
                                    }
                                    for req in self.store.requirements.iter().take(50) {
                                        // Limit to prevent huge lists
                                        let req_id = req
                                            .spec_id
                                            .clone()
                                            .unwrap_or_else(|| req.id.to_string());
                                        let label = format!(
                                            "{} - {}",
                                            req.spec_id.as_deref().unwrap_or("N/A"),
                                            req.title
                                        );
                                        if ui
                                            .selectable_label(current_value == req_id, label)
                                            .clicked()
                                        {
                                            self.form_custom_fields
                                                .insert(field.name.clone(), req_id);
                                        }
                                    }
                                });
                        }
                    }

                    // Show description tooltip if available
                    if let Some(desc) = &field.description {
                        ui.label("ⓘ").on_hover_text(desc);
                    }
                });
            }
        }

        ui.add_space(8.0);

        // Description field - full width and takes remaining height
        ui.horizontal(|ui| {
            ui.label("Description:");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Preview toggle button
                let preview_label = if self.show_description_preview {
                    "✏ Edit"
                } else {
                    "👁 Preview"
                };
                if ui.button(preview_label).clicked() {
                    self.show_description_preview = !self.show_description_preview;
                }
                if ui
                    .link("Supports Markdown")
                    .on_hover_text("Click for Markdown help")
                    .clicked()
                {
                    self.show_markdown_help = true;
                }
            });
        });

        // Calculate remaining height for description (leave space for buttons)
        let remaining_height = ui.available_height() - 50.0;
        let description_height = remaining_height.max(8.0 * self.current_font_size * 1.4); // At least 8 lines

        egui::ScrollArea::vertical()
            .max_height(description_height)
            .show(ui, |ui| {
                if self.show_description_preview {
                    // Preview mode - render as markdown
                    CommonMarkViewer::new().show(
                        ui,
                        &mut self.markdown_cache,
                        &self.form_description,
                    );
                } else {
                    // Edit mode - text editor with context menu
                    let output = egui::TextEdit::multiline(&mut self.form_description)
                        .desired_width(available_width)
                        .desired_rows(8)
                        .hint_text("Enter requirement description (Markdown supported)...")
                        .show(ui);

                    // Request focus if we came here via double-click on description
                    if self.focus_description {
                        output.response.request_focus();
                        self.focus_description = false;
                    }

                    show_text_context_menu(
                        ui,
                        &output.response,
                        &mut self.form_description,
                        output.response.id,
                        &mut self.last_text_selection,
                    );
                }
            });

        // Auto-sync description's first line to title in Add mode
        self.sync_title_from_description(is_edit);

        ui.add_space(8.0);
        ui.separator();

        // Check for pending save (triggered by Ctrl+S keybinding)
        let should_save = self.pending_save;
        if should_save {
            self.pending_save = false;
        }

        // Check for ESC key to cancel (only if confirmation dialog is not open)
        let esc_pressed =
            !self.show_cancel_confirm_dialog && ui.input(|i| i.key_pressed(egui::Key::Escape));

        ui.horizontal(|ui| {
            if ui.button("💾 Save").clicked() || should_save {
                if is_edit {
                    if let Some(idx) = self.selected_idx {
                        self.update_requirement(idx);
                    }
                } else {
                    self.add_requirement();
                }
            }
            if ui.button("❌ Cancel").clicked() || esc_pressed {
                self.request_form_cancel(is_edit);
            }
        });

        // Show cancel confirmation dialog if there are unsaved changes
        if self.show_cancel_confirm_dialog {
            // ESC while dialog is open should close the dialog (continue editing)
            let dialog_esc = ui.input(|i| i.key_pressed(egui::Key::Escape));

            egui::Window::new("Unsaved Changes")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label("You have unsaved changes. Are you sure you want to cancel?");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Discard Changes").clicked() {
                            self.cancel_form(is_edit);
                        }
                        if ui.button("Continue Editing").clicked() || dialog_esc {
                            self.show_cancel_confirm_dialog = false;
                        }
                    });
                });
        }
    }

    /// Show the form using a layout similar to the Detail View
    /// Title bar at top with editable title and Save/Cancel buttons
    /// Left panel: metadata fields (dropdowns for Type, Status, Priority, etc.)
    /// Right panel: Description editor with markdown preview toggle
    fn show_form_stacked(&mut self, ui: &mut egui::Ui, is_edit: bool) {
        let available_width = ui.available_width();

        // Get title bar styling from theme
        let title_bar_bg = self.user_settings.theme.title_bar_bg();
        let title_bar_font_size = self.user_settings.theme.title_bar_font_size();

        // Check for pending save (triggered by Ctrl+S keybinding)
        let should_save = self.pending_save;
        if should_save {
            self.pending_save = false;
        }

        // Check for ESC key to cancel (only if confirmation dialog is not open)
        let esc_pressed =
            !self.show_cancel_confirm_dialog && ui.input(|i| i.key_pressed(egui::Key::Escape));

        // === TITLE BAR ===
        // Calculate button widths to reserve space (Save + Cancel + spacing)
        let button_space = 180.0; // Approximate space for buttons

        egui::Frame::none()
            .fill(title_bar_bg)
            .inner_margin(egui::Margin::symmetric(8.0, 4.0))
            .rounding(2.0)
            .show(ui, |ui| {
                let available = ui.available_width();
                ui.horizontal(|ui| {
                    // Editable title - use same font size as Detail View (18.0 * multiplier)
                    // Take all available width minus button space
                    let title_width = (available - button_space).max(200.0);
                    let title_response = egui::TextEdit::singleline(&mut self.form_title)
                        .font(egui::FontId::proportional(18.0 * title_bar_font_size))
                        .desired_width(title_width)
                        .hint_text("Enter requirement title...")
                        .show(ui);

                    show_text_context_menu(
                        ui,
                        &title_response.response,
                        &mut self.form_title,
                        title_response.response.id,
                        &mut self.last_text_selection,
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Cancel button
                        if ui.button("❌ Cancel").clicked() || esc_pressed {
                            self.request_form_cancel(is_edit);
                        }

                        // Save button
                        if ui.button("💾 Save").clicked() || should_save {
                            if is_edit {
                                if let Some(idx) = self.selected_idx {
                                    self.update_requirement(idx);
                                }
                            } else {
                                self.add_requirement();
                            }
                        }
                    });
                });
            });

        ui.separator();

        // === MAIN CONTENT: Left panel (metadata) + Right panel (description) ===
        let default_left_width = available_width * 0.25;

        // Track if type changed for status validation
        let old_type = self.form_type.clone();

        // LEFT SIDE: Metadata fields in a resizable panel
        egui::SidePanel::left("form_metadata_panel")
            .resizable(true)
            .default_width(default_left_width)
            .min_width(180.0)
            .max_width(available_width * 0.5)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("form_metadata_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("form_metadata_grid")
                            .num_columns(2)
                            .spacing([10.0, 8.0])
                            .striped(true)
                            .show(ui, |ui| {
                                // ID (for edit mode) - dynamically update based on form_prefix/form_type
                                if is_edit {
                                    if let Some(idx) = self.selected_idx {
                                        if let Some(req) = self.store.requirements.get(idx) {
                                            ui.label("ID:");
                                            // Compute what the ID will be based on current form state
                                            let displayed_id = if let Some(current_spec_id) = &req.spec_id {
                                                // Extract the number part from current spec_id (e.g., "FR-0001" -> "0001")
                                                if let Some(dash_pos) = current_spec_id.rfind('-') {
                                                    let number_part = &current_spec_id[dash_pos..]; // includes the dash
                                                    // Determine the prefix to use
                                                    let effective_prefix = if !self.form_prefix.trim().is_empty() {
                                                        // User has explicitly set a prefix
                                                        self.form_prefix.trim().to_uppercase()
                                                    } else {
                                                        // Use the type's default prefix
                                                        let type_name = format!("{:?}", self.form_type);
                                                        self.store.type_definitions
                                                            .iter()
                                                            .find(|td| td.name == type_name)
                                                            .and_then(|td| td.prefix.clone())
                                                            .unwrap_or_else(|| "REQ".to_string())
                                                    };
                                                    format!("{}{}", effective_prefix, number_part)
                                                } else {
                                                    current_spec_id.clone()
                                                }
                                            } else {
                                                "N/A".to_string()
                                            };
                                            ui.label(&displayed_id);
                                            ui.end_row();
                                        }
                                    }
                                }

                                // ID Prefix
                                ui.label("Prefix:");
                                self.show_prefix_field(ui);
                                ui.end_row();

                                // Type dropdown
                                ui.label("Type:");
                                egui::ComboBox::new("form_type_combo", "")
                                    .selected_text(format!("{:?}", self.form_type))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut self.form_type, RequirementType::Functional, "Functional");
                                        ui.selectable_value(&mut self.form_type, RequirementType::NonFunctional, "NonFunctional");
                                        ui.selectable_value(&mut self.form_type, RequirementType::System, "System");
                                        ui.selectable_value(&mut self.form_type, RequirementType::User, "User");
                                        ui.selectable_value(&mut self.form_type, RequirementType::ChangeRequest, "Change Request");
                                        ui.selectable_value(&mut self.form_type, RequirementType::Bug, "Bug");
                                        ui.separator();
                                        ui.selectable_value(&mut self.form_type, RequirementType::Epic, "Epic");
                                        ui.selectable_value(&mut self.form_type, RequirementType::Story, "Story");
                                        ui.selectable_value(&mut self.form_type, RequirementType::Task, "Task");
                                        ui.selectable_value(&mut self.form_type, RequirementType::Spike, "Spike");
                                        ui.selectable_value(&mut self.form_type, RequirementType::Sprint, "🏃 Sprint");
                                        ui.separator();
                                        ui.selectable_value(&mut self.form_type, RequirementType::Folder, "📁 Folder");
                                    });
                                ui.end_row();

                                // Status and Priority only for stateful types
                                if !self.store.is_type_stateless(&self.form_type) {
                                    // Status dropdown
                                    ui.label("Status:");
                                    let statuses = self.store.get_statuses_for_type(&self.form_type);
                                    egui::ComboBox::new("form_status_combo", "")
                                        .selected_text(&self.form_status_string)
                                        .show_ui(ui, |ui| {
                                            for status in &statuses {
                                                if ui.selectable_label(self.form_status_string == *status, status).clicked() {
                                                    self.form_status_string = status.clone();
                                                }
                                            }
                                        });
                                    ui.end_row();

                                    // Priority dropdown
                                    ui.label("Priority:");
                                    egui::ComboBox::new("form_priority_combo", "")
                                        .selected_text(format!("{:?}", self.form_priority))
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut self.form_priority, RequirementPriority::High, "High");
                                            ui.selectable_value(&mut self.form_priority, RequirementPriority::Medium, "Medium");
                                            ui.selectable_value(&mut self.form_priority, RequirementPriority::Low, "Low");
                                        });
                                    ui.end_row();
                                }

                                // Feature with dropdown
                                ui.label("Feature:");
                                {
                                    let popup_id = ui.make_persistent_id("feature_combo_popup_2");
                                    let is_popup_open = ui.memory(|mem| mem.is_popup_open(popup_id));

                                    let all_features = self.get_all_features();
                                    let search_lower = self.form_feature.to_lowercase();
                                    let filtered: Vec<String> = if search_lower.is_empty() {
                                        all_features.clone()
                                    } else {
                                        all_features.iter()
                                            .filter(|f| f.to_lowercase().contains(&search_lower))
                                            .cloned()
                                            .collect()
                                    };
                                    let num_options = filtered.len();

                                    // Handle keyboard BEFORE the TextEdit
                                    let mut close_and_select = false;
                                    if is_popup_open {
                                        ui.ctx().input_mut(|i| {
                                            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                                                if num_options > 0 && self.form_feature_selected_idx < num_options - 1 {
                                                    self.form_feature_selected_idx += 1;
                                                } else if num_options > 0 {
                                                    self.form_feature_selected_idx = 0;
                                                }
                                            }
                                            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                                                if self.form_feature_selected_idx > 0 {
                                                    self.form_feature_selected_idx -= 1;
                                                } else if num_options > 0 {
                                                    self.form_feature_selected_idx = num_options - 1;
                                                }
                                            }
                                            if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                                                close_and_select = true;
                                            }
                                            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                                                ui.memory_mut(|mem| mem.close_popup());
                                            }
                                        });
                                    }

                                    if close_and_select && !filtered.is_empty() {
                                        if let Some(feature) = filtered.get(self.form_feature_selected_idx) {
                                            self.form_feature = feature.clone();
                                        }
                                        ui.memory_mut(|mem| mem.close_popup());
                                    }

                                    let feature_edit = ui.add(
                                        egui::TextEdit::singleline(&mut self.form_feature)
                                            .desired_width(120.0)
                                            .hint_text("Type to search...")
                                    );

                                    if feature_edit.gained_focus() || (feature_edit.has_focus() && feature_edit.changed()) {
                                        ui.memory_mut(|mem| mem.open_popup(popup_id));
                                        self.form_feature_selected_idx = 0;
                                    }

                                    if self.form_feature_selected_idx >= num_options && num_options > 0 {
                                        self.form_feature_selected_idx = num_options - 1;
                                    }

                                    egui::popup_below_widget(ui, popup_id, &feature_edit, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                                        ui.set_min_width(120.0);
                                        egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                                            if filtered.is_empty() {
                                                ui.weak("No matching features");
                                            } else {
                                                for (idx, feature) in filtered.iter().enumerate() {
                                                    let is_selected = idx == self.form_feature_selected_idx;
                                                    if ui.selectable_label(is_selected, feature).clicked() {
                                                        self.form_feature = feature.clone();
                                                        ui.memory_mut(|mem| mem.close_popup());
                                                    }
                                                }
                                            }
                                        });
                                    });
                                }
                                ui.end_row();

                                // Owner
                                ui.label("Owner:");
                                ui.add(egui::TextEdit::singleline(&mut self.form_owner).desired_width(120.0));
                                ui.end_row();

                                // Tags
                                ui.label("Tags:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.form_tags)
                                        .desired_width(120.0)
                                        .hint_text("comma-separated"),
                                );
                                ui.end_row();

                                // Parent relationship (for new requirements)
                                if !is_edit {
                                    if let Some(parent_id) = self.form_parent_id {
                                        let parent_info = self.store.requirements.iter()
                                            .find(|r| r.id == parent_id)
                                            .map(|r| {
                                                let spec = r.spec_id.as_deref().unwrap_or("N/A");
                                                format!("{}", spec)
                                            });
                                        if let Some(parent_label) = parent_info {
                                            ui.label("Parent:");
                                            ui.horizontal(|ui| {
                                                ui.label(&parent_label);
                                                if ui.small_button("x").on_hover_text("Remove parent").clicked() {
                                                    self.form_parent_id = None;
                                                }
                                            });
                                            ui.end_row();
                                        }
                                    }
                                }
                            });

                        // Custom fields section
                        let custom_fields = self.store.get_custom_fields_for_type(&self.form_type);
                        if !custom_fields.is_empty() {
                            ui.add_space(12.0);
                            ui.separator();
                            ui.label(egui::RichText::new("Custom Fields").strong());
                            ui.add_space(4.0);

                            let mut sorted_fields = custom_fields;
                            sorted_fields.sort_by_key(|f| f.order);

                            egui::Grid::new("form_custom_fields_grid")
                                .num_columns(2)
                                .spacing([10.0, 6.0])
                                .show(ui, |ui| {
                                    for field in sorted_fields {
                                        let label = if field.required {
                                            format!("{}*:", field.label)
                                        } else {
                                            format!("{}:", field.label)
                                        };
                                        ui.label(&label);

                                        let current_value = self.form_custom_fields
                                            .get(&field.name)
                                            .cloned()
                                            .or_else(|| field.default_value.clone())
                                            .unwrap_or_default();

                                        self.show_custom_field_editor(ui, &field, current_value);
                                        ui.end_row();
                                    }
                                });
                        }
                    });
            });

        // RIGHT SIDE: Description editor (takes remaining space)
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show_inside(ui, |ui| {
                // Description header with preview toggle
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Description").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let preview_label = if self.show_description_preview {
                            "✏ Edit"
                        } else {
                            "👁 Preview"
                        };
                        if ui.button(preview_label).clicked() {
                            self.show_description_preview = !self.show_description_preview;
                        }
                        if ui.link("Markdown").on_hover_text("Click for Markdown help").clicked() {
                            self.show_markdown_help = true;
                        }
                    });
                });

                ui.separator();

                // Description content area
                egui::ScrollArea::vertical()
                    .id_salt("form_description_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.show_description_preview {
                            // Preview mode - render as markdown
                            CommonMarkViewer::new().show(
                                ui,
                                &mut self.markdown_cache,
                                &self.form_description,
                            );
                        } else {
                            // Edit mode - text editor
                            let output = egui::TextEdit::multiline(&mut self.form_description)
                                .desired_width(ui.available_width())
                                .desired_rows(20)
                                .hint_text("Enter requirement description (Markdown supported)...")
                                .show(ui);

                            // Request focus if we came here via double-click on description
                            if self.focus_description {
                                output.response.request_focus();
                                self.focus_description = false;
                            }

                            show_text_context_menu(
                                ui,
                                &output.response,
                                &mut self.form_description,
                                output.response.id,
                                &mut self.last_text_selection,
                            );
                        }
                    });
            });

        // Auto-sync description's first line to title in Add mode
        self.sync_title_from_description(is_edit);

        // Handle type change - validate status
        if old_type != self.form_type {
            let statuses = self.store.get_statuses_for_type(&self.form_type);
            if !statuses.contains(&self.form_status_string) {
                self.form_status_string = statuses.first().cloned().unwrap_or_else(|| "Draft".to_string());
            }
            self.form_custom_fields.clear();
        }

        // Show cancel confirmation dialog if there are unsaved changes
        if self.show_cancel_confirm_dialog {
            let dialog_esc = ui.input(|i| i.key_pressed(egui::Key::Escape));

            egui::Window::new("Unsaved Changes")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label("You have unsaved changes. Are you sure you want to cancel?");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Discard Changes").clicked() {
                            self.cancel_form(is_edit);
                        }
                        if ui.button("Continue Editing").clicked() || dialog_esc {
                            self.show_cancel_confirm_dialog = false;
                        }
                    });
                });
        }
    }

    /// Show the form using a vertical layout (matches List|Details side-by-side view)
    /// Title bar at top with editable title and Save/Cancel buttons
    /// Metadata grid below title bar (dropdowns for Type, Status, Priority, etc.)
    /// Description editor at bottom with markdown preview toggle
    fn show_form_vertical(&mut self, ui: &mut egui::Ui, is_edit: bool) {
        // Get title bar styling from theme
        let title_bar_bg = self.user_settings.theme.title_bar_bg();
        let title_bar_font_size = self.user_settings.theme.title_bar_font_size();

        // Check for pending save (triggered by Ctrl+S keybinding)
        let should_save = self.pending_save;
        if should_save {
            self.pending_save = false;
        }

        // Check for ESC key to cancel (only if confirmation dialog is not open)
        let esc_pressed =
            !self.show_cancel_confirm_dialog && ui.input(|i| i.key_pressed(egui::Key::Escape));

        // === TITLE BAR ===
        // Calculate button widths to reserve space (Save + Cancel + spacing)
        let button_space = 180.0; // Approximate space for buttons

        egui::Frame::none()
            .fill(title_bar_bg)
            .inner_margin(egui::Margin::symmetric(8.0, 4.0))
            .rounding(2.0)
            .show(ui, |ui| {
                let available = ui.available_width();
                ui.horizontal(|ui| {
                    // Editable title - use same font size as Detail View (18.0 * multiplier)
                    // Take all available width minus button space
                    let title_width = (available - button_space).max(200.0);
                    let title_response = egui::TextEdit::singleline(&mut self.form_title)
                        .font(egui::FontId::proportional(18.0 * title_bar_font_size))
                        .desired_width(title_width)
                        .hint_text("Enter requirement title...")
                        .show(ui);

                    show_text_context_menu(
                        ui,
                        &title_response.response,
                        &mut self.form_title,
                        title_response.response.id,
                        &mut self.last_text_selection,
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Cancel button
                        if ui.button("❌ Cancel").clicked() || esc_pressed {
                            self.request_form_cancel(is_edit);
                        }

                        // Save button
                        if ui.button("💾 Save").clicked() || should_save {
                            if is_edit {
                                if let Some(idx) = self.selected_idx {
                                    self.update_requirement(idx);
                                }
                            } else {
                                self.add_requirement();
                            }
                        }
                    });
                });
            });

        ui.separator();

        // Track if type changed for status validation
        let old_type = self.form_type.clone();

        // === METADATA GRID ===
        egui::Grid::new("form_metadata_grid_vertical")
            .num_columns(2)
            .spacing([40.0, 8.0])
            .striped(true)
            .show(ui, |ui| {
                // ID (for edit mode) - dynamically update based on form_prefix/form_type
                if is_edit {
                    if let Some(idx) = self.selected_idx {
                        if let Some(req) = self.store.requirements.get(idx) {
                            ui.label("ID:");
                            // Compute what the ID will be based on current form state
                            let displayed_id = if let Some(current_spec_id) = &req.spec_id {
                                // Extract the number part from current spec_id (e.g., "FR-0001" -> "0001")
                                if let Some(dash_pos) = current_spec_id.rfind('-') {
                                    let number_part = &current_spec_id[dash_pos..]; // includes the dash
                                    // Determine the prefix to use
                                    let effective_prefix = if !self.form_prefix.trim().is_empty() {
                                        // User has explicitly set a prefix
                                        self.form_prefix.trim().to_uppercase()
                                    } else {
                                        // Use the type's default prefix
                                        let type_name = format!("{:?}", self.form_type);
                                        self.store.type_definitions
                                            .iter()
                                            .find(|td| td.name == type_name)
                                            .and_then(|td| td.prefix.clone())
                                            .unwrap_or_else(|| "REQ".to_string())
                                    };
                                    format!("{}{}", effective_prefix, number_part)
                                } else {
                                    current_spec_id.clone()
                                }
                            } else {
                                "N/A".to_string()
                            };
                            ui.label(&displayed_id);
                            ui.end_row();
                        }
                    }
                }

                // ID Prefix
                ui.label("Prefix:");
                self.show_prefix_field(ui);
                ui.end_row();

                // Type dropdown
                ui.label("Type:");
                egui::ComboBox::new("form_type_combo_vert", "")
                    .selected_text(format!("{:?}", self.form_type))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.form_type, RequirementType::Functional, "Functional");
                        ui.selectable_value(&mut self.form_type, RequirementType::NonFunctional, "NonFunctional");
                        ui.selectable_value(&mut self.form_type, RequirementType::System, "System");
                        ui.selectable_value(&mut self.form_type, RequirementType::User, "User");
                        ui.selectable_value(&mut self.form_type, RequirementType::ChangeRequest, "Change Request");
                        ui.selectable_value(&mut self.form_type, RequirementType::Bug, "Bug");
                        ui.separator();
                        ui.selectable_value(&mut self.form_type, RequirementType::Epic, "Epic");
                        ui.selectable_value(&mut self.form_type, RequirementType::Story, "Story");
                        ui.selectable_value(&mut self.form_type, RequirementType::Task, "Task");
                        ui.selectable_value(&mut self.form_type, RequirementType::Spike, "Spike");
                        ui.selectable_value(&mut self.form_type, RequirementType::Sprint, "🏃 Sprint");
                        ui.separator();
                        ui.selectable_value(&mut self.form_type, RequirementType::Folder, "📁 Folder");
                    });
                ui.end_row();

                // Status and Priority only for stateful types
                if !self.store.is_type_stateless(&self.form_type) {
                    // Status dropdown
                    ui.label("Status:");
                    let statuses = self.store.get_statuses_for_type(&self.form_type);
                    egui::ComboBox::new("form_status_combo_vert", "")
                        .selected_text(&self.form_status_string)
                        .show_ui(ui, |ui| {
                            for status in &statuses {
                                if ui.selectable_label(self.form_status_string == *status, status).clicked() {
                                    self.form_status_string = status.clone();
                                }
                            }
                        });
                    ui.end_row();

                    // Priority dropdown
                    ui.label("Priority:");
                    egui::ComboBox::new("form_priority_combo_vert", "")
                        .selected_text(format!("{:?}", self.form_priority))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.form_priority, RequirementPriority::High, "High");
                            ui.selectable_value(&mut self.form_priority, RequirementPriority::Medium, "Medium");
                            ui.selectable_value(&mut self.form_priority, RequirementPriority::Low, "Low");
                        });
                    ui.end_row();
                }

                // Feature with dropdown
                ui.label("Feature:");
                {
                    let popup_id = ui.make_persistent_id("feature_combo_popup_3");
                    let is_popup_open = ui.memory(|mem| mem.is_popup_open(popup_id));

                    let all_features = self.get_all_features();
                    let search_lower = self.form_feature.to_lowercase();
                    let filtered: Vec<String> = if search_lower.is_empty() {
                        all_features.clone()
                    } else {
                        all_features.iter()
                            .filter(|f| f.to_lowercase().contains(&search_lower))
                            .cloned()
                            .collect()
                    };
                    let num_options = filtered.len();

                    // Handle keyboard BEFORE the TextEdit
                    let mut close_and_select = false;
                    if is_popup_open {
                        ui.ctx().input_mut(|i| {
                            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                                if num_options > 0 && self.form_feature_selected_idx < num_options - 1 {
                                    self.form_feature_selected_idx += 1;
                                } else if num_options > 0 {
                                    self.form_feature_selected_idx = 0;
                                }
                            }
                            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                                if self.form_feature_selected_idx > 0 {
                                    self.form_feature_selected_idx -= 1;
                                } else if num_options > 0 {
                                    self.form_feature_selected_idx = num_options - 1;
                                }
                            }
                            if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                                close_and_select = true;
                            }
                            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                                ui.memory_mut(|mem| mem.close_popup());
                            }
                        });
                    }

                    if close_and_select && !filtered.is_empty() {
                        if let Some(feature) = filtered.get(self.form_feature_selected_idx) {
                            self.form_feature = feature.clone();
                        }
                        ui.memory_mut(|mem| mem.close_popup());
                    }

                    let feature_edit = ui.add(
                        egui::TextEdit::singleline(&mut self.form_feature)
                            .desired_width(200.0)
                            .hint_text("Type to search...")
                    );

                    if feature_edit.gained_focus() || (feature_edit.has_focus() && feature_edit.changed()) {
                        ui.memory_mut(|mem| mem.open_popup(popup_id));
                        self.form_feature_selected_idx = 0;
                    }

                    if self.form_feature_selected_idx >= num_options && num_options > 0 {
                        self.form_feature_selected_idx = num_options - 1;
                    }

                    egui::popup_below_widget(ui, popup_id, &feature_edit, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                        ui.set_min_width(200.0);
                        egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                            if filtered.is_empty() {
                                ui.weak("No matching features");
                            } else {
                                for (idx, feature) in filtered.iter().enumerate() {
                                    let is_selected = idx == self.form_feature_selected_idx;
                                    if ui.selectable_label(is_selected, feature).clicked() {
                                        self.form_feature = feature.clone();
                                        ui.memory_mut(|mem| mem.close_popup());
                                    }
                                }
                            }
                        });
                    });
                }
                ui.end_row();

                // Owner
                ui.label("Owner:");
                ui.add(egui::TextEdit::singleline(&mut self.form_owner).desired_width(200.0));
                ui.end_row();

                // Tags
                ui.label("Tags:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.form_tags)
                        .desired_width(200.0)
                        .hint_text("comma-separated"),
                );
                ui.end_row();

                // Parent relationship (for new requirements)
                if !is_edit {
                    if let Some(parent_id) = self.form_parent_id {
                        let parent_info = self.store.requirements.iter()
                            .find(|r| r.id == parent_id)
                            .map(|r| {
                                let spec = r.spec_id.as_deref().unwrap_or("N/A");
                                format!("{}", spec)
                            });
                        if let Some(parent_label) = parent_info {
                            ui.label("Parent:");
                            ui.horizontal(|ui| {
                                ui.label(&parent_label);
                                if ui.small_button("x").on_hover_text("Remove parent").clicked() {
                                    self.form_parent_id = None;
                                }
                            });
                            ui.end_row();
                        }
                    }
                }
            });

        // Custom fields section
        let custom_fields = self.store.get_custom_fields_for_type(&self.form_type);
        if !custom_fields.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            ui.label(egui::RichText::new("Custom Fields").strong());
            ui.add_space(4.0);

            let mut sorted_fields = custom_fields;
            sorted_fields.sort_by_key(|f| f.order);

            egui::Grid::new("form_custom_fields_grid_vertical")
                .num_columns(2)
                .spacing([40.0, 6.0])
                .show(ui, |ui| {
                    for field in sorted_fields {
                        let label = if field.required {
                            format!("{}*:", field.label)
                        } else {
                            format!("{}:", field.label)
                        };
                        ui.label(&label);

                        let current_value = self.form_custom_fields
                            .get(&field.name)
                            .cloned()
                            .or_else(|| field.default_value.clone())
                            .unwrap_or_default();

                        self.show_custom_field_editor(ui, &field, current_value);
                        ui.end_row();
                    }
                });
        }

        ui.separator();

        // Handle type change - validate status
        if old_type != self.form_type {
            let statuses = self.store.get_statuses_for_type(&self.form_type);
            if !statuses.contains(&self.form_status_string) {
                self.form_status_string = statuses.first().cloned().unwrap_or_else(|| "Draft".to_string());
            }
            self.form_custom_fields.clear();
        }

        // === DESCRIPTION EDITOR ===
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Description").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let preview_label = if self.show_description_preview {
                    "✏ Edit"
                } else {
                    "👁 Preview"
                };
                if ui.button(preview_label).clicked() {
                    self.show_description_preview = !self.show_description_preview;
                }
                if ui.link("Markdown").on_hover_text("Click for Markdown help").clicked() {
                    self.show_markdown_help = true;
                }
            });
        });

        ui.separator();

        // Description content area with scroll
        egui::ScrollArea::vertical()
            .id_salt("form_description_scroll_vertical")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.show_description_preview {
                    // Preview mode - render as markdown
                    CommonMarkViewer::new().show(
                        ui,
                        &mut self.markdown_cache,
                        &self.form_description,
                    );
                } else {
                    // Edit mode - text editor
                    let output = egui::TextEdit::multiline(&mut self.form_description)
                        .desired_width(ui.available_width())
                        .desired_rows(15)
                        .hint_text("Enter requirement description (Markdown supported)...")
                        .show(ui);

                    // Request focus if we came here via double-click on description
                    if self.focus_description {
                        output.response.request_focus();
                        self.focus_description = false;
                    }

                    show_text_context_menu(
                        ui,
                        &output.response,
                        &mut self.form_description,
                        output.response.id,
                        &mut self.last_text_selection,
                    );
                }
            });

        // Auto-sync description's first line to title in Add mode
        self.sync_title_from_description(is_edit);

        // Show cancel confirmation dialog if there are unsaved changes
        if self.show_cancel_confirm_dialog {
            let dialog_esc = ui.input(|i| i.key_pressed(egui::Key::Escape));

            egui::Window::new("Unsaved Changes")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label("You have unsaved changes. Are you sure you want to cancel?");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Discard Changes").clicked() {
                            self.cancel_form(is_edit);
                        }
                        if ui.button("Continue Editing").clicked() || dialog_esc {
                            self.show_cancel_confirm_dialog = false;
                        }
                    });
                });
        }
    }

    /// Helper to sync title from description (Add mode only)
    /// Called after description changes to auto-fill title from first line
    fn sync_title_from_description(&mut self, is_edit: bool) {
        // Only sync in Add mode (not edit), when auto-sync is enabled
        if is_edit || !self.form_title_auto_synced {
            return;
        }

        // Check if description changed
        if self.form_description != self.form_last_description {
            // Get the first line (before any newline)
            let first_line = self.form_description.lines().next().unwrap_or("");
            let has_newline = self.form_description.contains('\n');

            // Only sync if no newline has been entered yet
            if !has_newline {
                // Truncate to 50 characters with ellipsis if needed
                let new_title = if first_line.chars().count() > 50 {
                    let truncated: String = first_line.chars().take(47).collect();
                    format!("{}...", truncated)
                } else {
                    first_line.to_string()
                };
                self.form_title = new_title;
            }
            // Once a newline is entered, stop auto-sync
            if has_newline {
                self.form_title_auto_synced = false;
            }
            // Update the tracking variable
            self.form_last_description = self.form_description.clone();
        }
    }

    /// Helper to show prefix field (used in both form layouts)
    fn show_prefix_field(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // Always show dropdown with existing prefixes
            let all_prefixes = if self.store.restrict_prefixes && !self.store.allowed_prefixes.is_empty() {
                self.store.allowed_prefixes.clone()
            } else {
                self.store.get_all_prefixes()
            };

            if !all_prefixes.is_empty() {
                egui::ComboBox::new("form_prefix_combo", "")
                    .selected_text(if self.form_prefix.is_empty() { "(default)" } else { &self.form_prefix })
                    .width(80.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(self.form_prefix.is_empty(), "(default)").clicked() {
                            self.form_prefix.clear();
                        }
                        for prefix in &all_prefixes {
                            if ui.selectable_label(self.form_prefix == *prefix, prefix).clicked() {
                                self.form_prefix = prefix.clone();
                            }
                        }
                    });
                ui.label("or");
            }

            // Always show text input for manual entry
            ui.add(egui::TextEdit::singleline(&mut self.form_prefix)
                .desired_width(60.0)
                .hint_text("e.g., SEC"));

            // Show validation status
            let prefix_trimmed = self.form_prefix.trim();
            if !prefix_trimmed.is_empty() {
                if Requirement::validate_prefix(prefix_trimmed).is_some() {
                    ui.label("✓").on_hover_text("Valid prefix");
                } else {
                    ui.colored_label(egui::Color32::RED, "✗")
                        .on_hover_text("Prefix must contain only uppercase letters (A-Z)");
                }
            }
        });
    }

    /// Helper to show custom field editor (used in both form layouts)
    fn show_custom_field_editor(&mut self, ui: &mut egui::Ui, field: &CustomFieldDefinition, current_value: String) {
        match field.field_type {
            CustomFieldType::Text => {
                let mut value = current_value;
                if ui.add(egui::TextEdit::singleline(&mut value).desired_width(120.0)).changed() {
                    self.form_custom_fields.insert(field.name.clone(), value);
                }
            }
            CustomFieldType::TextArea => {
                let mut value = current_value;
                if ui.add(egui::TextEdit::multiline(&mut value).desired_width(120.0).desired_rows(2)).changed() {
                    self.form_custom_fields.insert(field.name.clone(), value);
                }
            }
            CustomFieldType::Select => {
                egui::ComboBox::new(&field.name, "")
                    .selected_text(&current_value)
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        for option in &field.options {
                            if ui.selectable_label(current_value == *option, option).clicked() {
                                self.form_custom_fields.insert(field.name.clone(), option.clone());
                            }
                        }
                    });
            }
            CustomFieldType::Boolean => {
                let mut checked = current_value == "true";
                if ui.checkbox(&mut checked, "").changed() {
                    self.form_custom_fields.insert(field.name.clone(), checked.to_string());
                }
            }
            CustomFieldType::Number => {
                let mut value = current_value;
                if ui.add(egui::TextEdit::singleline(&mut value).desired_width(60.0)).changed() {
                    if value.is_empty() || value.parse::<f64>().is_ok() {
                        self.form_custom_fields.insert(field.name.clone(), value);
                    }
                }
            }
            CustomFieldType::Date => {
                let mut value = current_value;
                if ui.add(egui::TextEdit::singleline(&mut value).desired_width(100.0).hint_text("YYYY-MM-DD")).changed() {
                    self.form_custom_fields.insert(field.name.clone(), value);
                }
            }
            CustomFieldType::User => {
                let active_users: Vec<_> = self.store.users.iter().filter(|u| !u.archived).collect();
                let display_text = if current_value.is_empty() {
                    "(none)".to_string()
                } else {
                    active_users.iter()
                        .find(|u| u.id.to_string() == current_value || u.spec_id.as_deref() == Some(&current_value))
                        .map(|u| u.name.clone())
                        .unwrap_or_else(|| current_value.clone())
                };
                egui::ComboBox::new(format!("{}_user", field.name), "")
                    .selected_text(&display_text)
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current_value.is_empty(), "(none)").clicked() {
                            self.form_custom_fields.insert(field.name.clone(), String::new());
                        }
                        for user in &active_users {
                            let user_id = user.spec_id.clone().unwrap_or_else(|| user.id.to_string());
                            if ui.selectable_label(current_value == user_id, &user.name).clicked() {
                                self.form_custom_fields.insert(field.name.clone(), user_id);
                            }
                        }
                    });
            }
            CustomFieldType::Requirement => {
                let display_name = if current_value.is_empty() {
                    "(none)".to_string()
                } else {
                    self.store.requirements.iter()
                        .find(|r| r.id.to_string() == current_value || r.spec_id.as_deref() == Some(&current_value))
                        .map(|r| r.spec_id.as_deref().unwrap_or("N/A").to_string())
                        .unwrap_or_else(|| current_value.clone())
                };
                egui::ComboBox::new(format!("{}_req", field.name), "")
                    .selected_text(&display_name)
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current_value.is_empty(), "(none)").clicked() {
                            self.form_custom_fields.insert(field.name.clone(), String::new());
                        }
                        for req in self.store.requirements.iter().take(50) {
                            let req_id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
                            let label = req.spec_id.as_deref().unwrap_or("N/A");
                            if ui.selectable_label(current_value == req_id, label).clicked() {
                                self.form_custom_fields.insert(field.name.clone(), req_id);
                            }
                        }
                    });
            }
        }
    }

    fn show_comment_form(&mut self, ui: &mut egui::Ui, _req_idx: usize) {
        let available_width = ui.available_width();

        ui.group(|ui| {
            ui.label(if self.reply_to_comment.is_some() {
                "Add Reply"
            } else {
                "Add Comment"
            });

            ui.horizontal(|ui| {
                ui.label("Author:");
                ui.add(egui::TextEdit::singleline(&mut self.comment_author).desired_width(200.0));
            });

            ui.label("Content:");
            let comment_output = egui::TextEdit::multiline(&mut self.comment_content)
                .desired_width(available_width - 20.0) // Account for group padding
                .desired_rows(4)
                .hint_text("Enter comment...")
                .show(ui);
            show_text_context_menu(
                ui,
                &comment_output.response,
                &mut self.comment_content,
                comment_output.response.id,
                &mut self.last_text_selection,
            );

            ui.horizontal(|ui| {
                if ui.button("💾 Save").clicked() {
                    if !self.comment_author.is_empty() && !self.comment_content.is_empty() {
                        self.pending_comment_add = Some((
                            self.comment_author.clone(),
                            self.comment_content.clone(),
                            self.reply_to_comment,
                        ));
                        self.show_add_comment = false;
                    }
                }
                if ui.button("❌ Cancel").clicked() {
                    self.show_add_comment = false;
                    self.reply_to_comment = None;
                    self.comment_author.clear();
                    self.comment_content.clear();
                }
            });
        });
    }

    fn show_comment_tree(
        &mut self,
        ui: &mut egui::Ui,
        comment: &Comment,
        req_idx: usize,
        depth: usize,
    ) {
        let indent = depth as f32 * 24.0;
        let is_collapsed = self
            .collapsed_comments
            .get(&comment.id)
            .copied()
            .unwrap_or(false);
        let comment_id = comment.id;
        let show_picker = self.show_reaction_picker == Some(comment_id);

        // Calculate available width for the comment (account for indent)
        let available_width = ui.available_width() - indent - 20.0; // 20.0 for group padding

        // Get reaction counts for display
        let reaction_counts = comment.reaction_counts();
        let current_user = self.user_settings.display_name();

        // Build list of reactions the current user has
        let user_reactions: Vec<String> = comment
            .reactions
            .iter()
            .filter(|r| r.author == current_user)
            .map(|r| r.reaction.clone())
            .collect();

        // Get reaction definitions for display
        let reaction_defs: Vec<_> = self.store.reaction_definitions.clone();

        ui.horizontal(|ui| {
            // Add horizontal indentation
            if indent > 0.0 {
                ui.add_space(indent);
            }

            ui.vertical(|ui| {
                ui.set_max_width(available_width);

                ui.group(|ui| {
                    ui.set_max_width(available_width - 16.0); // Account for group border

                    ui.horizontal(|ui| {
                        // Collapse/expand button if there are replies
                        let btn_size = egui::vec2(18.0, 18.0);
                        if !comment.replies.is_empty() {
                            let button_text = if is_collapsed { "+" } else { "-" };
                            if ui
                                .add_sized(btn_size, egui::Button::new(button_text))
                                .clicked()
                            {
                                self.collapsed_comments.insert(comment.id, !is_collapsed);
                            }
                        } else {
                            ui.add_space(btn_size.x + 4.0); // Spacing when no collapse button
                        }

                        ui.label(format!("👤 {}", comment.author));
                        ui.label(format!(
                            "🕒 {}",
                            comment.created_at.format("%Y-%m-%d %H:%M")
                        ));
                    });

                    // Comment content on its own line with text wrapping
                    ui.add(egui::Label::new(&comment.content).wrap());

                    // Display existing reactions
                    if !reaction_counts.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for def in &reaction_defs {
                                if let Some(&count) = reaction_counts.get(&def.name) {
                                    let has_reacted = user_reactions.contains(&def.name);
                                    let label = if has_reacted {
                                        format!("{} {} ✓", def.emoji, count)
                                    } else {
                                        format!("{} {}", def.emoji, count)
                                    };
                                    let btn = egui::Button::new(&label).small();
                                    let response = ui.add(btn).on_hover_text(&def.label);
                                    if response.clicked() {
                                        self.pending_reaction_toggle =
                                            Some((comment_id, def.name.clone()));
                                    }
                                }
                            }
                        });
                    }

                    ui.horizontal(|ui| {
                        if ui.small_button("💬 Reply").clicked() {
                            self.show_add_comment = true;
                            self.reply_to_comment = Some(comment.id);
                            // Pre-fill author from user settings
                            self.comment_author = self.user_settings.display_name();
                            self.comment_content.clear();
                        }

                        // Reaction picker button
                        let react_btn = if show_picker { "😊 ▼" } else { "😊" };
                        if ui
                            .small_button(react_btn)
                            .on_hover_text("Add reaction")
                            .clicked()
                        {
                            if show_picker {
                                self.show_reaction_picker = None;
                            } else {
                                self.show_reaction_picker = Some(comment_id);
                            }
                        }

                        if ui.small_button("🗑 Delete").clicked() {
                            self.pending_comment_delete = Some(comment.id);
                        }
                    });

                    // Show reaction picker if open
                    if show_picker {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("React:");
                            for def in &reaction_defs {
                                let has_reacted = user_reactions.contains(&def.name);
                                let btn_text = if has_reacted {
                                    format!("{} ✓", def.emoji)
                                } else {
                                    def.emoji.clone()
                                };
                                let response = ui.button(&btn_text).on_hover_text(&def.label);
                                if response.clicked() {
                                    self.pending_reaction_toggle =
                                        Some((comment_id, def.name.clone()));
                                    self.show_reaction_picker = None;
                                }
                            }
                        });
                    }
                });
            });
        });

        // Show replies if not collapsed
        if !is_collapsed {
            for reply in &comment.replies {
                self.show_comment_tree(ui, reply, req_idx, depth + 1);
            }
        }
    }

    fn add_comment_to_requirement(
        &mut self,
        idx: usize,
        author: String,
        content: String,
        parent_id: Option<Uuid>,
    ) {
        if let Some(req) = self.store.requirements.get_mut(idx) {
            if let Some(parent) = parent_id {
                // This is a reply
                let reply = Comment::new_reply(author, content, parent);
                if let Err(e) = req.add_reply(parent, reply) {
                    self.message = Some((format!("Error adding reply: {}", e), true));
                    return;
                }
            } else {
                // This is a top-level comment
                let comment = Comment::new(author, content);
                req.add_comment(comment);
            }

            self.save();
            self.comment_author.clear();
            self.comment_content.clear();
            self.reply_to_comment = None;
            self.message = Some(("Comment added successfully".to_string(), false));
        }
    }

    fn delete_comment_from_requirement(&mut self, idx: usize, comment_id: Uuid) {
        if let Some(req) = self.store.requirements.get_mut(idx) {
            if let Err(e) = req.delete_comment(&comment_id) {
                self.message = Some((format!("Error deleting comment: {}", e), true));
                return;
            }
            self.save();
            self.message = Some(("Comment deleted successfully".to_string(), false));
        }
    }

    fn toggle_comment_reaction(&mut self, req_idx: usize, comment_id: Uuid, reaction: &str) {
        let author = self.user_settings.display_name();
        if let Some(req) = self.store.requirements.get_mut(req_idx) {
            // Helper function to toggle reaction on a comment or its nested replies
            fn toggle_in_comments(
                comments: &mut [Comment],
                comment_id: Uuid,
                reaction: &str,
                author: &str,
            ) -> bool {
                for comment in comments.iter_mut() {
                    if comment.id == comment_id {
                        comment.toggle_reaction(reaction, author);
                        return true;
                    }
                    if toggle_in_comments(&mut comment.replies, comment_id, reaction, author) {
                        return true;
                    }
                }
                false
            }

            if toggle_in_comments(&mut req.comments, comment_id, reaction, &author) {
                self.save();
            }
        }
    }

    fn show_migration_confirmation_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_migration_dialog {
            return;
        }

        let Some((new_format, new_numbering, new_digits)) = self.pending_migration.clone() else {
            self.show_migration_dialog = false;
            return;
        };

        let validation =
            self.store
                .validate_id_config_change(&new_format, &new_numbering, new_digits);

        egui::Window::new("⚠ Confirm Migration")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.heading("Migrate Requirement IDs?");
                ui.add_space(10.0);

                ui.label(format!(
                    "This will update {} requirement ID(s) to the new format.",
                    validation.affected_count
                ));
                ui.add_space(5.0);

                ui.label("New format settings:");
                ui.indent("migration_details", |ui| {
                    ui.label(format!("• Format: {}", match new_format {
                        IdFormat::SingleLevel => "Single Level (PREFIX-NNN)",
                        IdFormat::TwoLevel => "Two Level (FEATURE-TYPE-NNN)",
                    }));
                    ui.label(format!("• Numbering: {}", match new_numbering {
                        NumberingStrategy::Global => "Global Sequential",
                        NumberingStrategy::PerPrefix => "Per Prefix",
                        NumberingStrategy::PerFeatureType => "Per Feature+Type",
                    }));
                    ui.label(format!("• Digits: {}", new_digits));
                });

                ui.add_space(10.0);
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "This action cannot be undone. Make sure to backup your requirements file first."
                );

                ui.add_space(15.0);
                ui.horizontal(|ui| {
                    if ui.button("✅ Migrate").clicked() {
                        // Perform the migration
                        let migrated = self.store.migrate_ids_to_config(
                            new_format,
                            new_numbering,
                            new_digits,
                        );

                        // Update form fields to reflect new state
                        self.settings_form_id_format = self.store.id_config.format.clone();
                        self.settings_form_numbering = self.store.id_config.numbering.clone();
                        self.settings_form_digits = self.store.id_config.digits;

                        // Save to file
                        match self.storage.save(&self.store) {
                            Ok(()) => {
                                self.message = Some((
                                    format!("Successfully migrated {} requirement ID(s)", migrated),
                                    false
                                ));
                            }
                            Err(e) => {
                                self.message = Some((
                                    format!("Migration completed but failed to save: {}", e),
                                    true
                                ));
                            }
                        }

                        self.show_migration_dialog = false;
                        self.pending_migration = None;
                    }

                    if ui.button("❌ Cancel").clicked() {
                        self.show_migration_dialog = false;
                        self.pending_migration = None;
                    }
                });
            });
    }

    fn show_save_preset_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_save_preset_dialog {
            return;
        }

        egui::Window::new("💾 Save View Preset")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(300.0);

                ui.label("Enter a name for this view preset:");
                ui.add_space(5.0);

                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.preset_name_input)
                        .hint_text("Preset name")
                        .desired_width(280.0),
                );

                // Focus the text field when dialog opens
                if response.gained_focus() || self.preset_name_input.is_empty() {
                    response.request_focus();
                }

                // Check if name already exists
                let name_exists = self
                    .user_settings
                    .view_presets
                    .iter()
                    .any(|p| p.name == self.preset_name_input);

                if name_exists {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "⚠ This will overwrite existing preset",
                    );
                }

                ui.add_space(10.0);

                // Show current view settings summary
                ui.group(|ui| {
                    ui.label("Current View Settings:");
                    ui.label(format!("  Perspective: {}", self.perspective.label()));
                    if self.perspective != Perspective::Flat {
                        ui.label(format!(
                            "  Direction: {}",
                            self.perspective_direction.label()
                        ));
                    }
                    if !self.filter_types.is_empty() {
                        let types: Vec<_> = self
                            .filter_types
                            .iter()
                            .map(|t| format!("{:?}", t))
                            .collect();
                        ui.label(format!("  Type filters: {}", types.join(", ")));
                    }
                    if !self.filter_features.is_empty() {
                        let features: Vec<_> = self.filter_features.iter().cloned().collect();
                        ui.label(format!("  Feature filters: {}", features.join(", ")));
                    }
                });

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    let can_save = !self.preset_name_input.trim().is_empty();

                    if ui
                        .add_enabled(can_save, egui::Button::new("Save"))
                        .clicked()
                        || (can_save && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        let name = self.preset_name_input.trim().to_string();
                        self.save_current_view_as_preset(name);
                        self.show_save_preset_dialog = false;
                        self.preset_name_input.clear();
                    }

                    if ui.button("Cancel").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        self.show_save_preset_dialog = false;
                        self.preset_name_input.clear();
                    }
                });
            });
    }

    fn show_split_save_preset_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_split_save_preset_dialog {
            return;
        }

        egui::Window::new("💾 Save View Preset - List 2")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(300.0);

                ui.label("Enter a name for this view preset:");
                ui.add_space(5.0);

                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.preset_name_input)
                        .hint_text("Preset name")
                        .desired_width(280.0),
                );

                // Focus the text field when dialog opens
                if response.gained_focus() || self.preset_name_input.is_empty() {
                    response.request_focus();
                }

                // Check if name already exists
                let name_exists = self
                    .user_settings
                    .view_presets
                    .iter()
                    .any(|p| p.name == self.preset_name_input);

                if name_exists {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "⚠ This will overwrite existing preset",
                    );
                }

                ui.add_space(10.0);

                // Show current view settings summary
                ui.group(|ui| {
                    ui.label("Current View Settings (List 2):");
                    ui.label(format!("  Perspective: {}", self.split_perspective.label()));
                    if self.split_perspective != Perspective::Flat {
                        ui.label(format!(
                            "  Direction: {}",
                            self.split_perspective_direction.label()
                        ));
                    }
                    if !self.split_filter_types.is_empty() {
                        let types: Vec<_> = self
                            .split_filter_types
                            .iter()
                            .map(|t| format!("{:?}", t))
                            .collect();
                        ui.label(format!("  Type filters: {}", types.join(", ")));
                    }
                    if !self.split_filter_features.is_empty() {
                        let features: Vec<_> = self.split_filter_features.iter().cloned().collect();
                        ui.label(format!("  Feature filters: {}", features.join(", ")));
                    }
                });

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    let can_save = !self.preset_name_input.trim().is_empty();

                    if ui
                        .add_enabled(can_save, egui::Button::new("Save"))
                        .clicked()
                        || (can_save && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        let name = self.preset_name_input.trim().to_string();
                        self.save_split_view_as_preset(name);
                        self.show_split_save_preset_dialog = false;
                        self.preset_name_input.clear();
                    }

                    if ui.button("Cancel").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        self.show_split_save_preset_dialog = false;
                        self.preset_name_input.clear();
                    }
                });
            });
    }

    fn show_delete_preset_confirmation_dialog(&mut self, ctx: &egui::Context) {
        let preset_name = match &self.show_delete_preset_confirm {
            Some(name) => name.clone(),
            None => return,
        };

        egui::Window::new("⚠ Delete Preset")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(300.0);

                ui.label(format!("Delete preset \"{}\"?", preset_name));
                ui.add_space(5.0);
                ui.label("This action cannot be undone.");

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        self.delete_preset(&preset_name);
                        self.show_delete_preset_confirm = None;
                    }

                    if ui.button("Cancel").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        self.show_delete_preset_confirm = None;
                    }
                });
            });
    }

    /// Show delete requirement confirmation dialog (triggered by 'd' key)
    fn show_delete_requirement_confirmation_dialog(&mut self, ctx: &egui::Context) {
        let idx = match self.pending_delete_confirm {
            Some(i) => i,
            None => return,
        };

        // Get the requirement info for display
        let req_info = self.store.requirements.get(idx).map(|r| {
            let spec_id = r.spec_id.clone().unwrap_or_else(|| format!("#{}", r.id.to_string().get(..8).unwrap_or("?")));
            let title = if r.title.len() > 40 {
                format!("{}...", &r.title.chars().take(40).collect::<String>())
            } else {
                r.title.clone()
            };
            (spec_id, title)
        });

        let Some((spec_id, title)) = req_info else {
            self.pending_delete_confirm = None;
            return;
        };

        // Handle keyboard shortcuts
        let mut do_delete = false;
        let mut do_cancel = false;
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::N) {
                do_cancel = true;
            }
            if i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Y) {
                do_delete = true;
            }
        });

        if do_cancel {
            self.pending_delete_confirm = None;
            return;
        }

        if do_delete {
            self.delete_requirement(idx);
            self.pending_delete_confirm = None;
            return;
        }

        egui::Window::new("⚠ Delete Requirement")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(350.0);

                ui.label(format!("Delete {} \"{}\"?", spec_id, title));
                ui.add_space(5.0);
                ui.label(egui::RichText::new("This action cannot be undone.").weak());

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if ui.button("🗑 Delete (y)").clicked() {
                        self.delete_requirement(idx);
                        self.pending_delete_confirm = None;
                    }

                    if ui.button("Cancel (n)").clicked() {
                        self.pending_delete_confirm = None;
                    }
                });

                ui.add_space(5.0);
                ui.small("Press Y to confirm, N or Esc to cancel");
            });
    }

    /// Show view settings dialog for List 1
    fn show_filter_dialog_list1(&mut self, ctx: &egui::Context) {
        if !self.show_filter_dialog_list1 {
            return;
        }

        let mut open = true;
        let max_size = modal_max_size(ctx);

        egui::Window::new("View Settings")
            .collapsible(false)
            .resizable(true)
            .default_width(450.0)
            .max_width(max_size.x)
            .max_height(max_size.y)
            .scroll([false, true])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                // View mode section
                ui.heading("View Mode");
                ui.add_space(5.0);

                ui.horizontal(|ui| {
                    ui.label("Perspective:");
                    egui::ComboBox::from_id_salt("view_dialog_perspective")
                        .selected_text(self.perspective.label())
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(
                                self.perspective == Perspective::Flat,
                                Perspective::Flat.label(),
                            ).clicked() {
                                self.perspective = Perspective::Flat;
                                self.active_preset = None;
                            }
                            if ui.selectable_label(
                                self.perspective == Perspective::ParentChild,
                                Perspective::ParentChild.label(),
                            ).clicked() {
                                self.perspective = Perspective::ParentChild;
                                self.active_preset = None;
                            }
                            if ui.selectable_label(
                                self.perspective == Perspective::Verification,
                                Perspective::Verification.label(),
                            ).clicked() {
                                self.perspective = Perspective::Verification;
                                self.active_preset = None;
                            }
                            if ui.selectable_label(
                                self.perspective == Perspective::References,
                                Perspective::References.label(),
                            ).clicked() {
                                self.perspective = Perspective::References;
                                self.active_preset = None;
                            }
                            if ui.selectable_label(
                                self.perspective == Perspective::ByOwner,
                                Perspective::ByOwner.label(),
                            ).clicked() {
                                self.perspective = Perspective::ByOwner;
                                self.active_preset = None;
                            }
                        });
                });

                // Direction selector (only for non-Flat perspectives)
                if self.perspective != Perspective::Flat && !self.perspective.uses_user_roots() {
                    ui.horizontal(|ui| {
                        ui.label("Direction:");
                        egui::ComboBox::from_id_salt("view_dialog_direction")
                            .selected_text(self.perspective_direction.label())
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(
                                    self.perspective_direction == PerspectiveDirection::TopDown,
                                    PerspectiveDirection::TopDown.label(),
                                ).clicked() {
                                    self.perspective_direction = PerspectiveDirection::TopDown;
                                    self.active_preset = None;
                                }
                                if ui.selectable_label(
                                    self.perspective_direction == PerspectiveDirection::BottomUp,
                                    PerspectiveDirection::BottomUp.label(),
                                ).clicked() {
                                    self.perspective_direction = PerspectiveDirection::BottomUp;
                                    self.active_preset = None;
                                }
                            });
                    });
                }

                // Owner view mode selector (only for ByOwner perspective)
                if self.perspective.uses_user_roots() {
                    ui.horizontal(|ui| {
                        ui.label("Sub-hierarchy:");
                        egui::ComboBox::from_id_salt("view_dialog_owner_mode")
                            .selected_text(match self.owner_view_mode {
                                OwnerViewMode::Flat => "Flat",
                                OwnerViewMode::Nested => "Nested (Parent/Child)",
                            })
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(
                                    self.owner_view_mode == OwnerViewMode::Flat,
                                    "Flat",
                                ).clicked() {
                                    self.owner_view_mode = OwnerViewMode::Flat;
                                }
                                if ui.selectable_label(
                                    self.owner_view_mode == OwnerViewMode::Nested,
                                    "Nested (Parent/Child)",
                                ).clicked() {
                                    self.owner_view_mode = OwnerViewMode::Nested;
                                }
                            });
                    });
                    ui.add_space(2.0);
                    ui.label("Flat: Shows all owned requirements directly under owner.\nNested: Preserves parent/child hierarchy within owned requirements.");
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(5.0);

                // Filters section
                ui.heading("Filters");
                ui.add_space(5.0);

                // Collect data needed for popups
                let features = self.get_all_features();
                let all_prefixes = self.store.get_all_prefixes();
                // Filter out type prefixes from ID prefix list
                let type_prefixes: std::collections::HashSet<String> = self.store.id_config.reserved_prefixes()
                    .into_iter()
                    .collect();
                let prefixes: Vec<String> = all_prefixes
                    .into_iter()
                    .filter(|p| !type_prefixes.contains(p))
                    .collect();

                // Type Filters dropdown
                ui.horizontal(|ui| {
                    ui.label("Types:");
                    let type_count = self.filter_types.len();
                    let type_label = if type_count == 0 {
                        "All Types ▼".to_string()
                    } else {
                        format!("{} selected ▼", type_count)
                    };
                    let type_response = ui.button(&type_label);
                    let popup_id = ui.make_persistent_id("type_filter_popup");
                    if type_response.clicked() {
                        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
                    }
                    egui::popup_below_widget(ui, popup_id, &type_response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                        ui.set_min_width(150.0);
                        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                            let types = [
                                (RequirementType::Functional, "Functional (FR)"),
                                (RequirementType::NonFunctional, "Non-Functional (NFR)"),
                                (RequirementType::System, "System (SR)"),
                                (RequirementType::User, "User (UR)"),
                                (RequirementType::ChangeRequest, "Change Request (CR)"),
                                (RequirementType::Bug, "Bug"),
                                (RequirementType::Epic, "Epic"),
                                (RequirementType::Story, "Story"),
                                (RequirementType::Task, "Task"),
                                (RequirementType::Spike, "Spike"),
                                (RequirementType::Sprint, "Sprint"),
                            ];
                            for (req_type, label) in types {
                                let mut checked = self.filter_types.contains(&req_type);
                                if ui.checkbox(&mut checked, label).changed() {
                                    if checked {
                                        self.filter_types.insert(req_type);
                                    } else {
                                        self.filter_types.remove(&req_type);
                                    }
                                    self.active_preset = None;
                                }
                            }
                            ui.separator();
                            if ui.button("Clear All").clicked() {
                                self.filter_types.clear();
                                self.active_preset = None;
                            }
                        });
                    });
                    if type_count > 0 {
                        if ui.small_button("✕").on_hover_text("Clear type filters").clicked() {
                            self.filter_types.clear();
                            self.active_preset = None;
                        }
                    }
                });

                // Status Filters dropdown
                ui.horizontal(|ui| {
                    ui.label("Status:");
                    let status_count = self.filter_statuses.len();
                    let status_label = if status_count == 0 {
                        "All Statuses ▼".to_string()
                    } else {
                        format!("{} selected ▼", status_count)
                    };
                    let status_response = ui.button(&status_label);
                    let popup_id = ui.make_persistent_id("status_filter_popup");
                    if status_response.clicked() {
                        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
                    }
                    egui::popup_below_widget(ui, popup_id, &status_response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                        ui.set_min_width(120.0);
                        let statuses = [
                            (RequirementStatus::Draft, "Draft"),
                            (RequirementStatus::Approved, "Approved"),
                            (RequirementStatus::Completed, "Completed"),
                            (RequirementStatus::Rejected, "Rejected"),
                        ];
                        for (status, label) in statuses {
                            let mut checked = self.filter_statuses.contains(&status);
                            if ui.checkbox(&mut checked, label).changed() {
                                if checked {
                                    self.filter_statuses.insert(status);
                                } else {
                                    self.filter_statuses.remove(&status);
                                }
                                self.active_preset = None;
                            }
                        }
                        ui.separator();
                        if ui.button("Clear All").clicked() {
                            self.filter_statuses.clear();
                            self.active_preset = None;
                        }
                    });
                    if status_count > 0 {
                        if ui.small_button("✕").on_hover_text("Clear status filters").clicked() {
                            self.filter_statuses.clear();
                            self.active_preset = None;
                        }
                    }
                });

                // Priority Filters dropdown
                ui.horizontal(|ui| {
                    ui.label("Priority:");
                    let priority_count = self.filter_priorities.len();
                    let priority_label = if priority_count == 0 {
                        "All Priorities ▼".to_string()
                    } else {
                        format!("{} selected ▼", priority_count)
                    };
                    let priority_response = ui.button(&priority_label);
                    let popup_id = ui.make_persistent_id("priority_filter_popup");
                    if priority_response.clicked() {
                        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
                    }
                    egui::popup_below_widget(ui, popup_id, &priority_response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                        ui.set_min_width(100.0);
                        let priorities = [
                            (RequirementPriority::High, "High"),
                            (RequirementPriority::Medium, "Medium"),
                            (RequirementPriority::Low, "Low"),
                        ];
                        for (priority, label) in priorities {
                            let mut checked = self.filter_priorities.contains(&priority);
                            if ui.checkbox(&mut checked, label).changed() {
                                if checked {
                                    self.filter_priorities.insert(priority);
                                } else {
                                    self.filter_priorities.remove(&priority);
                                }
                                self.active_preset = None;
                            }
                        }
                        ui.separator();
                        if ui.button("Clear All").clicked() {
                            self.filter_priorities.clear();
                            self.active_preset = None;
                        }
                    });
                    if priority_count > 0 {
                        if ui.small_button("✕").on_hover_text("Clear priority filters").clicked() {
                            self.filter_priorities.clear();
                            self.active_preset = None;
                        }
                    }
                });

                // Feature Filters dropdown (only if there are features)
                if !features.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label("Features:");
                        let feature_count = self.filter_features.len();
                        let feature_label = if feature_count == 0 {
                            "All Features ▼".to_string()
                        } else {
                            format!("{} selected ▼", feature_count)
                        };
                        let feature_response = ui.button(&feature_label);
                        let popup_id = ui.make_persistent_id("feature_filter_popup");
                        if feature_response.clicked() {
                            ui.memory_mut(|mem| mem.toggle_popup(popup_id));
                        }
                        egui::popup_below_widget(ui, popup_id, &feature_response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                            ui.set_min_width(180.0);
                            egui::ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
                                for feature in &features {
                                    let mut checked = self.filter_features.contains(feature);
                                    if ui.checkbox(&mut checked, feature).changed() {
                                        if checked {
                                            self.filter_features.insert(feature.clone());
                                        } else {
                                            self.filter_features.remove(feature);
                                        }
                                        self.active_preset = None;
                                    }
                                }
                                ui.separator();
                                if ui.button("Clear All").clicked() {
                                    self.filter_features.clear();
                                    self.active_preset = None;
                                }
                            });
                        });
                        if feature_count > 0 {
                            if ui.small_button("✕").on_hover_text("Clear feature filters").clicked() {
                                self.filter_features.clear();
                                self.active_preset = None;
                            }
                        }
                    });
                }

                // ID Prefix Filters dropdown (only if there are non-type prefixes)
                if !prefixes.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label("ID Prefixes:");
                        let prefix_count = self.filter_prefixes.len();
                        let prefix_label = if prefix_count == 0 {
                            "All Prefixes ▼".to_string()
                        } else {
                            format!("{} selected ▼", prefix_count)
                        };
                        let prefix_response = ui.button(&prefix_label);
                        let popup_id = ui.make_persistent_id("prefix_filter_popup");
                        if prefix_response.clicked() {
                            ui.memory_mut(|mem| mem.toggle_popup(popup_id));
                        }
                        egui::popup_below_widget(ui, popup_id, &prefix_response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                            ui.set_min_width(150.0);
                            egui::ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
                                for prefix in &prefixes {
                                    let mut checked = self.filter_prefixes.contains(prefix);
                                    if ui.checkbox(&mut checked, prefix).changed() {
                                        if checked {
                                            self.filter_prefixes.insert(prefix.clone());
                                        } else {
                                            self.filter_prefixes.remove(prefix);
                                        }
                                        self.active_preset = None;
                                    }
                                }
                                ui.separator();
                                if ui.button("Clear All").clicked() {
                                    self.filter_prefixes.clear();
                                    self.active_preset = None;
                                }
                            });
                        });
                        if prefix_count > 0 {
                            if ui.small_button("✕").on_hover_text("Clear prefix filters").clicked() {
                                self.filter_prefixes.clear();
                                self.active_preset = None;
                            }
                        }
                    });
                }

                ui.add_space(10.0);
                ui.separator();

                // Show Archived and Show Parents with visible frames for better visibility
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(4.0, 2.0))
                    .rounding(egui::Rounding::same(3.0))
                    .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.show_archived, "Show Archived");
                            ui.add_space(10.0);
                            ui.checkbox(&mut self.show_filtered_parents, "Show Parents")
                                .on_hover_text("Show greyed-out parent requirements for filtered items");
                        });
                    });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(5.0);

                // Action buttons
                ui.horizontal(|ui| {
                    if ui.button("Clear All Filters").clicked() {
                        self.filter_types.clear();
                        self.filter_features.clear();
                        self.filter_prefixes.clear();
                        self.filter_statuses.clear();
                        self.filter_priorities.clear();
                        self.active_preset = None;
                    }

                    if ui.button("💾 Save As...").clicked() {
                        self.show_save_preset_dialog = true;
                    }

                    if ui.button("Close").clicked() {
                        self.show_filter_dialog_list1 = false;
                    }
                });
            });

        if !open {
            self.show_filter_dialog_list1 = false;
        }
    }

    /// Show view settings dialog for List 2
    fn show_filter_dialog_list2(&mut self, ctx: &egui::Context) {
        if !self.show_filter_dialog_list2 {
            return;
        }

        let mut open = true;
        let max_size = modal_max_size(ctx);

        egui::Window::new("View Settings - List 2")
            .collapsible(false)
            .resizable(true)
            .default_width(450.0)
            .max_width(max_size.x)
            .max_height(max_size.y)
            .scroll([false, true])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                // View mode section
                ui.heading("View Mode");
                ui.add_space(5.0);

                ui.horizontal(|ui| {
                    ui.label("Perspective:");
                    egui::ComboBox::from_id_salt("view_dialog_perspective_list2")
                        .selected_text(self.split_perspective.label())
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(
                                self.split_perspective == Perspective::Flat,
                                Perspective::Flat.label(),
                            ).clicked() {
                                self.split_perspective = Perspective::Flat;
                                self.split_active_preset = None;
                            }
                            if ui.selectable_label(
                                self.split_perspective == Perspective::ParentChild,
                                Perspective::ParentChild.label(),
                            ).clicked() {
                                self.split_perspective = Perspective::ParentChild;
                                self.split_active_preset = None;
                            }
                            if ui.selectable_label(
                                self.split_perspective == Perspective::Verification,
                                Perspective::Verification.label(),
                            ).clicked() {
                                self.split_perspective = Perspective::Verification;
                                self.split_active_preset = None;
                            }
                            if ui.selectable_label(
                                self.split_perspective == Perspective::References,
                                Perspective::References.label(),
                            ).clicked() {
                                self.split_perspective = Perspective::References;
                                self.split_active_preset = None;
                            }
                            if ui.selectable_label(
                                self.split_perspective == Perspective::ByOwner,
                                Perspective::ByOwner.label(),
                            ).clicked() {
                                self.split_perspective = Perspective::ByOwner;
                                self.split_active_preset = None;
                            }
                        });
                });

                // Direction selector (only for non-Flat perspectives and non-ByOwner)
                if self.split_perspective != Perspective::Flat && !self.split_perspective.uses_user_roots() {
                    ui.horizontal(|ui| {
                        ui.label("Direction:");
                        egui::ComboBox::from_id_salt("view_dialog_direction_list2")
                            .selected_text(self.split_perspective_direction.label())
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(
                                    self.split_perspective_direction == PerspectiveDirection::TopDown,
                                    PerspectiveDirection::TopDown.label(),
                                ).clicked() {
                                    self.split_perspective_direction = PerspectiveDirection::TopDown;
                                    self.split_active_preset = None;
                                }
                                if ui.selectable_label(
                                    self.split_perspective_direction == PerspectiveDirection::BottomUp,
                                    PerspectiveDirection::BottomUp.label(),
                                ).clicked() {
                                    self.split_perspective_direction = PerspectiveDirection::BottomUp;
                                    self.split_active_preset = None;
                                }
                            });
                    });
                }

                // Owner view mode selector (only for ByOwner perspective)
                if self.split_perspective.uses_user_roots() {
                    ui.horizontal(|ui| {
                        ui.label("Sub-hierarchy:");
                        egui::ComboBox::from_id_salt("view_dialog_owner_mode_list2")
                            .selected_text(match self.owner_view_mode {
                                OwnerViewMode::Flat => "Flat",
                                OwnerViewMode::Nested => "Nested (Parent/Child)",
                            })
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(
                                    self.owner_view_mode == OwnerViewMode::Flat,
                                    "Flat",
                                ).clicked() {
                                    self.owner_view_mode = OwnerViewMode::Flat;
                                }
                                if ui.selectable_label(
                                    self.owner_view_mode == OwnerViewMode::Nested,
                                    "Nested (Parent/Child)",
                                ).clicked() {
                                    self.owner_view_mode = OwnerViewMode::Nested;
                                }
                            });
                    });
                    ui.add_space(2.0);
                    ui.label("Flat: Shows all owned requirements directly under owner.\nNested: Preserves parent/child hierarchy within owned requirements.");
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(5.0);

                // Filters section
                ui.heading("Filters");
                ui.add_space(5.0);

                // Collect data needed for popups
                let features = self.get_all_features();
                let all_prefixes = self.store.get_all_prefixes();
                // Filter out type prefixes from ID prefix list
                let type_prefixes: std::collections::HashSet<String> = self.store.id_config.reserved_prefixes()
                    .into_iter()
                    .collect();
                let prefixes: Vec<String> = all_prefixes
                    .into_iter()
                    .filter(|p| !type_prefixes.contains(p))
                    .collect();

                // Type Filters dropdown
                ui.horizontal(|ui| {
                    ui.label("Types:");
                    let type_count = self.split_filter_types.len();
                    let type_label = if type_count == 0 {
                        "All Types ▼".to_string()
                    } else {
                        format!("{} selected ▼", type_count)
                    };
                    let type_response = ui.button(&type_label);
                    let popup_id = ui.make_persistent_id("type_filter_popup_list2");
                    if type_response.clicked() {
                        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
                    }
                    egui::popup_below_widget(ui, popup_id, &type_response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                        ui.set_min_width(150.0);
                        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                            let types = [
                                (RequirementType::Functional, "Functional (FR)"),
                                (RequirementType::NonFunctional, "Non-Functional (NFR)"),
                                (RequirementType::System, "System (SR)"),
                                (RequirementType::User, "User (UR)"),
                                (RequirementType::ChangeRequest, "Change Request (CR)"),
                                (RequirementType::Bug, "Bug"),
                                (RequirementType::Epic, "Epic"),
                                (RequirementType::Story, "Story"),
                                (RequirementType::Task, "Task"),
                                (RequirementType::Spike, "Spike"),
                                (RequirementType::Sprint, "Sprint"),
                            ];
                            for (req_type, label) in types {
                                let mut checked = self.split_filter_types.contains(&req_type);
                                if ui.checkbox(&mut checked, label).changed() {
                                    if checked {
                                        self.split_filter_types.insert(req_type);
                                    } else {
                                        self.split_filter_types.remove(&req_type);
                                    }
                                    self.split_active_preset = None;
                                }
                            }
                            ui.separator();
                            if ui.button("Clear All").clicked() {
                                self.split_filter_types.clear();
                                self.split_active_preset = None;
                            }
                        });
                    });
                    if type_count > 0 {
                        if ui.small_button("✕").on_hover_text("Clear type filters").clicked() {
                            self.split_filter_types.clear();
                            self.split_active_preset = None;
                        }
                    }
                });

                // Status Filters dropdown
                ui.horizontal(|ui| {
                    ui.label("Status:");
                    let status_count = self.split_filter_statuses.len();
                    let status_label = if status_count == 0 {
                        "All Statuses ▼".to_string()
                    } else {
                        format!("{} selected ▼", status_count)
                    };
                    let status_response = ui.button(&status_label);
                    let popup_id = ui.make_persistent_id("status_filter_popup_list2");
                    if status_response.clicked() {
                        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
                    }
                    egui::popup_below_widget(ui, popup_id, &status_response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                        ui.set_min_width(120.0);
                        let statuses = [
                            (RequirementStatus::Draft, "Draft"),
                            (RequirementStatus::Approved, "Approved"),
                            (RequirementStatus::Completed, "Completed"),
                            (RequirementStatus::Rejected, "Rejected"),
                        ];
                        for (status, label) in statuses {
                            let mut checked = self.split_filter_statuses.contains(&status);
                            if ui.checkbox(&mut checked, label).changed() {
                                if checked {
                                    self.split_filter_statuses.insert(status);
                                } else {
                                    self.split_filter_statuses.remove(&status);
                                }
                                self.split_active_preset = None;
                            }
                        }
                        ui.separator();
                        if ui.button("Clear All").clicked() {
                            self.split_filter_statuses.clear();
                            self.split_active_preset = None;
                        }
                    });
                    if status_count > 0 {
                        if ui.small_button("✕").on_hover_text("Clear status filters").clicked() {
                            self.split_filter_statuses.clear();
                            self.split_active_preset = None;
                        }
                    }
                });

                // Priority Filters dropdown
                ui.horizontal(|ui| {
                    ui.label("Priority:");
                    let priority_count = self.split_filter_priorities.len();
                    let priority_label = if priority_count == 0 {
                        "All Priorities ▼".to_string()
                    } else {
                        format!("{} selected ▼", priority_count)
                    };
                    let priority_response = ui.button(&priority_label);
                    let popup_id = ui.make_persistent_id("priority_filter_popup_list2");
                    if priority_response.clicked() {
                        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
                    }
                    egui::popup_below_widget(ui, popup_id, &priority_response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                        ui.set_min_width(100.0);
                        let priorities = [
                            (RequirementPriority::High, "High"),
                            (RequirementPriority::Medium, "Medium"),
                            (RequirementPriority::Low, "Low"),
                        ];
                        for (priority, label) in priorities {
                            let mut checked = self.split_filter_priorities.contains(&priority);
                            if ui.checkbox(&mut checked, label).changed() {
                                if checked {
                                    self.split_filter_priorities.insert(priority);
                                } else {
                                    self.split_filter_priorities.remove(&priority);
                                }
                                self.split_active_preset = None;
                            }
                        }
                        ui.separator();
                        if ui.button("Clear All").clicked() {
                            self.split_filter_priorities.clear();
                            self.split_active_preset = None;
                        }
                    });
                    if priority_count > 0 {
                        if ui.small_button("✕").on_hover_text("Clear priority filters").clicked() {
                            self.split_filter_priorities.clear();
                            self.split_active_preset = None;
                        }
                    }
                });

                // Feature Filters dropdown (only if there are features)
                if !features.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label("Features:");
                        let feature_count = self.split_filter_features.len();
                        let feature_label = if feature_count == 0 {
                            "All Features ▼".to_string()
                        } else {
                            format!("{} selected ▼", feature_count)
                        };
                        let feature_response = ui.button(&feature_label);
                        let popup_id = ui.make_persistent_id("feature_filter_popup_list2");
                        if feature_response.clicked() {
                            ui.memory_mut(|mem| mem.toggle_popup(popup_id));
                        }
                        egui::popup_below_widget(ui, popup_id, &feature_response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                            ui.set_min_width(180.0);
                            egui::ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
                                for feature in &features {
                                    let mut checked = self.split_filter_features.contains(feature);
                                    if ui.checkbox(&mut checked, feature).changed() {
                                        if checked {
                                            self.split_filter_features.insert(feature.clone());
                                        } else {
                                            self.split_filter_features.remove(feature);
                                        }
                                        self.split_active_preset = None;
                                    }
                                }
                                ui.separator();
                                if ui.button("Clear All").clicked() {
                                    self.split_filter_features.clear();
                                    self.split_active_preset = None;
                                }
                            });
                        });
                        if feature_count > 0 {
                            if ui.small_button("✕").on_hover_text("Clear feature filters").clicked() {
                                self.split_filter_features.clear();
                                self.split_active_preset = None;
                            }
                        }
                    });
                }

                // ID Prefix Filters dropdown (only if there are non-type prefixes)
                if !prefixes.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label("ID Prefixes:");
                        let prefix_count = self.split_filter_prefixes.len();
                        let prefix_label = if prefix_count == 0 {
                            "All Prefixes ▼".to_string()
                        } else {
                            format!("{} selected ▼", prefix_count)
                        };
                        let prefix_response = ui.button(&prefix_label);
                        let popup_id = ui.make_persistent_id("prefix_filter_popup_list2");
                        if prefix_response.clicked() {
                            ui.memory_mut(|mem| mem.toggle_popup(popup_id));
                        }
                        egui::popup_below_widget(ui, popup_id, &prefix_response, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                            ui.set_min_width(150.0);
                            egui::ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
                                for prefix in &prefixes {
                                    let mut checked = self.split_filter_prefixes.contains(prefix);
                                    if ui.checkbox(&mut checked, prefix).changed() {
                                        if checked {
                                            self.split_filter_prefixes.insert(prefix.clone());
                                        } else {
                                            self.split_filter_prefixes.remove(prefix);
                                        }
                                        self.split_active_preset = None;
                                    }
                                }
                                ui.separator();
                                if ui.button("Clear All").clicked() {
                                    self.split_filter_prefixes.clear();
                                    self.split_active_preset = None;
                                }
                            });
                        });
                        if prefix_count > 0 {
                            if ui.small_button("✕").on_hover_text("Clear prefix filters").clicked() {
                                self.split_filter_prefixes.clear();
                                self.split_active_preset = None;
                            }
                        }
                    });
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(5.0);

                // Action buttons
                ui.horizontal(|ui| {
                    if ui.button("Clear All Filters").clicked() {
                        self.split_filter_types.clear();
                        self.split_filter_features.clear();
                        self.split_filter_prefixes.clear();
                        self.split_filter_statuses.clear();
                        self.split_filter_priorities.clear();
                        self.split_active_preset = None;
                    }

                    if ui.button("💾 Save As...").clicked() {
                        self.show_split_save_preset_dialog = true;
                    }

                    if ui.button("Close").clicked() {
                        self.show_filter_dialog_list2 = false;
                    }
                });
            });

        if !open {
            self.show_filter_dialog_list2 = false;
        }
    }
}

impl eframe::App for RequirementsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Reset per-frame flags at the start of each frame
        self.quick_change_consumed_action = false;

        // Periodically check for other concurrent users (every ~60 frames = ~1 second)
        static CONCURRENT_CHECK_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let counter = CONCURRENT_CHECK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if counter % 60 == 0 {
            if let Ok(lock_info) = self.storage.get_active_sessions() {
                let other_sessions = lock_info.get_other_sessions(&self.session_id);
                if !other_sessions.is_empty() && !self.other_sessions_warning_shown {
                    // Show warning about other users
                    let user_names: Vec<&str> = other_sessions.iter()
                        .map(|s| s.user_name.as_str())
                        .collect();
                    self.toast_message = Some(ToastNotification {
                        message: format!("⚠️ {} also has this file open", user_names.join(", ")),
                        is_success: false,
                        show_until: Instant::now() + std::time::Duration::from_secs(10),
                    });
                    self.other_sessions_warning_shown = true;
                }
                self.last_lock_info = Some(lock_info);
            }
        }

        // Poll for background AI evaluation results
        if let Some(receiver) = &self.ai_eval_receiver {
            if let Ok(result) = receiver.try_recv() {
                eprintln!("DEBUG: Background AI evaluation completed for {}", result.spec_id);

                // Clear the in-progress state
                self.ai_eval_in_progress = None;
                self.ai_eval_receiver = None;

                match result.result {
                    Ok(response) => {
                        // Create the stored evaluation
                        let stored_eval = StoredAiEvaluation::new(response.clone(), result.content_hash);

                        // Find and update the requirement
                        if let Some(req) = self.store.requirements.iter_mut().find(|r| r.id == result.req_id) {
                            req.ai_evaluation = Some(stored_eval);
                            eprintln!("DEBUG: Updated AI evaluation for {}", result.spec_id);
                        }

                        // Save the store
                        self.save();

                        // Show success toast
                        self.toast_message = Some(ToastNotification {
                            message: format!("AI evaluation complete for {} (Score: {}/10)",
                                result.spec_id, response.quality_score),
                            is_success: true,
                            show_until: Instant::now() + std::time::Duration::from_secs(5),
                        });

                        // Also update the AI result panel
                        self.ai_last_result = Some(AiResult {
                            action_name: "Evaluate".to_string(),
                            success: true,
                            message: format!(
                                "Quality Score: {}/10\n\nStrengths:\n{}\n\nIssues:\n{}",
                                response.quality_score,
                                response.strengths.iter()
                                    .map(|s| format!("• {}", s))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                response.issues.iter()
                                    .map(|i| format!("• [{}] {}: {}", i.severity, i.issue_type, i.text))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            ),
                            details: response.suggested_improvements.as_ref().map(|imp| {
                                format!("Suggested improvement:\n{}\n\nRationale: {}",
                                    imp.description.as_deref().unwrap_or("(none)"),
                                    imp.rationale)
                            }),
                        });
                    }
                    Err(e) => {
                        // Show error toast
                        self.toast_message = Some(ToastNotification {
                            message: format!("AI evaluation failed for {}: {}", result.spec_id, e),
                            is_success: false,
                            show_until: Instant::now() + std::time::Duration::from_secs(5),
                        });
                    }
                }
            }
        }

        // Poll for background Find Duplicates results
        if let Some(receiver) = &self.find_duplicates_receiver {
            if let Ok(result) = receiver.try_recv() {
                eprintln!("DEBUG: Background Find Duplicates completed for {}", result.spec_id);

                // Clear the in-progress state
                self.find_duplicates_in_progress = None;
                self.find_duplicates_receiver = None;

                match result.result {
                    Ok(duplicates) => {
                        let msg = if duplicates.is_empty() {
                            "No potential duplicates found.".to_string()
                        } else {
                            duplicates.iter()
                                .map(|d| format!(
                                    "• {} ({}% similar): {}\n  Recommendation: {}",
                                    d.spec_id,
                                    (d.similarity * 100.0) as i32,
                                    d.reason,
                                    d.recommendation
                                ))
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        };

                        // Show success toast
                        let count = duplicates.len();
                        self.toast_message = Some(ToastNotification {
                            message: if count == 0 {
                                format!("No duplicates found for {}", result.spec_id)
                            } else {
                                format!("Found {} potential duplicate(s) for {}", count, result.spec_id)
                            },
                            is_success: true,
                            show_until: Instant::now() + std::time::Duration::from_secs(5),
                        });

                        // Update AI result panel
                        self.ai_last_result = Some(AiResult {
                            action_name: "Find Duplicates".to_string(),
                            success: true,
                            message: msg,
                            details: None,
                        });
                        self.show_ai_results_panel = true;
                    }
                    Err(e) => {
                        // Show error toast
                        self.toast_message = Some(ToastNotification {
                            message: format!("Find duplicates failed for {}: {}", result.spec_id, e),
                            is_success: false,
                            show_until: Instant::now() + std::time::Duration::from_secs(5),
                        });

                        // Update AI result panel with error
                        self.ai_last_result = Some(AiResult {
                            action_name: "Find Duplicates".to_string(),
                            success: false,
                            message: format!("AI error: {}", e),
                            details: None,
                        });
                        self.show_ai_results_panel = true;
                    }
                }
            }
        }

        // Update window title based on database name and title
        // Format: "Name - Title" or just "Title" or just "Name" or "Requirements Manager"
        let title = match (self.store.name.is_empty(), self.store.title.is_empty()) {
            (true, true) => "Requirements Manager".to_string(),
            (true, false) => self.store.title.clone(),
            (false, true) => self.store.name.clone(),
            (false, false) => format!("{} - {}", self.store.name, self.store.title),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // Apply the selected theme
        self.user_settings.theme.apply(ctx);

        // Apply current font size to the context with distinct heading sizes
        let mut style = (*ctx.style()).clone();
        let base = self.current_font_size;

        // Exponential scaling for markdown headings - makes differences much more visible
        let scale = 1.25_f32;
        let heading_sizes = [
            base * scale.powi(5), // H1 = ~3.05x
            base * scale.powi(4), // H2 = ~2.44x
            base * scale.powi(3), // H3 = ~1.95x
            base * scale.powi(2), // H4 = ~1.56x
            base * scale,         // H5 = 1.25x
            base,                 // H6 = base size
        ];

        // Get the UI heading size based on user preference (1-6 maps to index 0-5)
        let ui_heading_idx = (self.user_settings.ui_heading_level.clamp(1, 6) - 1) as usize;
        let ui_heading_size = heading_sizes[ui_heading_idx];

        // Update standard text styles
        for (text_style, font_id) in style.text_styles.iter_mut() {
            match text_style {
                egui::TextStyle::Small => font_id.size = base * 0.85,
                egui::TextStyle::Body => font_id.size = base,
                egui::TextStyle::Monospace => font_id.size = base,
                egui::TextStyle::Button => font_id.size = base,
                egui::TextStyle::Heading => font_id.size = ui_heading_size, // UI headings use user preference
                egui::TextStyle::Name(name) => {
                    // Set distinct sizes for markdown heading levels (exponential scaling)
                    let name_str: &str = name.as_ref();
                    match name_str {
                        "Heading" => font_id.size = heading_sizes[0],  // H1
                        "Heading2" => font_id.size = heading_sizes[1], // H2
                        "Heading3" => font_id.size = heading_sizes[2], // H3
                        "Heading4" => font_id.size = heading_sizes[3], // H4
                        "Heading5" => font_id.size = heading_sizes[4], // H5
                        "Heading6" => font_id.size = heading_sizes[5], // H6
                        _ => font_id.size = base,
                    }
                }
            }
        }
        ctx.set_style(style);

        // Determine current keybinding context based on view state
        // If a text field has focus, we don't want to trigger navigation keys
        // Also consider if we're in Add/Edit mode - navigation shouldn't work there
        let text_input_focused = ctx.wants_keyboard_input();
        let in_form_view = matches!(self.current_view, View::Add | View::Edit);
        let in_settings = self.show_settings_dialog;

        self.current_key_context = if text_input_focused || in_form_view || in_settings {
            // When typing in a text field, in form view, or in settings - only global shortcuts should work
            KeyContext::Form // Use Form context - this is NOT Global and NOT RequirementsList
        } else {
            match self.current_view {
                View::List => KeyContext::RequirementsList,
                View::Detail => KeyContext::DetailView,
                View::Add | View::Edit => KeyContext::Form, // This branch won't be reached due to in_form_view check
                View::OrgChart => KeyContext::RequirementsList, // Use same context for org chart
                View::KanBan => KeyContext::RequirementsList,   // Use same context for kanban
                View::Baselines => KeyContext::RequirementsList, // Use same context for baselines
                View::Timeline => KeyContext::RequirementsList, // Use same context for timeline
                View::Planning => KeyContext::RequirementsList, // Use same context for planning
            }
        };

        // Handle keyboard shortcuts for zoom (global context)
        let mut zoom_delta: f32 = 0.0;
        let mut zoom_reset = false;

        // Check for zoom keybindings
        if self.user_settings.keybindings.is_pressed(
            KeyAction::ZoomIn,
            ctx,
            self.current_key_context,
        ) {
            zoom_delta = 1.0;
        }
        if self.user_settings.keybindings.is_pressed(
            KeyAction::ZoomOut,
            ctx,
            self.current_key_context,
        ) {
            zoom_delta = -1.0;
        }
        if self.user_settings.keybindings.is_pressed(
            KeyAction::ZoomReset,
            ctx,
            self.current_key_context,
        ) {
            zoom_reset = true;
        }

        // Check for theme cycling keybinding (global context)
        if self.user_settings.keybindings.is_pressed(
            KeyAction::CycleTheme,
            ctx,
            self.current_key_context,
        ) {
            self.cycle_theme();
            self.user_settings.theme.apply(ctx);
            let _ = self.user_settings.save();
        }

        // Check for new requirement keybinding (Ctrl+N, global context)
        // Uses smart heuristic for parent selection
        if self.user_settings.keybindings.is_pressed(
            KeyAction::NewRequirement,
            ctx,
            self.current_key_context,
        ) && !self.show_settings_dialog
        {
            self.clear_form(); // Uses smart heuristic for parent
            self.pending_view_change = Some(View::Add);
        }

        // Search navigation keys - only active when there's an active search in highlight mode
        let has_active_search = !self.filter_text.is_empty()
            && self.user_settings.search_mode == SearchMode::Highlight;

        if has_active_search {
            // 'n' for next search match (takes priority over new sibling when search is active)
            if self.user_settings.keybindings.is_pressed(
                KeyAction::NextSearchMatch,
                ctx,
                self.current_key_context,
            ) {
                self.next_search_match();
            }
            // 'N' for previous search match (takes priority over new child when search is active)
            else if self.user_settings.keybindings.is_pressed(
                KeyAction::PrevSearchMatch,
                ctx,
                self.current_key_context,
            ) {
                self.prev_search_match();
            }
        } else {
            // When no search is active, 'n' creates new sibling requirement
            if self.user_settings.keybindings.is_pressed(
                KeyAction::NewSiblingRequirement,
                ctx,
                self.current_key_context,
            ) && !self.show_settings_dialog
            {
                self.clear_form_for_sibling();
                self.pending_view_change = Some(View::Add);
            }

            // 'N'/Shift+N creates new child requirement
            if self.user_settings.keybindings.is_pressed(
                KeyAction::NewChildRequirement,
                ctx,
                self.current_key_context,
            ) && !self.show_settings_dialog
            {
                self.clear_form_for_child();
                self.pending_view_change = Some(View::Add);
            }
        }

        // Escape to clear search (always active when there's search text)
        if !self.filter_text.is_empty()
            && self.user_settings.keybindings.is_pressed(
                KeyAction::ClearSearch,
                ctx,
                self.current_key_context,
            )
        {
            self.clear_search();
        }

        // '/' to switch to filter mode (vim-style)
        if self.user_settings.keybindings.is_pressed(
            KeyAction::SwitchToFilterMode,
            ctx,
            self.current_key_context,
        ) && !self.filter_text.is_empty()
        {
            self.user_settings.search_mode = SearchMode::Filter;
            let _ = self.user_settings.save();
        }

        // Also handle Ctrl+= as alternate zoom in (common on keyboards)
        ctx.input(|i| {
            let ctrl = i.modifiers.ctrl || i.modifiers.mac_cmd;
            let shift = i.modifiers.shift;
            if ctrl && shift && i.key_pressed(egui::Key::Equals) {
                zoom_delta = 1.0;
            }

            // Ctrl+MouseWheel to zoom - use raw scroll delta and check for events
            if ctrl {
                // Check for scroll events
                for event in &i.events {
                    if let egui::Event::MouseWheel { delta, .. } = event {
                        if delta.y > 0.0 {
                            zoom_delta = 1.0;
                        } else if delta.y < 0.0 {
                            zoom_delta = -1.0;
                        }
                    }
                }
                // Also check raw scroll delta as fallback
                if zoom_delta == 0.0 && i.raw_scroll_delta.y != 0.0 {
                    if i.raw_scroll_delta.y > 0.0 {
                        zoom_delta = 1.0;
                    } else {
                        zoom_delta = -1.0;
                    }
                }
            }
        });

        if zoom_reset {
            self.reset_zoom();
        }

        // Apply zoom after input closure
        if zoom_delta > 0.0 {
            self.zoom_in();
        } else if zoom_delta < 0.0 {
            self.zoom_out();
        }

        // Handle keyboard navigation in the requirements list
        // Context checking is now handled by the keybinding system
        {
            let mut nav_delta: i32 = 0;
            let mut jump_to_start = false;
            let mut jump_to_end = false;
            let page_size: i32 = 10; // Number of items to move for Page Up/Down

            // Check if we're in a context where list navigation should work
            let nav_context_active = matches!(
                self.current_key_context,
                KeyContext::RequirementsList | KeyContext::DetailView
            );

            // Check navigation keybindings (context-aware)
            // Block list navigation when status popup is open or delete confirm is pending
            // Also skip for Timeline and Planning views - they have their own navigation handling
            let can_navigate = self.quick_change_field.is_none() && self.pending_delete_confirm.is_none();
            let in_timeline = self.current_view == View::Timeline;
            let in_planning = self.current_view == View::Planning;
            if !in_timeline && !in_planning && can_navigate
                && (self.user_settings.keybindings.is_pressed(
                    KeyAction::NavigateDown,
                    ctx,
                    self.current_key_context,
                ) || self.user_settings.keybindings.is_pressed(
                    KeyAction::NavigateDownVim,
                    ctx,
                    self.current_key_context,
                ))
            {
                nav_delta = 1;
            } else if !in_timeline && !in_planning && can_navigate
                && (self.user_settings.keybindings.is_pressed(
                    KeyAction::NavigateUp,
                    ctx,
                    self.current_key_context,
                ) || self.user_settings.keybindings.is_pressed(
                    KeyAction::NavigateUpVim,
                    ctx,
                    self.current_key_context,
                ))
            {
                nav_delta = -1;
            }

            // Page Up/Down, Home/End, and Mouse Wheel (only when not in text input)
            // Skip for Timeline and Planning views - they have their own handling
            if nav_context_active && !in_timeline && !in_planning {
                ctx.input(|i| {
                    // Page Up/Down
                    if i.key_pressed(egui::Key::PageDown) {
                        nav_delta = page_size;
                    } else if i.key_pressed(egui::Key::PageUp) {
                        nav_delta = -page_size;
                    }

                    // Home/End
                    if i.key_pressed(egui::Key::Home) {
                        jump_to_start = true;
                    } else if i.key_pressed(egui::Key::End) {
                        jump_to_end = true;
                    }

                    // Mouse wheel scrolls the view without changing selection
                    // (Ctrl+wheel is handled separately for zoom)
                });
            }

            // Edit keybinding (context-aware)
            // Skip if quick change popup is open or just consumed an action (prevents Enter key pass-through)
            if self.quick_change_field.is_none()
                && !self.quick_change_consumed_action
                && self.user_settings.keybindings.is_pressed(
                    KeyAction::Edit,
                    ctx,
                    self.current_key_context,
                )
            {
                if let Some(idx) = self.selected_idx {
                    self.load_form_from_requirement(idx);
                    self.pending_view_change = Some(View::Edit);
                }
            }

            // Toggle expand/collapse in tree views (context-aware)
            if self.user_settings.keybindings.is_pressed(
                KeyAction::ToggleExpand,
                ctx,
                self.current_key_context,
            ) {
                if self.perspective != Perspective::Flat {
                    if let Some(idx) = self.selected_idx {
                        if let Some(req) = self.store.requirements.get(idx) {
                            let req_id = req.id;
                            let is_collapsed =
                                self.tree_collapsed.get(&req_id).copied().unwrap_or(false);
                            self.tree_collapsed.insert(req_id, !is_collapsed);
                        }
                    }
                }
            }

            // Save keybinding (context-aware - works in Form context)
            if self.user_settings.keybindings.is_pressed(
                KeyAction::Save,
                ctx,
                self.current_key_context,
            ) {
                self.pending_save = true;
            }

            // Quick change popups and other list-context hotkeys
            // (only in list/detail view, not in edit/add mode)
            let not_in_form = !matches!(self.current_view, View::Add | View::Edit);
            let no_popup_open = self.quick_change_field.is_none();
            let no_delete_confirm = self.pending_delete_confirm.is_none();

            if nav_context_active && no_popup_open && not_in_form && no_delete_confirm {
                // 's' key to open status popup
                if self.user_settings.keybindings.is_pressed(
                    KeyAction::OpenStatusPicker,
                    ctx,
                    self.current_key_context,
                ) {
                    if let Some(idx) = self.selected_idx {
                        if let Some(req) = self.store.requirements.get(idx) {
                            let statuses = [
                                RequirementStatus::Draft,
                                RequirementStatus::Approved,
                                RequirementStatus::Completed,
                                RequirementStatus::Rejected,
                            ];
                            let current_status_idx = statuses.iter().position(|s| *s == req.status).unwrap_or(0);
                            self.quick_change_selected = current_status_idx;
                            self.quick_change_target_id = Some(req.id);
                            self.quick_change_field = Some(QuickChangeField::Status);
                        }
                    }
                }
                // 'p' key to open priority popup
                else if self.user_settings.keybindings.is_pressed(
                    KeyAction::OpenPriorityPicker,
                    ctx,
                    self.current_key_context,
                ) {
                    if let Some(idx) = self.selected_idx {
                        if let Some(req) = self.store.requirements.get(idx) {
                            let priorities = [
                                RequirementPriority::High,
                                RequirementPriority::Medium,
                                RequirementPriority::Low,
                            ];
                            let current_priority_idx = priorities.iter().position(|p| *p == req.priority).unwrap_or(1);
                            self.quick_change_selected = current_priority_idx;
                            self.quick_change_target_id = Some(req.id);
                            self.quick_change_field = Some(QuickChangeField::Priority);
                        }
                    }
                }
                // 'o' key to open owner picker (fuzzy finder)
                else if self.user_settings.keybindings.is_pressed(
                    KeyAction::OpenOwnerPicker,
                    ctx,
                    self.current_key_context,
                ) {
                    if let Some(idx) = self.selected_idx {
                        if let Some(req) = self.store.requirements.get(idx) {
                            self.quick_change_owner_search.clear();
                            self.quick_change_selected = 0;
                            self.quick_change_target_id = Some(req.id);
                            self.quick_change_field = Some(QuickChangeField::Owner);
                        }
                    }
                }
                // 'S' (Shift+S) key to open sprint picker
                else if self.user_settings.keybindings.is_pressed(
                    KeyAction::OpenSprintPicker,
                    ctx,
                    self.current_key_context,
                ) {
                    if let Some(idx) = self.selected_idx {
                        if let Some(req) = self.store.requirements.get(idx) {
                            // Don't open sprint picker for Sprint type requirements
                            if req.req_type != RequirementType::Sprint {
                                self.quick_change_selected = 0;
                                self.quick_change_target_id = Some(req.id);
                                self.quick_change_field = Some(QuickChangeField::Sprint);
                            }
                        }
                    }
                }
                // 'f' key to open feature picker (fuzzy finder)
                else if self.user_settings.keybindings.is_pressed(
                    KeyAction::OpenFeaturePicker,
                    ctx,
                    self.current_key_context,
                ) {
                    eprintln!("DEBUG: 'f' handler triggered! selected_idx={:?}", self.selected_idx);
                    if let Some(idx) = self.selected_idx {
                        if let Some(req) = self.store.requirements.get(idx) {
                            eprintln!("DEBUG: Opening feature picker for req: {:?}", req.spec_id);
                            self.quick_change_feature_search.clear();
                            self.quick_change_selected = 0;
                            self.quick_change_target_id = Some(req.id);
                            self.quick_change_field = Some(QuickChangeField::Feature);
                        }
                    }
                }
                // 'c' key to add comment
                else if self.user_settings.keybindings.is_pressed(
                    KeyAction::AddComment,
                    ctx,
                    self.current_key_context,
                ) {
                    if self.selected_idx.is_some() {
                        self.show_add_comment = true;
                        self.comment_content.clear();
                        self.reply_to_comment = None;
                    }
                }
                // 'L' (Shift+L) to toggle links panel
                else if self.user_settings.keybindings.is_pressed(
                    KeyAction::ToggleLinksPanel,
                    ctx,
                    self.current_key_context,
                ) {
                    if self.selected_idx.is_some() {
                        // Toggle to Links tab in detail view
                        if self.active_tab == DetailTab::Links {
                            self.active_tab = DetailTab::Description;
                        } else {
                            self.active_tab = DetailTab::Links;
                        }
                    }
                }
                // 'd' key to delete with confirmation
                else if self.user_settings.keybindings.is_pressed(
                    KeyAction::DeleteWithConfirm,
                    ctx,
                    self.current_key_context,
                ) {
                    if let Some(idx) = self.selected_idx {
                        self.pending_delete_confirm = Some(idx);
                    }
                }
                // 'D' (Shift+D) to delete immediately
                else if self.user_settings.keybindings.is_pressed(
                    KeyAction::DeleteImmediate,
                    ctx,
                    self.current_key_context,
                ) {
                    if let Some(idx) = self.selected_idx {
                        self.delete_requirement(idx);
                    }
                }
                // 'a' key to archive/unarchive
                else if self.user_settings.keybindings.is_pressed(
                    KeyAction::ArchiveToggle,
                    ctx,
                    self.current_key_context,
                ) {
                    if let Some(idx) = self.selected_idx {
                        self.toggle_archive(idx);
                    }
                }
            }

            // Handle navigation (delta-based or jump-based) based on focused list
            // Get the appropriate filtered indices and current selection based on focus
            let (filtered_indices, current_selection) = match self.focused_list {
                FocusedList::List1 => (self.get_filtered_indices(), self.selected_idx),
                FocusedList::List2 => (self.get_split_filtered_indices(), self.split_selected_idx),
            };

            // Skip requirements list navigation when in Timeline or Planning view (they have their own navigation)
            if !in_timeline && !in_planning && !filtered_indices.is_empty() {
                let new_selection = if jump_to_start {
                    // Jump to first item
                    Some(filtered_indices[0])
                } else if jump_to_end {
                    // Jump to last item
                    Some(filtered_indices[filtered_indices.len() - 1])
                } else if nav_delta != 0 {
                    if let Some(current_idx) = current_selection {
                        // Find current position in filtered list
                        if let Some(pos) =
                            filtered_indices.iter().position(|&idx| idx == current_idx)
                        {
                            // Move up or down within bounds
                            let new_pos = (pos as i32 + nav_delta)
                                .max(0)
                                .min(filtered_indices.len() as i32 - 1)
                                as usize;
                            Some(filtered_indices[new_pos])
                        } else {
                            // Current selection not in filtered list, select first/last
                            if nav_delta > 0 {
                                Some(filtered_indices[0])
                            } else {
                                Some(filtered_indices[filtered_indices.len() - 1])
                            }
                        }
                    } else {
                        // Nothing selected, select first or last based on direction
                        if nav_delta > 0 {
                            Some(filtered_indices[0])
                        } else {
                            Some(filtered_indices[filtered_indices.len() - 1])
                        }
                    }
                } else {
                    None // No navigation action
                };

                if let Some(new_sel) = new_selection {
                    match self.focused_list {
                        FocusedList::List1 => {
                            if Some(new_sel) != self.selected_idx {
                                self.selected_idx = Some(new_sel);
                                self.pending_view_change = Some(View::Detail);
                                // Scroll the newly selected item into view (centered)
                                if let Some(req) = self.store.requirements.get(new_sel) {
                                    self.scroll_to_requirement = Some(req.id);
                                    // Sync List 2 when navigation is locked
                                    if self.navigation_locked {
                                        self.split_selected_idx = Some(new_sel);
                                        self.split_scroll_to_requirement = Some(req.id);
                                    }
                                }
                            }
                        }
                        FocusedList::List2 => {
                            if Some(new_sel) != self.split_selected_idx {
                                self.split_selected_idx = Some(new_sel);
                                // Scroll into view (centered)
                                if let Some(req) = self.store.requirements.get(new_sel) {
                                    self.split_scroll_to_requirement = Some(req.id);
                                    // Sync List 1 when navigation is locked
                                    if self.navigation_locked {
                                        self.selected_idx = Some(new_sel);
                                        self.scroll_to_requirement = Some(req.id);
                                        self.pending_view_change = Some(View::Detail);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Handle keyboard navigation in Timeline view
            if self.current_view == View::Timeline && nav_context_active {
                let filtered_events = self.get_filtered_timeline_events();
                let event_count = filtered_events.len();

                if event_count > 0 {
                    let mut timeline_nav_delta: i32 = 0;
                    let mut timeline_jump_start = false;
                    let mut timeline_jump_end = false;
                    let mut timeline_navigate_to_req = false;

                    // Check navigation keys
                    if can_navigate
                        && (self.user_settings.keybindings.is_pressed(
                            KeyAction::NavigateDown,
                            ctx,
                            self.current_key_context,
                        ) || self.user_settings.keybindings.is_pressed(
                            KeyAction::NavigateDownVim,
                            ctx,
                            self.current_key_context,
                        ))
                    {
                        timeline_nav_delta = 1;
                    } else if can_navigate
                        && (self.user_settings.keybindings.is_pressed(
                            KeyAction::NavigateUp,
                            ctx,
                            self.current_key_context,
                        ) || self.user_settings.keybindings.is_pressed(
                            KeyAction::NavigateUpVim,
                            ctx,
                            self.current_key_context,
                        ))
                    {
                        timeline_nav_delta = -1;
                    }

                    // Page Up/Down, Home/End
                    ctx.input(|i| {
                        if i.key_pressed(egui::Key::PageDown) {
                            timeline_nav_delta = 10;
                        } else if i.key_pressed(egui::Key::PageUp) {
                            timeline_nav_delta = -10;
                        }
                        if i.key_pressed(egui::Key::Home) {
                            timeline_jump_start = true;
                        } else if i.key_pressed(egui::Key::End) {
                            timeline_jump_end = true;
                        }
                        // Enter key to navigate to the selected event's requirement
                        if i.key_pressed(egui::Key::Enter) {
                            timeline_navigate_to_req = true;
                        }
                    });

                    // Apply navigation
                    let new_idx = if timeline_jump_start {
                        Some(0)
                    } else if timeline_jump_end {
                        Some(event_count - 1)
                    } else if timeline_nav_delta != 0 {
                        let current = self.timeline_selected_event_idx.unwrap_or(0) as i32;
                        let new_pos = (current + timeline_nav_delta)
                            .max(0)
                            .min(event_count as i32 - 1) as usize;
                        Some(new_pos)
                    } else {
                        None
                    };

                    if let Some(idx) = new_idx {
                        self.timeline_selected_event_idx = Some(idx);
                        // Suppress hover highlight when using keyboard navigation
                        self.timeline_suppress_hover = true;
                        self.timeline_suppress_hover_pos = ctx.input(|i| i.pointer.hover_pos());
                    }

                    // Handle Enter key to navigate to requirement
                    if timeline_navigate_to_req {
                        if let Some(event_idx) = self.timeline_selected_event_idx {
                            let filtered = self.get_filtered_timeline_events();
                            if let Some(event) = filtered.get(event_idx) {
                                if !event.req_id.is_nil() {
                                    if let Some(req_idx) = self.store.requirements.iter().position(|r| r.id == event.req_id) {
                                        self.selected_idx = Some(req_idx);
                                        self.pending_view_change = Some(View::List);
                                        self.scroll_to_center = true;
                                        if let Some(req) = self.store.requirements.get(req_idx) {
                                            self.scroll_to_requirement = Some(req.id);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Handle keyboard navigation in Planning view
            if self.current_view == View::Planning && nav_context_active {
                let planning_items = self.get_planning_items_list();
                let item_count = planning_items.len();

                if item_count > 0 {
                    let mut planning_nav_delta: i32 = 0;
                    let mut planning_jump_start = false;
                    let mut planning_jump_end = false;
                    let mut planning_navigate_to_detail = false;

                    // Check navigation keys
                    if can_navigate
                        && (self.user_settings.keybindings.is_pressed(
                            KeyAction::NavigateDown,
                            ctx,
                            self.current_key_context,
                        ) || self.user_settings.keybindings.is_pressed(
                            KeyAction::NavigateDownVim,
                            ctx,
                            self.current_key_context,
                        ))
                    {
                        planning_nav_delta = 1;
                    } else if can_navigate
                        && (self.user_settings.keybindings.is_pressed(
                            KeyAction::NavigateUp,
                            ctx,
                            self.current_key_context,
                        ) || self.user_settings.keybindings.is_pressed(
                            KeyAction::NavigateUpVim,
                            ctx,
                            self.current_key_context,
                        ))
                    {
                        planning_nav_delta = -1;
                    }

                    // Page Up/Down, Home/End
                    ctx.input(|i| {
                        if i.key_pressed(egui::Key::PageDown) {
                            planning_nav_delta = 10;
                        } else if i.key_pressed(egui::Key::PageUp) {
                            planning_nav_delta = -10;
                        }
                        if i.key_pressed(egui::Key::Home) {
                            planning_jump_start = true;
                        } else if i.key_pressed(egui::Key::End) {
                            planning_jump_end = true;
                        }
                        // Enter key to navigate to the selected item's detail view
                        if i.key_pressed(egui::Key::Enter) {
                            planning_navigate_to_detail = true;
                        }
                    });

                    // Find current position based on planning_selected_item
                    let current_pos = self.planning_selected_item
                        .and_then(|id| planning_items.iter().position(|&item_id| item_id == id));

                    // Apply navigation
                    let new_idx = if planning_jump_start {
                        Some(0)
                    } else if planning_jump_end {
                        Some(item_count - 1)
                    } else if planning_nav_delta != 0 {
                        let current = current_pos.unwrap_or(0) as i32;
                        let new_pos = (current + planning_nav_delta)
                            .max(0)
                            .min(item_count as i32 - 1) as usize;
                        Some(new_pos)
                    } else {
                        None
                    };

                    if let Some(idx) = new_idx {
                        if let Some(&item_id) = planning_items.get(idx) {
                            self.planning_selected_item = Some(item_id);
                            // Also update selected_idx for consistency
                            if let Some(req_idx) = self.store.requirements.iter().position(|r| r.id == item_id) {
                                self.selected_idx = Some(req_idx);
                            }
                        }
                    }

                    // Handle Enter key to navigate to detail view
                    if planning_navigate_to_detail {
                        if let Some(item_id) = self.planning_selected_item {
                            if let Some(req_idx) = self.store.requirements.iter().position(|r| r.id == item_id) {
                                self.selected_idx = Some(req_idx);
                                self.pending_view_change = Some(View::Detail);
                            }
                        }
                    }
                }
            }
        }

        // Handle pending operations (to avoid borrow checker issues)
        if let Some(idx) = self.pending_delete.take() {
            self.delete_requirement(idx);
        }
        if let Some(view) = self.pending_view_change.take() {
            // Track edit locks for collaborative awareness
            let was_editing = self.current_view == View::Edit;
            let will_edit = view == View::Edit;

            if will_edit && !was_editing {
                // Entering edit mode - check if someone else is editing and set edit lock
                if let Some(idx) = self.selected_idx {
                    if let Some(req) = self.store.requirements.get(idx) {
                        // Check if someone else is editing this requirement
                        if let Some(lock_info) = &self.last_lock_info {
                            let editors = lock_info.get_editors(req.id);
                            if !editors.is_empty() {
                                let editor_names: Vec<&str> = editors.iter()
                                    .filter(|s| s.session_id != self.session_id)
                                    .map(|s| s.user_name.as_str())
                                    .collect();
                                if !editor_names.is_empty() {
                                    self.toast_message = Some(ToastNotification {
                                        message: format!("⚠️ {} is currently editing this requirement!",
                                            editor_names.join(", ")),
                                        is_success: false,
                                        show_until: Instant::now() + std::time::Duration::from_secs(8),
                                    });
                                }
                            }
                        }

                        let edit_lock = EditLock {
                            requirement_id: req.id,
                            spec_id: req.spec_id.clone().unwrap_or_else(|| req.id.to_string()),
                            started_at: Utc::now(),
                        };
                        if let Some(sender) = &self.heartbeat_sender {
                            let _ = sender.send(HeartbeatCommand::SetEditLock(Some(edit_lock)));
                        }
                    }
                }
            } else if was_editing && !will_edit {
                // Exiting edit mode - clear edit lock
                if let Some(sender) = &self.heartbeat_sender {
                    let _ = sender.send(HeartbeatCommand::SetEditLock(None));
                }
            }

            self.current_view = view;
        }
        if let Some((author, content, parent_id)) = self.pending_comment_add.take() {
            if let Some(idx) = self.selected_idx {
                self.add_comment_to_requirement(idx, author, content, parent_id);
            }
        }
        if let Some(comment_id) = self.pending_comment_delete.take() {
            if let Some(idx) = self.selected_idx {
                self.delete_comment_from_requirement(idx, comment_id);
            }
        }
        if let Some((comment_id, reaction_name)) = self.pending_reaction_toggle.take() {
            if let Some(idx) = self.selected_idx {
                self.toggle_comment_reaction(idx, comment_id, &reaction_name);
            }
        }
        if let Some((source_idx, target_idx)) = self.pending_relationship.take() {
            self.create_relationship_from_drop(source_idx, target_idx);
        }
        if let Some((req_id, new_description)) = self.pending_apply_ai_description.take() {
            self.apply_ai_description(req_id, new_description);
        }

        // Handle drag release globally - check if primary button was just released
        let released = ctx.input(|i| i.pointer.primary_released());
        if released && self.drag_source.is_some() {
            if let (Some(source), Some(target)) = (self.drag_source, self.drop_target) {
                if source != target {
                    self.pending_relationship = Some((source, target));
                }
            }
            self.drag_source = None;
            self.drop_target = None;
        }

        // Handle Sprint Planning drag release globally
        if released && self.planning_drag_source.is_some() {
            if let Some(source_id) = self.planning_drag_source.take() {
                if let Some(target) = self.planning_drag_target_sprint.take() {
                    let username = self.user_settings.display_name();
                    // Get current sprint of the item
                    let current_sprint = self.store.get_requirement_sprint(&source_id)
                        .map(|r| r.id);

                    match target {
                        None => {
                            // Move to backlog
                            self.store.remove_from_sprint(source_id, &username);
                            self.save();
                        }
                        Some(sprint_id) => {
                            // Move to sprint (only if different from current)
                            if current_sprint != Some(sprint_id) {
                                self.store.assign_to_sprint(source_id, sprint_id, &username);
                                self.save();
                            }
                        }
                    }
                }
            }
            self.planning_drag_target_sprint = None;
        }

        // Clear drag state if mouse is not pressed (safety cleanup)
        ctx.input(|i| {
            if !i.pointer.any_down() && self.drag_source.is_some() {
                // Mouse released but we didn't catch it - clear state
            }
        });

        self.show_top_panel(ctx);

        // Determine layout based on current view and layout mode
        let screen_width = ctx.screen_rect().width();
        let screen_height = ctx.screen_rect().height();
        let min_width_for_side_panel = 900.0; // Minimum width to show side panel in edit mode
        let in_form_view = self.current_view == View::Add || self.current_view == View::Edit;

        if in_form_view {
            // In Add/Edit form view - layout depends on mode
            let show_left_panel = screen_width >= min_width_for_side_panel && !self.left_panel_collapsed;
            let is_edit = self.current_view == View::Edit;

            match self.layout_mode {
                LayoutMode::ListDetailsSide => {
                    // Use columns approach to match Detail View exactly (50/50 split)
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if show_left_panel {
                            ui.columns(2, |columns| {
                                // Left column (List) - matches Detail View list column
                                let col0_rect = columns[0].available_rect_before_wrap();
                                columns[0].set_clip_rect(col0_rect);
                                columns[0].vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.heading("Requirements");
                                    });
                                    // Search bar
                                    ui.horizontal(|ui| {
                                        self.show_search_bar(ui, 120.0, true);
                                        // Filter toggle button
                                        let filter_active = !self.filter_types.is_empty() || !self.filter_features.is_empty()
                                            || !self.filter_prefixes.is_empty() || !self.filter_statuses.is_empty()
                                            || !self.filter_priorities.is_empty();
                                        let filter_btn_text = if filter_active { "🔽 ●" } else { "🔽" };
                                        if ui.button(filter_btn_text).on_hover_text("Filters").clicked() {
                                            self.show_filter_dialog_list1 = !self.show_filter_dialog_list1;
                                        }
                                    });
                                    ui.separator();
                                    // Scrollable list
                                    egui::ScrollArea::vertical()
                                        .id_salt("list_side_scroll")
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            self.show_tree_list(ui);
                                        });
                                });

                                // Right column (Form)
                                columns[1].vertical(|ui| {
                                    self.show_form_vertical(ui, is_edit);
                                });
                            });
                        } else {
                            // No list panel - show button to restore and full-width form
                            ui.horizontal(|ui| {
                                if ui
                                    .button("◀ Show List")
                                    .on_hover_text("Show requirements list")
                                    .clicked()
                                {
                                    self.left_panel_collapsed = false;
                                }
                            });
                            ui.separator();
                            self.show_form_vertical(ui, is_edit);
                        }
                    });
                }
                LayoutMode::ListDetailsStacked => {
                    // Use TopBottomPanel approach to match Detail View exactly
                    // List on top, Form on bottom (vertically stacked)
                    if show_left_panel {
                        let list_height = (screen_height - 60.0) * 0.4;
                        egui::TopBottomPanel::top("list_top_panel")
                            .min_height(150.0)
                            .default_height(list_height)
                            .resizable(true)
                            .show(ctx, |ui| {
                                // Simplified list content matching Detail View
                                ui.horizontal(|ui| {
                                    ui.heading("Requirements");
                                });
                                // Search bar
                                ui.horizontal(|ui| {
                                    self.show_search_bar(ui, 120.0, true);
                                    // Filter toggle button
                                    let filter_active = !self.filter_types.is_empty() || !self.filter_features.is_empty()
                                        || !self.filter_prefixes.is_empty() || !self.filter_statuses.is_empty()
                                        || !self.filter_priorities.is_empty();
                                    let filter_btn_text = if filter_active { "🔽 ●" } else { "🔽" };
                                    if ui.button(filter_btn_text).on_hover_text("Filters").clicked() {
                                        self.show_filter_dialog_list1 = !self.show_filter_dialog_list1;
                                    }
                                });
                                ui.separator();
                                // Scrollable list
                                egui::ScrollArea::vertical()
                                    .id_salt("list_stacked_scroll")
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        self.show_tree_list(ui);
                                    });
                            });
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if !show_left_panel {
                            ui.horizontal(|ui| {
                                if ui
                                    .button("▲ Show List")
                                    .on_hover_text("Show requirements list")
                                    .clicked()
                                {
                                    self.left_panel_collapsed = false;
                                }
                            });
                            ui.separator();
                        }
                        self.show_form_stacked(ui, is_edit);
                    });
                }
                _ => {
                    // For other layout modes (SplitListDetails, SplitListOnly, ListOnly),
                    // use SidePanel approach
                    if show_left_panel {
                        self.show_list_panel_simple(ctx);
                    }

                    egui::CentralPanel::default().show(ctx, |ui| {
                        if !show_left_panel {
                            ui.horizontal(|ui| {
                                if ui
                                    .button("◀ Show List")
                                    .on_hover_text("Show requirements list")
                                    .clicked()
                                {
                                    self.left_panel_collapsed = false;
                                }
                            });
                            ui.separator();
                        }
                        self.show_form_vertical(ui, is_edit);
                    });
                }
            }
        } else if self.current_view == View::OrgChart {
            // Organization Chart view
            egui::CentralPanel::default().show(ctx, |ui| {
                self.show_org_chart_view(ui);
            });
        } else if self.current_view == View::KanBan {
            // KanBan board view
            egui::CentralPanel::default().show(ctx, |ui| {
                self.show_kanban_view(ui);
            });

            // KanBan detail modal window
            self.show_kanban_detail_modal(ctx);
        } else if self.current_view == View::Baselines {
            // Baselines management view
            egui::CentralPanel::default().show(ctx, |ui| {
                self.show_baselines_view(ui);
            });

            // Create baseline dialog
            self.show_create_baseline_dialog(ctx);
        } else if self.current_view == View::Timeline {
            // Timeline view showing requirement changes over time
            egui::CentralPanel::default().show(ctx, |ui| {
                self.show_timeline_view(ui);
            });
        } else if self.current_view == View::Planning {
            // Sprint planning view with planning on left, detail panel on right (consistent with List view)
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.columns(2, |columns| {
                    // Left column - Planning view (sprints and backlog)
                    columns[0].vertical(|ui| {
                        self.show_planning_view(ui);
                    });

                    // Right column - Detail panel (for selected item)
                    columns[1].vertical(|ui| {
                        self.show_planning_detail_panel(ui);
                    });
                });
            });
        } else {
            // In List/Detail view, use layout mode
            match self.layout_mode {
                LayoutMode::ListDetailsSide => {
                    // List on left, Details on right (side-by-side) - default
                    // Use columns approach to ensure proper 50/50 split
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.columns(2, |columns| {
                            // Left column (List)
                            let col0_rect = columns[0].available_rect_before_wrap();
                            columns[0].set_clip_rect(col0_rect);
                            columns[0].vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.heading("Requirements");
                                });
                                // Search bar
                                ui.horizontal(|ui| {
                                    self.show_search_bar(ui, 120.0, true);
                                    // Filter toggle button
                                    let filter_active = !self.filter_types.is_empty() || !self.filter_features.is_empty()
                                        || !self.filter_prefixes.is_empty() || !self.filter_statuses.is_empty()
                                        || !self.filter_priorities.is_empty();
                                    let filter_btn_text = if filter_active { "🔽 ●" } else { "🔽" };
                                    if ui.button(filter_btn_text).on_hover_text("Filters").clicked() {
                                        self.show_filter_dialog_list1 = !self.show_filter_dialog_list1;
                                    }
                                });
                                ui.separator();
                                // Scrollable list
                                egui::ScrollArea::vertical()
                                    .id_salt("list_side_scroll")
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        self.show_tree_list(ui);
                                    });
                            });

                            // Right column (Details)
                            columns[1].vertical(|ui| {
                                self.show_detail_view_with_close(ui);
                            });
                        });
                    });
                }
                LayoutMode::ListDetailsStacked => {
                    // List on top, Details on bottom (stacked)
                    let list_height = (screen_height - 60.0) * 0.4;
                    egui::TopBottomPanel::top("list_top_panel")
                        .min_height(150.0)
                        .default_height(list_height)
                        .resizable(true)
                        .show(ctx, |ui| {
                            self.show_list_content_with_close(ui, false);
                        });
                    egui::CentralPanel::default().show(ctx, |ui| {
                        self.show_detail_view_with_close(ui);
                    });
                }
                LayoutMode::SplitListDetails => {
                    // Two lists side-by-side on top, Details on bottom
                    let list_height = (screen_height - 60.0) * 0.4;
                    egui::TopBottomPanel::top("split_lists_top")
                        .min_height(150.0)
                        .default_height(list_height)
                        .resizable(true)
                        .show(ctx, |ui| {
                            // Two lists side by side with clipping
                            ui.columns(2, |columns| {
                                // Set clip rect for left column
                                let col0_rect = columns[0].available_rect_before_wrap();
                                columns[0].set_clip_rect(col0_rect);
                                self.show_list_content_in_column(&mut columns[0], false);

                                // Set clip rect for right column
                                let col1_rect = columns[1].available_rect_before_wrap();
                                columns[1].set_clip_rect(col1_rect);
                                self.show_split_list_content_in_column(&mut columns[1]);
                            });
                        });
                    egui::CentralPanel::default().show(ctx, |ui| {
                        self.show_detail_view_with_close(ui);
                    });
                }
                LayoutMode::SplitListOnly => {
                    // Two lists side-by-side, no details
                    // Use a Frame inside CentralPanel with explicit column layout
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.columns(2, |columns| {
                            // Left column (List 1)
                            let col0_rect = columns[0].available_rect_before_wrap();
                            columns[0].set_clip_rect(col0_rect);
                            columns[0].vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.heading("Requirements");
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button("✕").on_hover_text("Close this list").clicked() {
                                            self.layout_mode = self.layout_mode.without_split();
                                        }
                                    });
                                });
                                // Search bar for List 1
                                ui.horizontal(|ui| {
                                    self.show_search_bar(ui, 120.0, true);
                                    // Filter toggle button
                                    let filter_active = !self.filter_types.is_empty() || !self.filter_features.is_empty()
                                        || !self.filter_prefixes.is_empty() || !self.filter_statuses.is_empty()
                                        || !self.filter_priorities.is_empty();
                                    let filter_btn_text = if filter_active { "🔽 ●" } else { "🔽" };
                                    if ui.button(filter_btn_text).on_hover_text("Filters").clicked() {
                                        self.show_filter_dialog_list1 = !self.show_filter_dialog_list1;
                                    }
                                });
                                ui.separator();
                                // Scrollable list with clipping
                                egui::ScrollArea::vertical()
                                    .id_salt("list1_split_only_scroll")
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        self.show_tree_list(ui);
                                    });
                            });

                            // Right column (List 2)
                            let col1_rect = columns[1].available_rect_before_wrap();
                            columns[1].set_clip_rect(col1_rect);
                            columns[1].vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.heading("Requirements");
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button("✕").on_hover_text("Close this list").clicked() {
                                            self.layout_mode = self.layout_mode.without_split();
                                        }
                                    });
                                });
                                // Search bar for List 2
                                ui.horizontal(|ui| {
                                    ui.label("🔍");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.split_filter_text)
                                            .hint_text("Search...")
                                            .desired_width(120.0),
                                    );
                                    // Clear button
                                    if !self.split_filter_text.is_empty() {
                                        if ui.small_button("✕").on_hover_text("Clear search").clicked() {
                                            self.split_filter_text.clear();
                                        }
                                    }
                                    // Filter toggle button
                                    let filter_active = !self.split_filter_types.is_empty() || !self.split_filter_features.is_empty()
                                        || !self.split_filter_prefixes.is_empty() || !self.split_filter_statuses.is_empty()
                                        || !self.split_filter_priorities.is_empty();
                                    let filter_btn_text = if filter_active { "🔽 ●" } else { "🔽" };
                                    if ui.button(filter_btn_text).on_hover_text("Filters").clicked() {
                                        self.show_filter_dialog_list2 = !self.show_filter_dialog_list2;
                                    }
                                });
                                ui.separator();
                                // Scrollable list with clipping
                                egui::ScrollArea::vertical()
                                    .id_salt("list2_split_only_scroll")
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        self.show_split_list(ui);
                                    });
                            });
                        });
                    });
                }
                LayoutMode::ListOnly => {
                    // Single list, no details - full width
                    egui::CentralPanel::default().show(ctx, |ui| {
                        self.show_list_panel_content(ui);
                    });
                }
            }
        }

        // Show settings dialog (modal overlay)
        self.show_settings_dialog(ctx);

        // Show theme editor dialog
        self.show_theme_editor_dialog(ctx);

        // Show icon editor dialog
        self.show_icon_editor_dialog(ctx);

        // Show layout menu popup (triggered by long-press on layout button)
        self.show_layout_menu_popup(ctx);

        // Show quick change popup (triggered by 's' for status, 'p' for priority)
        self.show_quick_change_popup(ctx);

        // Show project dialogs
        self.show_switch_project_dialog(ctx);
        self.show_new_project_dialog(ctx);

        // Show scaffolding dialog (FR-0152)
        self.show_scaffold_dialog(ctx);

        // Show conflict resolution dialog (FR-0153)
        self.show_conflict_resolution_dialog(ctx);

        // Show migration confirmation dialog
        self.show_migration_confirmation_dialog(ctx);

        // Show rename prefix dialog
        self.show_rename_prefix_dialog(ctx);

        // Show save preset dialogs
        self.show_save_preset_dialog_window(ctx);
        self.show_split_save_preset_dialog_window(ctx);

        // Show delete preset confirmation dialog
        self.show_delete_preset_confirmation_dialog(ctx);

        // Show delete requirement confirmation dialog
        self.show_delete_requirement_confirmation_dialog(ctx);

        // Show clone requirement dialog
        self.show_clone_requirement_dialog(ctx);

        // Show filter dialogs
        self.show_filter_dialog_list1(ctx);
        self.show_filter_dialog_list2(ctx);

        // Show markdown help modal
        if self.show_markdown_help {
            self.show_markdown_help_modal(ctx);
        }

        // Show AI results modal
        if self.show_ai_results_panel {
            self.show_ai_results_modal(ctx);
        }

        // Show toast notification overlay (must be last to appear on top)
        self.show_toast_notification(ctx);
    }
}

/// Parse a hex color string (e.g., "#ff6b6b" or "ff6b6b") into an egui Color32
fn parse_hex_color(hex: &str) -> Option<egui::Color32> {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }

    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;

    Some(egui::Color32::from_rgb(r, g, b))
}
