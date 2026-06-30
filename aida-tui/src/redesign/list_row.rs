//! CLI-style columnar status palette for the redesign scope list — a PURE,
//! IO-free mirror of the CLI's status display so the bottom-panel rows render
//! like `aida list`: aligned columns (ID · Type · Status · Priority · Title)
//! with the matching status GLYPH and SEMANTIC COLOUR.
//!
//! ## Why this is a replica, not a shared call
//!
//! The authoritative palette lives in `aida-cli`:
//! `aida-cli/src/status_display.rs` (the documented "one palette of glyph +
//! colour" table near its module top) and `aida-cli/src/glyphs.rs` (the glyph
//! registry + unicode/ascii profile selector). `aida-tui` MUST NOT depend on
//! `aida-cli`, and the CLI paints with the `colored` crate while the TUI paints
//! with `ratatui::style::Color` — so the mapping is REPLICATED here as a small
//! pure table whose *appearance* matches the CLI's.
//!
//! FOLLOW-UP (not this task): the right long-term home for the
//! status→glyph/status→colour mapping is ONE shared place — `aida-core`, shared
//! by both the CLI and the TUI — so the two surfaces can never drift. Until
//! then this replica is the agreed cost; keep it byte-aligned with
//! `status_display.rs` when that palette changes. trace:TASK-914 | ai:claude
//!
//! ## The mirrored palette (from `status_display.rs`)
//!
//! | Status         | Colour (ratatui)     | Glyph (unicode / ascii) |
//! |----------------|----------------------|-------------------------|
//! | Draft          | dim                  | ◯ / ( )                 |
//! | Approved       | cyan (accent)        | ▸ / ->                  |
//! | Planned        | blue (info)          | ▷ / [>]                 |
//! | InProgress     | yellow (warn)        | ◐ / [~]                 |
//! | Done           | bright green, bold   | ◉ / [*]                 |
//! | Completed      | green                | ✓ / [x]                 |
//! | Rejected       | red (error)          | ✗ / [ ]                 |
//! | NeedsAttention | magenta, bold        | ⚠ / (!)                 |
//! | (unknown)      | plain fg             | · / .                   |
//!
//! Colour mapping note: the CLI uses the `colored` crate's named ANSI colours
//! (`cyan`, `blue`, `yellow`, …). The TUI honours the active [`Theme`] where a
//! role exists (Approved→`accent`, Planned/Completed→`info`, InProgress→`warn`,
//! Rejected→`error`, Draft→`dim`) and falls back to a fixed ratatui [`Color`]
//! for the roles a theme has no slot for (Done→bright green bold, NeedsAttention
//! →magenta bold). This keeps the redesign list legible under any built-in theme
//! while preserving the CLI's status→colour *semantics*. trace:TASK-914

use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};

/// The active glyph profile, mirroring the CLI's dominant precedence tier:
/// the `AIDA_GLYPHS` env var (`unicode` | `ascii`, case-insensitive). The CLI
/// also layers project/user `.aida/config.toml [ui] glyphs` under the env, but
/// the env tier is the per-terminal control most users reach for; the redesign
/// prototype honours that one and defaults to UNICODE (the CLI default).
/// trace:TASK-914 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GlyphMode {
    /// Unicode glyphs (the CLI default; ◯ ▸ ◐ ✓ …).
    #[default]
    Unicode,
    /// Curated ASCII fallback (( ) -> [~] [x] …).
    Ascii,
}

impl GlyphMode {
    /// Resolve the active mode from `AIDA_GLYPHS` (`ascii`/`plain` → ASCII,
    /// `unicode`/`emoji`/`utf8` → Unicode), defaulting to Unicode for an
    /// absent/unparseable value. Matches `glyphs::GlyphProfile::parse`'s tokens.
    pub fn from_env() -> GlyphMode {
        match std::env::var("AIDA_GLYPHS").ok().as_deref() {
            Some(raw) => Self::parse(raw).unwrap_or_default(),
            None => GlyphMode::default(),
        }
    }

    /// Parse a profile token; `None` for unknown so callers fall back to the
    /// default. Mirrors `aida-cli/src/glyphs.rs` `GlyphProfile::parse`.
    fn parse(raw: &str) -> Option<GlyphMode> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "unicode" | "emoji" | "utf8" | "utf-8" => Some(GlyphMode::Unicode),
            "ascii" | "plain" => Some(GlyphMode::Ascii),
            _ => None,
        }
    }
}

/// Collapse a status string to a bare match key — lowercase with whitespace,
/// `-` and `_` stripped — so "In Progress", "InProgress", "in-progress" and a
/// column-padded "Approved   " all resolve to the same arm. Mirrors
/// `status_display::normalize`. trace:TASK-914 | ai:claude
fn normalize(status: &str) -> String {
    status
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// The status glyph for the active [`GlyphMode`]. Mirrors
/// `status_display::status_glyph_literal` (unicode) + the curated ASCII
/// fallbacks from `glyphs.rs`. Unknown/custom statuses get the neutral bullet
/// so column layout stays stable. trace:TASK-914 | ai:claude
pub fn status_glyph(status: &str, mode: GlyphMode) -> &'static str {
    match (normalize(status).as_str(), mode) {
        ("draft", GlyphMode::Unicode) => "◯",
        ("draft", GlyphMode::Ascii) => "( )",
        ("approved", GlyphMode::Unicode) => "▸",
        ("approved", GlyphMode::Ascii) => "->",
        ("planned", GlyphMode::Unicode) => "▷",
        ("planned", GlyphMode::Ascii) => "[>]",
        ("inprogress", GlyphMode::Unicode) => "◐",
        ("inprogress", GlyphMode::Ascii) => "[~]",
        ("done", GlyphMode::Unicode) => "◉",
        ("done", GlyphMode::Ascii) => "[*]",
        ("completed", GlyphMode::Unicode) => "✓",
        ("completed", GlyphMode::Ascii) => "[x]",
        ("rejected", GlyphMode::Unicode) => "✗",
        ("rejected", GlyphMode::Ascii) => "[ ]",
        ("needsattention", GlyphMode::Unicode) => "⚠",
        ("needsattention", GlyphMode::Ascii) => "(!)",
        (_, GlyphMode::Unicode) => "·",
        (_, GlyphMode::Ascii) => ".",
    }
}

/// The work-liveness of a target row — is a live session/lease actively backing
/// this spec? Mirrors the CLI's `aida ps` / `aida status <spec>` per-spec
/// verdict (the `SpecLiveness` enum in `aida-cli/src/main.rs`), collapsed to the
/// three states the cockpit glyph needs:
///
/// | RowLiveness | `aida ps` source                                  | glyph |
/// |-------------|---------------------------------------------------|-------|
/// | `Live`      | a spec-scoped session/lease with a live process   | ●     |
/// | `Stale`     | flag set but no live session (dead lease OR a     | ⚠     |
/// |             | flag-only/orphaned In-Progress spec)              |       |
/// | `Idle`      | no session backing the spec (the default)         | ◦     |
///
/// ## Why this is a consumed signal, not a re-probe
///
/// The authoritative liveness machinery — the `/proc` process probe
/// (`process_probe::pid_is_alive` / `probe_live_claude_sessions`), the session
/// lease parse, and the `classify_spec_liveness` matrix — all live in
/// `aida-cli`, which `aida-tui` MUST NOT depend on. So the cockpit does NOT
/// reimplement the probe: it shells out to `aida ps --json` (the same binary,
/// via `current_exe()`) on a poll cadence and maps each row's spec id through
/// this enum. The probe runs in the CLI; the TUI only consumes its verdict.
/// See `liveness.rs` for the shell-out + the `should_probe` cache gate.
///
/// FOLLOW-UP (not this task): the long-term home for the liveness probe is a
/// shared `aida-core` helper both surfaces call, so the spec→liveness map can be
/// computed in-process without a subprocess. Until then the `aida ps --json`
/// shell-out is the agreed shared-logic seam.
// trace:TASK-978 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowLiveness {
    /// A spec-scoped session/lease is alive — work is actively backing this row.
    Live,
    /// The In-Progress flag is set but no live session backs it (a dead/dormant
    /// lease, or a flag-only/orphaned spec). The honest "is anything working
    /// it?" answer is no.
    Stale,
    /// No session is backing this spec — the idle default.
    Idle,
}

/// The liveness glyph for the active [`GlyphMode`]. Mirrors the CLI's `aida ps`
/// glyph conventions: ● live (the `LeaseState::Live` glyph), ⚠ stale (the
/// `Glyph::Warning` glyph), and a dim ◦ for idle. ASCII fallbacks stay a single
/// visible column so the leading liveness cell aligns across rows.
// trace:TASK-978 | ai:claude
pub fn liveness_glyph(state: RowLiveness, mode: GlyphMode) -> &'static str {
    match (state, mode) {
        (RowLiveness::Live, GlyphMode::Unicode) => "●",
        (RowLiveness::Live, GlyphMode::Ascii) => "*",
        (RowLiveness::Stale, GlyphMode::Unicode) => "⚠",
        // Mirrors `glyphs.rs` `Glyph::Warning` ASCII ("!").
        (RowLiveness::Stale, GlyphMode::Ascii) => "!",
        (RowLiveness::Idle, GlyphMode::Unicode) => "◦",
        // Idle reads as blank in ASCII — the column stays one space wide.
        (RowLiveness::Idle, GlyphMode::Ascii) => " ",
    }
}

/// The liveness [`Style`] for a glyph, mirroring `aida ps`'s colours: live →
/// green, stale → the theme's warn (yellow), idle → dim.
// trace:TASK-978 | ai:claude
pub fn liveness_style(state: RowLiveness, theme: &Theme) -> Style {
    match state {
        // Live → green, matching the CLI's `.green()` "live" label.
        RowLiveness::Live => Style::default().fg(Color::Green),
        // Stale → the theme's warn role (yellow), matching the CLI's `.yellow()`.
        RowLiveness::Stale => Style::default().fg(theme.warn),
        // Idle → dim, so a row with no live backing recedes.
        RowLiveness::Idle => Style::default().fg(theme.dim),
    }
}

/// The status [`Style`] (colour + emphasis) for a status, mirroring
/// `status_display::paint_status`'s semantics against the active [`Theme`].
///
/// Theme roles carry the CLI's named colours where one exists; the two roles a
/// theme has no slot for (Done = bright-green bold, NeedsAttention = magenta
/// bold) use fixed ratatui colours so the "finished on a branch" / "decide
/// something here" pops survive. Unknown statuses render plain `fg`.
/// trace:TASK-914 | ai:claude
pub fn status_style(status: &str, theme: &Theme) -> Style {
    let base = Style::default();
    match normalize(status).as_str() {
        // Draft → dim grey.
        "draft" => base.fg(theme.dim),
        // Approved → cyan (the theme's accent).
        "approved" => base.fg(theme.accent),
        // Planned → blue. Theme has no dedicated blue role; `info` is the
        // closest positive-cool role across the built-in palettes.
        "planned" => base.fg(theme.info),
        // InProgress → yellow (the theme's warn role).
        "inprogress" => base.fg(theme.warn),
        // Done → bold bright-green, so "finished on a branch" visibly pops
        // against plain-green Completed (matches status_display's STORY-86 note).
        "done" => base.fg(Color::LightGreen).add_modifier(Modifier::BOLD),
        // Completed → green (the theme's info/success role).
        "completed" => base.fg(theme.info),
        // Rejected → red (the theme's error role).
        "rejected" => base.fg(theme.error),
        // NeedsAttention → bold magenta — a colour no other status uses.
        "needsattention" => base.fg(Color::Magenta).add_modifier(Modifier::BOLD),
        // Unknown / custom status → plain foreground.
        _ => base.fg(theme.fg),
    }
}

/// The priority [`Style`]. The CLI's `aida list` prints `req.priority` PLAIN
/// (no colour call), so the faithful mirror renders priority in the theme's
/// foreground. Kept as its own function so a future CLI change that colours
/// priority can be matched in one place. trace:TASK-914 | ai:claude
pub fn priority_style(_priority: &str, theme: &Theme) -> Style {
    Style::default().fg(theme.fg)
}

/// A single row's pre-computed column cells (plain strings, no styling) laid
/// out to fixed widths so columns line up across rows — the CLI's aligned
/// output. The status/priority *colours* are applied at render time by the
/// caller (which builds styled spans from these cells); this struct is the
/// pure, testable layout. trace:TASK-914 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowCells {
    /// ID, left-padded to the id column width.
    pub id: String,
    /// Type, left-padded to the type column width.
    pub req_type: String,
    /// The status GLYPH (already mode-resolved) — one visible cell.
    pub status_glyph: String,
    /// The status LABEL, left-padded to the status-label width.
    pub status_label: String,
    /// Priority, left-padded to the priority column width.
    pub priority: String,
    /// Title, truncated to fit the remaining width (may be empty).
    pub title: String,
}

/// Column widths, mirroring `aida list`'s layout: ID 14, Type 12, status label
/// 11 (the glyph adds a 14th display column ahead of it in the CLI, here it's a
/// separate cell), Priority 10. The id width is computed from the visible set
/// (so short ids don't waste a fixed 14 cols) but capped so one pathological id
/// can't blow the layout. trace:TASK-914 | ai:claude
#[derive(Debug, Clone, Copy)]
pub struct ColumnWidths {
    pub id: usize,
    pub req_type: usize,
    pub status_label: usize,
    pub priority: usize,
}

impl ColumnWidths {
    /// The CLI-matching fixed widths for the non-id columns.
    const TYPE: usize = 12;
    const STATUS_LABEL: usize = 11;
    const PRIORITY: usize = 10;
    /// Lower bound on the id column so a set of short ids still reads as a
    /// column, and upper bound so one long id can't dominate. The CLI uses a
    /// fixed 14; we cap at 14 and floor at 6.
    const ID_MIN: usize = 6;
    const ID_MAX: usize = 14;

    /// Compute the column widths for a set of rows: the id column is the max id
    /// width in the set, clamped to `[ID_MIN, ID_MAX]`; the rest are the
    /// CLI-fixed widths. trace:TASK-914 | ai:claude
    pub fn for_rows<'a, I>(ids: I) -> ColumnWidths
    where
        I: IntoIterator<Item = &'a str>,
    {
        let max_id = ids
            .into_iter()
            .map(|s| s.chars().count())
            .max()
            .unwrap_or(0)
            .clamp(Self::ID_MIN, Self::ID_MAX);
        ColumnWidths {
            id: max_id,
            req_type: Self::TYPE,
            status_label: Self::STATUS_LABEL,
            priority: Self::PRIORITY,
        }
    }
}

/// Truncate `s` to at most `max` display chars, appending `…` when it had to
/// cut (so the truncation is visible). `max == 0` yields an empty string.
/// trace:TASK-914 | ai:claude
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The raw fields of one row, before layout. Grouping them keeps
/// [`layout_row`]'s signature small. trace:TASK-914 | ai:claude
#[derive(Debug, Clone, Copy)]
pub struct RowInput<'a> {
    pub id: &'a str,
    pub req_type: &'a str,
    pub status: &'a str,
    pub priority: &'a str,
    pub title: &'a str,
}

/// Lay out one row into aligned column cells given the shared [`ColumnWidths`],
/// the active [`GlyphMode`], and the width available for the title (the caller
/// derives `title_width` from the terminal width minus the fixed columns +
/// separators). PURE — no IO, no styling. trace:TASK-914 | ai:claude
pub fn layout_row(
    input: RowInput<'_>,
    widths: ColumnWidths,
    mode: GlyphMode,
    title_width: usize,
) -> RowCells {
    let status_label = format!("{:<width$}", input.status, width = widths.status_label);
    RowCells {
        id: format!("{:<width$}", input.id, width = widths.id),
        req_type: format!("{:<width$}", input.req_type, width = widths.req_type),
        status_glyph: status_glyph(input.status, mode).to_string(),
        status_label,
        priority: format!("{:<width$}", input.priority, width = widths.priority),
        title: truncate(input.title, title_width),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_for_each_canonical_status_unicode() {
        // Mirrors status_display::glyph_for_each_canonical_status.
        assert_eq!(status_glyph("Draft", GlyphMode::Unicode), "◯");
        assert_eq!(status_glyph("Approved", GlyphMode::Unicode), "▸");
        assert_eq!(status_glyph("Planned", GlyphMode::Unicode), "▷");
        assert_eq!(status_glyph("In Progress", GlyphMode::Unicode), "◐");
        assert_eq!(status_glyph("Done", GlyphMode::Unicode), "◉");
        assert_eq!(status_glyph("Completed", GlyphMode::Unicode), "✓");
        assert_eq!(status_glyph("Rejected", GlyphMode::Unicode), "✗");
        assert_eq!(status_glyph("Needs Attention", GlyphMode::Unicode), "⚠");
        assert_eq!(status_glyph("NeedsAttention", GlyphMode::Unicode), "⚠");
    }

    #[test]
    fn glyph_ascii_downgrades() {
        assert_eq!(status_glyph("Completed", GlyphMode::Ascii), "[x]");
        assert_eq!(status_glyph("Approved", GlyphMode::Ascii), "->");
        assert_eq!(status_glyph("Done", GlyphMode::Ascii), "[*]");
        assert_eq!(status_glyph("Draft", GlyphMode::Ascii), "( )");
    }

    #[test]
    fn glyph_normalizes_spacing_and_separators() {
        assert_eq!(status_glyph("InProgress", GlyphMode::Unicode), "◐");
        assert_eq!(status_glyph("in-progress", GlyphMode::Unicode), "◐");
        assert_eq!(status_glyph("in_progress", GlyphMode::Unicode), "◐");
        assert_eq!(status_glyph("Approved   ", GlyphMode::Unicode), "▸");
    }

    #[test]
    fn glyph_unknown_is_neutral_bullet() {
        assert_eq!(status_glyph("Frobnicate", GlyphMode::Unicode), "·");
        assert_eq!(status_glyph("", GlyphMode::Unicode), "·");
        assert_eq!(status_glyph("Frobnicate", GlyphMode::Ascii), ".");
    }

    #[test]
    fn status_style_maps_to_semantic_colours() {
        let t = Theme::default();
        // Draft → dim.
        assert_eq!(status_style("Draft", &t).fg, Some(t.dim));
        // Approved → accent (cyan-family).
        assert_eq!(status_style("Approved", &t).fg, Some(t.accent));
        // InProgress → warn (yellow).
        assert_eq!(status_style("In Progress", &t).fg, Some(t.warn));
        // Completed → info (green).
        assert_eq!(status_style("Completed", &t).fg, Some(t.info));
        // Rejected → error (red).
        assert_eq!(status_style("Rejected", &t).fg, Some(t.error));
    }

    #[test]
    fn status_style_done_and_needs_attention_are_bold_distinct() {
        let t = Theme::default();
        let done = status_style("Done", &t);
        assert_eq!(done.fg, Some(Color::LightGreen));
        assert!(done.add_modifier.contains(Modifier::BOLD));
        let na = status_style("Needs Attention", &t);
        assert_eq!(na.fg, Some(Color::Magenta));
        assert!(na.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn status_style_unknown_is_plain_fg() {
        let t = Theme::default();
        assert_eq!(status_style("Frobnicate", &t).fg, Some(t.fg));
    }

    #[test]
    fn priority_is_plain_fg_like_the_cli() {
        let t = Theme::default();
        // The CLI prints priority uncoloured; mirror that.
        assert_eq!(priority_style("High", &t).fg, Some(t.fg));
        assert_eq!(priority_style("", &t).fg, Some(t.fg));
    }

    #[test]
    fn id_column_width_is_max_id_width_clamped() {
        // Width = longest id in the set.
        let w = ColumnWidths::for_rows(["STORY-1", "TASK-1234", "BUG-9"]);
        assert_eq!(w.id, "TASK-1234".chars().count()); // 9
                                                       // Clamped up to the floor for a set of tiny ids.
        let small = ColumnWidths::for_rows(["A", "B"]);
        assert_eq!(small.id, ColumnWidths::ID_MIN);
        // Clamped down to the cap for a pathological id.
        let huge = ColumnWidths::for_rows(["SPEC-WITH-A-VERY-LONG-IDENTIFIER"]);
        assert_eq!(huge.id, ColumnWidths::ID_MAX);
        // Empty set → floor.
        let empty = ColumnWidths::for_rows(std::iter::empty::<&str>());
        assert_eq!(empty.id, ColumnWidths::ID_MIN);
        // Non-id columns are the CLI-fixed widths.
        assert_eq!(w.req_type, 12);
        assert_eq!(w.status_label, 11);
        assert_eq!(w.priority, 10);
    }

    #[test]
    fn truncate_appends_ellipsis_only_when_cut() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactlyten", 10), "exactlyten");
        assert_eq!(truncate("a longer title here", 8), "a longe…");
        assert_eq!(truncate("anything", 0), "");
        assert_eq!(truncate("anything", 1), "…");
    }

    #[test]
    fn rows_lay_out_into_aligned_columns() {
        // The headline alignment property: across a multi-row set, every row's
        // id/type/status-label/priority cells have identical visible width, so
        // the columns line up.
        let rows = [
            ("STORY-1", "Story", "Draft", "Medium", "first item"),
            ("TASK-42", "Task", "In Progress", "High", "second item here"),
            ("BUG-9", "Bug", "Completed", "Low", "third"),
        ];
        let widths = ColumnWidths::for_rows(rows.iter().map(|r| r.0));
        let laid: Vec<RowCells> = rows
            .iter()
            .map(|r| {
                layout_row(
                    RowInput {
                        id: r.0,
                        req_type: r.1,
                        status: r.2,
                        priority: r.3,
                        title: r.4,
                    },
                    widths,
                    GlyphMode::Unicode,
                    20,
                )
            })
            .collect();

        // id width = max("STORY-1"=7, "TASK-42"=7, "BUG-9"=5) = 7.
        let id_w = laid[0].id.chars().count();
        assert_eq!(id_w, 7);
        for row in &laid {
            assert_eq!(row.id.chars().count(), id_w, "id columns aligned");
            assert_eq!(row.req_type.chars().count(), 12, "type columns aligned");
            assert_eq!(
                row.status_label.chars().count(),
                11,
                "status-label columns aligned"
            );
            assert_eq!(row.priority.chars().count(), 10, "priority columns aligned");
        }
        // The status glyph rode along, mode-resolved.
        assert_eq!(laid[0].status_glyph, "◯"); // Draft
        assert_eq!(laid[1].status_glyph, "◐"); // In Progress
        assert_eq!(laid[2].status_glyph, "✓"); // Completed
                                               // Padded id keeps the original prefix.
        assert!(laid[2].id.starts_with("BUG-9"));
    }

    #[test]
    fn layout_row_truncates_title_to_title_width() {
        let widths = ColumnWidths::for_rows(["TASK-1"]);
        let row = layout_row(
            RowInput {
                id: "TASK-1",
                req_type: "Task",
                status: "Approved",
                priority: "High",
                title: "a very long title that will not fit",
            },
            widths,
            GlyphMode::Unicode,
            10,
        );
        assert_eq!(row.title.chars().count(), 10);
        assert!(row.title.ends_with('…'));
    }

    #[test]
    fn liveness_glyph_maps_each_state_unicode() {
        // live → ●, stale → ⚠, idle → ◦ (the TASK-978 mapping, mirroring
        // `aida ps`'s LeaseState::Live ● / Glyph::Warning ⚠ conventions).
        assert_eq!(liveness_glyph(RowLiveness::Live, GlyphMode::Unicode), "●");
        assert_eq!(liveness_glyph(RowLiveness::Stale, GlyphMode::Unicode), "⚠");
        assert_eq!(liveness_glyph(RowLiveness::Idle, GlyphMode::Unicode), "◦");
    }

    #[test]
    fn liveness_glyph_ascii_downgrades_stay_single_column() {
        assert_eq!(liveness_glyph(RowLiveness::Live, GlyphMode::Ascii), "*");
        assert_eq!(liveness_glyph(RowLiveness::Stale, GlyphMode::Ascii), "!");
        // Idle is blank in ASCII, but still one visible column wide.
        assert_eq!(liveness_glyph(RowLiveness::Idle, GlyphMode::Ascii), " ");
        for state in [RowLiveness::Live, RowLiveness::Stale, RowLiveness::Idle] {
            assert_eq!(
                liveness_glyph(state, GlyphMode::Ascii).chars().count(),
                1,
                "ASCII liveness glyph stays one column for alignment"
            );
        }
    }

    #[test]
    fn liveness_style_maps_to_semantic_colours() {
        let t = Theme::default();
        // Live → green (matches the CLI's `.green()` live label).
        assert_eq!(liveness_style(RowLiveness::Live, &t).fg, Some(Color::Green));
        // Stale → warn/yellow (matches the CLI's `.yellow()` stale label).
        assert_eq!(liveness_style(RowLiveness::Stale, &t).fg, Some(t.warn));
        // Idle → dim, so an unbacked row recedes.
        assert_eq!(liveness_style(RowLiveness::Idle, &t).fg, Some(t.dim));
    }

    #[test]
    fn glyph_mode_from_env_parses_tokens() {
        assert_eq!(GlyphMode::parse("ascii"), Some(GlyphMode::Ascii));
        assert_eq!(GlyphMode::parse("PLAIN"), Some(GlyphMode::Ascii));
        assert_eq!(GlyphMode::parse(" unicode "), Some(GlyphMode::Unicode));
        assert_eq!(GlyphMode::parse("emoji"), Some(GlyphMode::Unicode));
        assert_eq!(GlyphMode::parse("nonsense"), None);
    }
}
