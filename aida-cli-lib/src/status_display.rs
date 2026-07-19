//! Shared requirement-status display — one palette of glyph + colour used by
//! every CLI surface that prints a status (`aida show`, `aida list`,
//! `aida queue list`, `aida history`, the `aida status` overlay).
//!
//! Before TASK-269 each renderer carried its own `match` over status
//! strings, and they had drifted apart (`aida show` and `aida history` used
//! one palette, `aida queue list` another). This module is the single source
//! of truth so the eye learns one colour map.
//!
//! Palette (authored in the TASK-269 spec):
//!
//! | Status      | Colour              | Glyph |
//! |-------------|---------------------|-------|
//! | Draft       | dim grey            | ◯     |
//! | Approved    | cyan                | ▸     |
//! | Planned     | blue                | ▷     |
//! | InProgress  | yellow              | ◐     |
//! | Done        | bright green (bold) | ◉     |
//! | Completed   | green               | ✓     |
//! | Rejected    | red                 | ✗     |
//! | NeedsAttention | magenta (bold)   | ⚠     |
//!
//! `colored` auto-degrades to plain text under `NO_COLOR` / a non-TTY, so the
//! NO_COLOR acceptance criterion holds for free. Glyphs are plain Unicode and
//! always print, giving colourblind and copy-paste consumers the same signal.
//!
//! trace:TASK-269 | ai:claude

use colored::{ColoredString, Colorize};

/// Collapse a status string to a bare match key: lowercase, with whitespace,
/// `-` and `_` stripped. Lets "In Progress", "InProgress", "in-progress" and
/// even a column-padded "Approved   " all resolve to the same arm.
fn normalize(status: &str) -> String {
    status
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Profile-aware status glyph (proof site for the EPIC-45 glyph registry).
///
/// Maps a requirement status to a [`crate::glyphs::Glyph`] and renders it for
/// the supplied profile, so `[ui] glyphs = "ascii"` (or `AIDA_GLYPHS=ascii`)
/// downgrades these markers to ASCII. The default profile is Unicode, which
/// reproduces [`status_glyph`] byte-for-byte for the canonical statuses.
/// Unmapped / custom statuses still fall back to the neutral bullet.
//
// trace:STORY-628 | ai:claude
pub(crate) fn status_glyph_for_profile(
    status: &str,
    profile: crate::glyphs::GlyphProfile,
) -> &'static str {
    use crate::glyphs::Glyph;
    // trace:TASK-835 | ai:claude — Done + the neutral fallback now have registry
    // entries, so every canonical status routes through the profile.
    let glyph = match normalize(status).as_str() {
        "draft" => Glyph::Pending,
        "approved" => Glyph::Arrow,
        "planned" => Glyph::Queued,
        "inprogress" => Glyph::InFlight,
        "done" => Glyph::Done,
        "completed" => Glyph::Check,
        "rejected" => Glyph::Cross,
        "needsattention" => Glyph::Blocked,
        // Unmapped/custom statuses get the neutral bullet (also profile-aware).
        _ => Glyph::Neutral,
    };
    glyph.render(profile)
}

/// Glyph for a requirement status. Always safe to print — plain Unicode, no
/// ANSI — so it survives `NO_COLOR` and copy-paste. Unknown / project-specific
/// `custom_status` values get a neutral bullet rather than nothing, so the
/// badge layout stays stable.
///
/// Honors the EPIC-45 glyph profile (proof site): with `[ui] glyphs = "ascii"`
/// or `AIDA_GLYPHS=ascii` the canonical statuses downgrade to ASCII. The
/// default profile is Unicode, which reproduces the historical literals
/// byte-for-byte. trace:STORY-628 | ai:claude
pub(crate) fn status_glyph(status: &str) -> &'static str {
    let profile = crate::glyphs::active_profile(crate::find_project_root().ok().as_deref());
    if profile != crate::glyphs::GlyphProfile::Unicode {
        return status_glyph_for_profile(status, profile);
    }
    status_glyph_literal(status)
}

/// The historical literal status→glyph map. Kept as the Unicode source of
/// truth for [`status_glyph_for_profile`]'s unmapped fallback and for tests
/// that assert the default rendering. trace:STORY-628 | ai:claude
fn status_glyph_literal(status: &str) -> &'static str {
    match normalize(status).as_str() {
        "draft" => "◯",
        "approved" => "▸",
        "planned" => "▷",
        "inprogress" => "◐",
        "done" => "◉",
        "completed" => "✓",
        "rejected" => "✗",
        // STORY-332: a punted spec — paused mid-work, awaiting triage.
        "needsattention" => "⚠",
        _ => "·",
    }
}

/// Apply the status palette colour to `text`. `text` is usually the status
/// label itself, but may be a pre-padded table cell — callers in fixed-width
/// tables must pad the plain string *first*, then colour, since `{:<width}`
/// counts the (zero-visible-width) ANSI escapes as bytes otherwise.
///
/// `status` is the key that selects the colour; pass the same string as
/// `text` when colouring a bare label.
pub(crate) fn paint_status(text: &str, status: &str) -> ColoredString {
    match normalize(status).as_str() {
        "draft" => text.dimmed(),
        "approved" => text.cyan(),
        "planned" => text.blue(),
        "inprogress" => text.yellow(),
        // Done stays bold bright-green so "finished on a branch" visibly
        // pops against plain-green "merged to main" Completed.
        // trace:STORY-86 | ai:claude
        "done" => text.bright_green().bold(),
        "completed" => text.green(),
        "rejected" => text.red(),
        // STORY-332: bold magenta — a colour no other status uses, so a
        // punted spec visibly pops out of a list as "decide something here".
        "needsattention" => text.magenta().bold(),
        // History rows can carry a synthetic "(deleted)" status.
        "(deleted)" => text.red().dimmed(),
        _ => text.normal(),
    }
}

/// `"<glyph> <coloured status>"` — the badge for prominent single-status
/// displays: the `aida show` Status line and its bottom reprint, the spec
/// card one-liner, the `[…]` chips in `aida queue list` and the `aida status`
/// overlay. Fixed-width table columns should use [`paint_status`] on a padded
/// cell instead, since the glyph would break column alignment.
pub(crate) fn status_badge(status: &str) -> String {
    format!("{} {}", status_glyph(status), paint_status(status, status))
}

/// A fixed-width status cell for list tables: `"<glyph> <coloured label>"` with
/// the PLAIN label left-padded to `label_width` BEFORE colouring (ANSI escapes
/// would otherwise inflate `{:<}` byte counts and break column alignment). The
/// cell occupies `label_width + 2` visible columns — glyph (1) + space (1) +
/// label. Use this where `aida show`/badges aren't appropriate but a glyph in
/// the column is still wanted (TASK-315). trace:TASK-315 | ai:claude
pub(crate) fn status_cell(status: &str, label_width: usize) -> String {
    let padded = format!("{:<width$}", status, width = label_width);
    format!("{} {}", status_glyph(status), paint_status(&padded, status))
}

/// TASK-670: a fixed-width status cell with NO leading glyph — the padded,
/// coloured label only, occupying `width` visible columns. Used by `aida list
/// --no-glyph`, which strips every glyph (status + work-routing) for plain-text
/// / grep / non-Unicode output. (Colour still auto-degrades under NO_COLOR / a
/// non-TTY, so the plain-text path is honoured for free.) trace:TASK-670 | ai:claude
pub(crate) fn status_cell_no_glyph(status: &str, width: usize) -> String {
    let padded = format!("{:<width$}", status, width = width);
    paint_status(&padded, status).to_string()
}

/// TASK-670: the leading **work-routing** glyph for an `aida list` row. This
/// axis is ORTHOGONAL to status (Draft/NeedsAttention already carry glyphs via
/// [`status_glyph`], so they're deliberately NOT duplicated here) — it answers
/// "where is this spec in the work pipeline RIGHT NOW", not "what state is it
/// in".
///
/// Priority when several apply: in-flight `▶` > blocked `⊘` > queued `↑` >
/// idle (a single space, so the column stays aligned). Returns a `&'static str`
/// (not a `char`) for a uniform single-display-column cell.
///
/// - `▶` in-flight — a *live* session lease holds the spec (someone's on it
///   now; additive over the persistent `in-progress` status, which lingers
///   after a dead session). The caller supplies liveness — only live leases
///   should set `in_flight` (cf. the dead-agent reaper, STORY-496).
/// - `⊘` blocked — BlockedBy an incomplete spec (needs a graph walk, so the
///   caller only sets this behind `--blocked`).
/// - `↑` queued — present in a role queue, not yet started.
///
/// trace:TASK-670 | ai:claude
///
/// Profile-aware (TASK-835): the three routing markers route through the glyph
/// registry so `[ui] glyphs = "ascii"` / `AIDA_GLYPHS=ascii` / a custom
/// `[glyphs]` override applies here too. The default Unicode profile reproduces
/// the historical literals (▶ / ⊘ / ↑) byte-for-byte; idle stays a bare space.
/// trace:TASK-835 | ai:claude
pub(crate) fn flow_glyph(in_flight: bool, blocked: bool, queued: bool) -> &'static str {
    use crate::glyphs::Glyph;
    let profile = crate::glyphs::active_profile(crate::find_project_root().ok().as_deref());
    if in_flight {
        Glyph::FlowActive.render(profile)
    } else if blocked {
        Glyph::FlowBlocked.render(profile)
    } else if queued {
        Glyph::FlowQueued.render(profile)
    } else {
        " "
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_glyph_unicode_matches_literal_ascii_downgrades() {
        use crate::glyphs::GlyphProfile;
        // Unicode profile reproduces the literal byte-for-byte for mapped statuses.
        assert_eq!(
            status_glyph_for_profile("Completed", GlyphProfile::Unicode),
            "✓"
        );
        assert_eq!(
            status_glyph_for_profile("Approved", GlyphProfile::Unicode),
            "▸"
        );
        // ASCII profile downgrades.
        assert_eq!(
            status_glyph_for_profile("Completed", GlyphProfile::Ascii),
            "[x]"
        );
        assert_eq!(
            status_glyph_for_profile("Approved", GlyphProfile::Ascii),
            "->"
        );
        // Done now routes through the registry (TASK-835): Unicode reproduces
        // the historical ◉ byte-for-byte; ASCII downgrades.
        assert_eq!(status_glyph_for_profile("Done", GlyphProfile::Unicode), "◉");
        assert_eq!(status_glyph_for_profile("Done", GlyphProfile::Ascii), "[*]");
        // An unmapped/custom status renders the neutral bullet, profile-aware.
        assert_eq!(
            status_glyph_for_profile("Frobnicate", GlyphProfile::Unicode),
            "·"
        );
        assert_eq!(
            status_glyph_for_profile("Frobnicate", GlyphProfile::Ascii),
            "."
        );
    }

    #[test]
    fn glyph_for_each_canonical_status() {
        assert_eq!(status_glyph_literal("Draft"), "◯");
        assert_eq!(status_glyph_literal("Approved"), "▸");
        assert_eq!(status_glyph_literal("Planned"), "▷");
        assert_eq!(status_glyph_literal("In Progress"), "◐");
        assert_eq!(status_glyph_literal("Done"), "◉");
        assert_eq!(status_glyph_literal("Completed"), "✓");
        assert_eq!(status_glyph_literal("Rejected"), "✗");
        // trace:STORY-332 | ai:claude
        assert_eq!(status_glyph_literal("Needs Attention"), "⚠");
        assert_eq!(status_glyph_literal("NeedsAttention"), "⚠");
    }

    #[test]
    fn paint_status_needs_attention_is_magenta() {
        // STORY-332: punted specs paint magenta — verify the arm resolves
        // (and isn't swallowed by the unknown-status `_` fallback). Assert on
        // the selected style instead of ANSI bytes: colored intentionally
        // renders plain text on some Windows test runners.
        let painted = paint_status("Needs Attention", "Needs Attention");
        assert_eq!(painted.fgcolor, Some(colored::Color::Magenta));
        assert!(painted.style.contains(colored::Styles::Bold));
    }

    #[test]
    fn glyph_normalizes_spacing() {
        // The InProgress label reaches this code in several spellings, and
        // table callers hand in a column-padded cell.
        assert_eq!(status_glyph_literal("In Progress"), "◐");
        assert_eq!(status_glyph_literal("InProgress"), "◐");
        assert_eq!(status_glyph_literal("in-progress"), "◐");
        assert_eq!(status_glyph_literal("in_progress"), "◐");
        assert_eq!(status_glyph_literal("Approved   "), "▸");
    }

    #[test]
    fn glyph_unknown_status_is_neutral_bullet() {
        // Project-specific custom_status values must not fall through to
        // nothing — a neutral bullet keeps the badge layout stable.
        assert_eq!(status_glyph_literal("Blocked"), "·");
        assert_eq!(status_glyph_literal(""), "·");
    }

    #[test]
    fn badge_contains_glyph_and_label() {
        let badge = status_badge("Done");
        assert!(badge.contains('◉'), "badge missing glyph: {badge:?}");
        assert!(badge.contains("Done"), "badge missing label: {badge:?}");
    }

    /// TASK-315: a list status cell is `"<glyph> <label>"` padded so its visible
    /// width is `label_width + 2`. Asserted under NO_COLOR for an exact compare.
    #[test]
    fn status_cell_is_glyph_space_padded_label() {
        colored::control::set_override(false);
        let cell = status_cell("Approved", 11);
        colored::control::unset_override();
        // glyph + space + "Approved" padded to 11 = "Approved   ".
        assert_eq!(cell, "▸ Approved   ", "cell: {cell:?}");
        assert_eq!(cell.chars().count(), 13, "visible width = 11 + 2");
        // Over-long labels are not truncated (alignment degrades gracefully).
        colored::control::set_override(false);
        let wide = status_cell("In Progress", 11);
        colored::control::unset_override();
        assert_eq!(wide, "◐ In Progress", "cell: {wide:?}");
    }

    /// TASK-670: the work-routing glyph obeys the in-flight > blocked > queued
    /// priority, and falls back to a single space (column stays aligned) when
    /// nothing applies. trace:TASK-670 | ai:claude
    #[test]
    fn flow_glyph_priority_and_idle() {
        // Idle: no routing state.
        assert_eq!(flow_glyph(false, false, false), " ");
        // Each state alone.
        assert_eq!(flow_glyph(false, false, true), "↑", "queued");
        assert_eq!(flow_glyph(false, true, false), "⊘", "blocked");
        assert_eq!(flow_glyph(true, false, false), "▶", "in-flight");
        // Priority: in-flight wins over everything.
        assert_eq!(flow_glyph(true, true, true), "▶");
        assert_eq!(flow_glyph(true, false, true), "▶");
        // Blocked beats queued.
        assert_eq!(flow_glyph(false, true, true), "⊘");
        // Glyph is always a single display column.
        for g in [
            flow_glyph(false, false, false),
            flow_glyph(false, false, true),
            flow_glyph(false, true, false),
            flow_glyph(true, false, false),
        ] {
            assert_eq!(g.chars().count(), 1, "flow glyph must be one column: {g:?}");
        }
    }

    #[test]
    fn paint_status_plain_under_no_color() {
        // colored is a process-global; force colour off and confirm
        // paint_status emits no ANSI escapes (the NO_COLOR criterion).
        colored::control::set_override(false);
        let painted = paint_status("Approved", "Approved").to_string();
        colored::control::unset_override();
        assert_eq!(painted, "Approved", "expected no escape codes: {painted:?}");
    }
}
