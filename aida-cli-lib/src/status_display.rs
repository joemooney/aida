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
//! | Draft       | dim grey            | ○     |
//! | Approved    | cyan                | ▸     |
//! | Planned     | blue                | ▷     |
//! | InProgress  | yellow              | ◐     |
//! | Done        | bright green (bold) | ◉     |
//! | Completed   | green               | ✓     |
//! | Rejected    | red                 | ✗     |
//! | NeedsAttention | magenta (bold)   | ⚠     |
//! | Accepted    | green               | ☑     |
//! | Superseded  | green (dimmed)      | ⊡     |
//!
//! `Superseded` (TASK-1176) is terminal-but-ADOPTED: the spec was followed and
//! then replaced by a successor. It sits in the closed-green family (dimmed,
//! because a newer spec governs) and deliberately never wears the red ✗ that
//! means DECLINED.
//!
//! `Accepted` is a DISPLAY-only label (BUG-781): a decision spec's stored
//! `Approved` is that class's terminal state, so it renders as `Accepted` via
//! [`display_status_for_type`] rather than wearing the cyan "cleared to start"
//! badge a task wears.
//!
//! `colored` auto-degrades to plain text under `NO_COLOR` / a non-TTY, so the
//! NO_COLOR acceptance criterion holds for free. Glyphs are plain Unicode and
//! always print, giving colourblind and copy-paste consumers the same signal.
//!
//! trace:TASK-269 | ai:claude

use colored::{ColoredString, Colorize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NeedsAttentionLens {
    Shelved { cause: Option<String> },
    NeedsDecision { reason: Option<String> },
}

impl NeedsAttentionLens {
    pub(crate) fn label(&self) -> String {
        match self {
            NeedsAttentionLens::Shelved { cause } => label_with_reason("Shelved", cause.as_deref()),
            NeedsAttentionLens::NeedsDecision { reason } => {
                label_with_reason("Needs Decision", reason.as_deref())
            }
        }
    }

    pub(crate) fn palette_key(&self) -> &'static str {
        match self {
            NeedsAttentionLens::Shelved { .. } => "Shelved",
            NeedsAttentionLens::NeedsDecision { .. } => "NeedsDecision",
        }
    }
}

fn label_with_reason(label: &str, reason: Option<&str>) -> String {
    match reason.map(str::trim).filter(|s| !s.is_empty()) {
        Some(reason) => format!("{label} ({reason})"),
        None => label.to_string(),
    }
}

// trace:STORY-1023 | ai:codex
pub(crate) fn needs_attention_lens(
    req: &aida_core::models::Requirement,
) -> Option<NeedsAttentionLens> {
    if !matches!(req.status, aida_core::RequirementStatus::NeedsAttention) {
        return None;
    }
    if let Some(fr) = req.failure_reason.as_ref() {
        return Some(NeedsAttentionLens::Shelved {
            cause: Some(fr.kind.clone()),
        });
    }
    Some(NeedsAttentionLens::NeedsDecision {
        reason: req
            .attention_reason
            .as_ref()
            .map(|a| a.category.to_string()),
    })
}

/// The non-stored `--status` lens tokens, paired with the
/// [`NeedsAttentionLens::palette_key`] each one selects.
///
/// `shelved` and `needs-decision` name a DISPLAY lens over the stored
/// `NeedsAttention` status (STORY-1023), not statuses of their own, so no cache
/// column can hold them. This table is the single source of truth for which
/// tokens `aida list --status` accepts beyond the stored set, and the
/// "Unknown status filter" refusal enumerates it rather than restating it — a
/// hand-written list is exactly how the accepted set and the refusal drifted
/// apart until BUG-1771.
// trace:BUG-1771 | ai:claude
pub(crate) const LENS_FILTER_TOKENS: &[(&str, &str)] =
    &[("shelved", "Shelved"), ("needs-decision", "NeedsDecision")];

/// Resolve one `--status` token to the lens palette key it selects, or `None`
/// when the token names something other than a lens.
// trace:BUG-1771 | ai:claude
pub(crate) fn lens_filter_key(token: &str) -> Option<&'static str> {
    let normalized = normalize(token);
    LENS_FILTER_TOKENS
        .iter()
        .find(|(tok, _)| normalize(tok) == normalized)
        .map(|(_, key)| *key)
}

/// The lens tokens, comma-joined for a help/refusal line.
// trace:BUG-1771 | ai:claude
pub(crate) fn lens_filter_token_list() -> String {
    LENS_FILTER_TOKENS
        .iter()
        .map(|(tok, _)| *tok)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A VIEW axis a non-stored `--status` token opens, as distinct from a lens
/// over a stored status.
///
/// `deferred` is the one axis today: deferral is STORY-584's view-flag on the
/// requirement (parallel to `archived`), orthogonal to status, so there is no
/// stored status to widen a query to the way a parked lens does. The token is
/// therefore a spelling of `aida list --deferred`, and it is accepted on
/// `--status` because an orchestrator reaching for "which work is deferred?"
/// reaches for the status filter first — and got "Unknown status filter
/// 'deferred'" until BUG-1687.
// trace:BUG-1687 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewFilterAxis {
    Deferred,
}

/// The non-stored `--status` tokens that open a view axis.
///
/// Read by [`split_status_filter_spec`], by the legacy `list_requirements`
/// path, and by the "Unknown status filter" refusal — the same single-table
/// discipline BUG-1771 established for the parked lenses, so the accepted set
/// and the refusal that enumerates it cannot drift apart.
// trace:BUG-1687 | ai:claude
pub(crate) const VIEW_FILTER_TOKENS: &[(&str, ViewFilterAxis)] =
    &[("deferred", ViewFilterAxis::Deferred)];

/// Resolve one `--status` token to the view axis it opens, or `None` when the
/// token names something other than a view axis.
// trace:BUG-1687 | ai:claude
pub(crate) fn view_filter_axis(token: &str) -> Option<ViewFilterAxis> {
    let normalized = normalize(token);
    VIEW_FILTER_TOKENS
        .iter()
        .find(|(tok, _)| normalize(tok) == normalized)
        .map(|(_, axis)| *axis)
}

/// The view-axis tokens, comma-joined for a help/refusal line.
// trace:BUG-1687 | ai:claude
pub(crate) fn view_filter_token_list() -> String {
    VIEW_FILTER_TOKENS
        .iter()
        .map(|(tok, _)| *tok)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a `--status` filter spec asked for, once its non-stored tokens are
/// separated from the stored-status ones.
// trace:BUG-1687 | ai:claude
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StatusFilterSplit {
    /// [`NeedsAttentionLens::palette_key`] values the spec named.
    pub(crate) lens_keys: Vec<&'static str>,
    /// View axes the spec named.
    pub(crate) view_axes: Vec<ViewFilterAxis>,
    /// The stored-status spec left over, or `None` when the caller named
    /// nothing but non-stored tokens.
    pub(crate) residual: Option<String>,
}

/// Split a `--status` filter spec into the lens keys it names, the view axes it
/// opens, and the residual stored-status spec.
///
/// The cache query filters on stored statuses only, so a lens token has to be
/// widened to `needs-attention` for the query and the returned rows narrowed
/// again by lens; a view token instead sets its own filter axis and leaves the
/// status query alone. `residual` is `None` when the spec named non-stored
/// tokens only. Empty tokens are dropped and surrounding whitespace trimmed,
/// matching `RequirementStatus::expand_filter_spec` so the two halves of a
/// mixed spec agree on what counts as a token.
// trace:BUG-1771 | ai:claude
// trace:BUG-1687 | ai:claude — view axes joined the lens keys here.
pub(crate) fn split_status_filter_spec(spec: &str) -> StatusFilterSplit {
    let mut split = StatusFilterSplit::default();
    let mut stored: Vec<&str> = Vec::new();
    for raw in spec.split(',') {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(key) = lens_filter_key(token) {
            if !split.lens_keys.contains(&key) {
                split.lens_keys.push(key);
            }
            continue;
        }
        if let Some(axis) = view_filter_axis(token) {
            if !split.view_axes.contains(&axis) {
                split.view_axes.push(axis);
            }
            continue;
        }
        stored.push(token);
    }
    split.residual = if stored.is_empty() {
        None
    } else {
        Some(stored.join(","))
    };
    split
}

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
        "needsdecision" => Glyph::Blocked,
        "shelved" => Glyph::Pause,
        // trace:BUG-1687 | ai:claude — primed work waiting on a trigger, which
        // is the hourglass, NOT the ⏸ a mechanically shelved spec wears.
        "deferred" => Glyph::Hourglass,
        // trace:BUG-781 | ai:claude — the decision-class terminal label.
        "accepted" => Glyph::Accepted,
        // trace:TASK-1176 | ai:claude — adopted, then replaced.
        "superseded" => Glyph::Superseded,
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
        // trace:BUG-1018 | ai:codex
        "draft" => "○",
        "approved" => "▸",
        "planned" => "▷",
        "inprogress" => "◐",
        "done" => "◉",
        "completed" => "✓",
        "rejected" => "✗",
        // STORY-332: a punted spec — paused mid-work, awaiting triage.
        "needsattention" => "⚠",
        "needsdecision" => "⚠",
        // STORY-1023: mechanically parked with a typed recovery path.
        "shelved" => "⏸",
        // BUG-1687: deferred is PROSPECTIVE — primed, with a recorded revisit
        // trigger — so it wears the hourglass, not the shelf’s pause.
        "deferred" => "⏳",
        // trace:BUG-781 | ai:claude — a ratified decision: checked and closed.
        "accepted" => "☑",
        // trace:TASK-1176 | ai:claude — adopted, then replaced: the same box
        // family as `accepted`, with the check swapped for a dot. NOT the ✗ a
        // DECLINED spec wears.
        "superseded" => "⊡",
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
        "needsdecision" => text.magenta().bold(),
        // STORY-1023: a typed mechanical shelf should stay visible without
        // looking like an operator escalation.
        "shelved" => text.blue(),
        // BUG-1687: deferred work is parked BY CHOICE with a revisit trigger,
        // so it reads quieter than any lens that wants a human now — dimmed
        // blue, in the parked family but visibly not an escalation.
        // trace:BUG-1687 | ai:claude
        "deferred" => text.blue().dimmed(),
        // BUG-781: a ratified decision is terminal, so it paints in the closed
        // green family — never the cyan `Approved` wears on a task that has yet
        // to be started. trace:BUG-781 | ai:claude
        "accepted" => text.green(),
        // TASK-1176: superseded is terminal-but-ADOPTED — it paints in the
        // closed GREEN family (the spec was followed) but dimmed, because a
        // successor now governs. Never red: red means DECLINED.
        // trace:TASK-1176 | ai:claude
        "superseded" => text.green().dimmed(),
        // History rows can carry a synthetic "(deleted)" status.
        "(deleted)" => text.red().dimmed(),
        _ => text.normal(),
    }
}

/// The label a status should DISPLAY as for a given requirement type.
///
/// Identity for every type but one: a `decision` spec (an ADR) records
/// acceptance as `Approved`, which for that class is the TERMINAL state — the
/// same badge a task wears BEFORE anyone starts it. Rendering the stored word
/// verbatim made every ratified ADR read as pending work ("when will these
/// complete?"), so the decision class displays `Accepted` instead. Nothing is
/// rewritten on disk: this is a display-layer relabel, and `Approved` remains
/// the stored status (with `accepted` already an accepted input alias).
///
/// The returned label doubles as the palette key — pass it to
/// [`paint_status`] / [`status_cell`] / [`status_badge`] and the accepted arm
/// (green, ☑) selects itself.
// trace:BUG-781 | ai:claude
pub(crate) fn display_status_for_type<'a>(req_type: &str, status: &'a str) -> &'a str {
    if aida_core::lifecycle::is_accepted_decision(req_type, status) {
        "Accepted"
    } else {
        status
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

/// The palette key / display label a DEFERRED spec wears.
///
/// Doubles as the key for [`paint_status`] / [`status_glyph`], the way
/// [`display_status_for_type`]'s `"Accepted"` does.
// trace:BUG-1687 | ai:claude
pub(crate) const DEFERRED_LABEL: &str = "Deferred";

/// The agent-surface (`aida show` under AGENT-MODE, `--fields status`) token
/// for a deferred spec.
// trace:BUG-1687 | ai:claude
pub(crate) const DEFERRED_TOKEN: &str = "deferred";

/// Is this spec on the deferred shelf?
///
/// Mirrors the cache's own defer predicate (`DEFER_TAG_LIKE` in
/// `aida-core/src/db/cache.rs`): the STORY-584 flag OR a legacy `deferred:*`
/// parking tag. The two have to agree or BUG-1687 AC4 fails by construction —
/// `aida show` would call a spec deferred that `aida list --deferred` does not
/// return, or the reverse.
// trace:BUG-1687 | ai:claude
pub(crate) fn is_deferred(req: &aida_core::models::Requirement) -> bool {
    req.deferred || req.tags.iter().any(|t| t.starts_with("deferred:"))
}

/// The revisit trigger to show beside the Deferred label: the explicit
/// `deferred_until` field first, else the suffix of the first legacy
/// `deferred:*` tag — the same derivation `print_deferred_triggers` uses for
/// the `aida list --deferred` footer, so the two surfaces quote one trigger.
// trace:BUG-1687 | ai:claude
pub(crate) fn deferred_revisit_trigger(req: &aida_core::models::Requirement) -> Option<String> {
    if let Some(cond) = req.deferred_until.as_deref() {
        let cond = cond.trim();
        if !cond.is_empty() {
            return Some(cond.to_string());
        }
    }
    // `tags` is a HashSet, so "the first `deferred:*` tag" has no stable
    // meaning — take the lexicographic minimum instead, or two runs of the same
    // command could quote different triggers for the same spec.
    req.tags
        .iter()
        .filter_map(|t| t.strip_prefix("deferred:"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .min()
        .map(str::to_string)
}

/// `"Deferred (<revisit trigger>)"` for a deferred spec, `None` otherwise.
///
/// Deferral is a VIEW-FLAG orthogonal to status (STORY-584), so nothing is
/// rewritten on disk and the stored status survives an un-defer untouched —
/// this is the same display-layer relabel [`display_status_for_type`] performs
/// for a ratified decision (BUG-781). Until BUG-1687 the stored status showed
/// through verbatim, so a spec deferred out of `needs-attention` still read as
/// "someone act now" and misled two agent sessions in one afternoon.
///
/// Deliberately NOT a [`NeedsAttentionLens`] variant: a spec can be deferred at
/// ANY stored status, while that enum is the lens over the stored
/// `NeedsAttention` one, and BUG-1771 made its variants select `--status`
/// filters. Folding deferral in there would make `--status shelved` silently
/// lose a shelved-AND-deferred row.
// trace:BUG-1687 | ai:claude
pub(crate) fn deferred_display_label(req: &aida_core::models::Requirement) -> Option<String> {
    if !is_deferred(req) {
        return None;
    }
    if let Some(reason) = req
        .deferred_reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        let trigger = deferred_revisit_trigger(req)
            .map(|trigger| format!("; revisit: {trigger}"))
            .unwrap_or_default();
        return Some(format!("Held ({reason}{trigger})"));
    }
    Some(label_with_reason(
        DEFERRED_LABEL,
        deferred_revisit_trigger(req).as_deref(),
    ))
}

/// `"<glyph> <coloured Deferred (trigger)>"` for a deferred spec, `None`
/// otherwise — the override every prominent single-status display applies
/// before falling back to the stored badge.
// trace:BUG-1687 | ai:claude
pub(crate) fn deferred_badge(req: &aida_core::models::Requirement) -> Option<String> {
    deferred_display_label(req).map(|label| {
        format!(
            "{} {}",
            status_glyph(DEFERRED_LABEL),
            paint_status(&label, DEFERRED_LABEL)
        )
    })
}

/// The badge for one spec's already-resolved display status, with the deferral
/// override applied. Use this rather than [`status_badge`] wherever the
/// requirement itself is in hand: a deferred spec must not advertise the status
/// it was deferred out of.
// trace:BUG-1687 | ai:claude
pub(crate) fn spec_status_badge(
    req: &aida_core::models::Requirement,
    display_status: &str,
) -> String {
    deferred_badge(req).unwrap_or_else(|| status_badge(display_status))
}

pub(crate) fn parked_status_badge(req: &aida_core::models::Requirement) -> String {
    // BUG-1687: deferral outranks the parked lens. A spec punted and THEN
    // deferred carries both an `attention_reason` and the defer flag, and the
    // honest answer to "what state is this in?" is the later of the two
    // decisions — nobody is being asked to decide it today.
    // trace:BUG-1687 | ai:claude
    if let Some(badge) = deferred_badge(req) {
        return badge;
    }
    match needs_attention_lens(req) {
        Some(lens) => {
            let label = lens.label();
            let key = lens.palette_key();
            format!("{} {}", status_glyph(key), paint_status(&label, key))
        }
        None => status_badge(&req.status.to_string()),
    }
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
/// Priority when several apply: in-flight `▶` > blocked `⊘` > queued-supervised
/// `⇈` > queued `↑` > idle (a single space, so the column stays aligned).
/// Returns a `&'static str` (not a `char`) for a uniform cell.
///
/// - `▶` in-flight — a *live* session lease holds the spec (someone's on it
///   now; additive over the persistent `in-progress` status, which lingers
///   after a dead session). The caller supplies liveness — only live leases
///   should set `in_flight` (cf. the dead-agent reaper, STORY-496).
/// - `⊘` blocked — BlockedBy an incomplete spec (needs a graph walk, so the
///   caller only sets this behind `--blocked`).
/// - `↑` queued — present in a role queue, not yet started.
/// - `⇈` queued-supervised — queued but execution_mode guided/operator/decide,
///   so a headless drain skips it. Display-only; pickup behavior is unchanged.
///
/// trace:TASK-670 | ai:claude
///
/// Profile-aware (TASK-835): the three routing markers route through the glyph
/// registry so `[ui] glyphs = "ascii"` / `AIDA_GLYPHS=ascii` / a custom
/// `[glyphs]` override applies here too. The default Unicode profile reproduces
/// the historical literals (▶ / ⊘ / ↑) byte-for-byte; idle stays a bare space.
/// trace:TASK-835 | ai:claude
// trace:TASK-1234 | ai:codex
pub(crate) fn flow_glyph(
    in_flight: bool,
    blocked: bool,
    queued: bool,
    supervised: bool,
) -> &'static str {
    use crate::glyphs::Glyph;
    let profile = crate::glyphs::active_profile(crate::find_project_root().ok().as_deref());
    if in_flight {
        Glyph::FlowActive.render(profile)
    } else if blocked {
        Glyph::FlowBlocked.render(profile)
    } else if queued && supervised {
        Glyph::FlowQueuedSupervised.render(profile)
    } else if queued {
        Glyph::FlowQueued.render(profile)
    } else {
        " "
    }
}

/// The derived-status disclosure for a per-spec `show` status line, or `None`
/// when the displayed value needs no annotation.
///
/// An epic's displayed status is the read-only rollup of its children
/// (BUG-626), so it can disagree with the STORED field: nine epics stood in the
/// advisor's close bucket while `aida show` said `status: completed` with
/// nothing marking the value as derived, and callers concluded the close
/// actions were already taken. When the displayed and stored values resolve to
/// different lifecycle states, return the annotation naming both; when they
/// agree (every non-epic, and an epic whose rollup matches its stored field) or
/// either is a custom status outside the lifecycle, return `None` so the line
/// renders exactly as before.
// trace:BUG-1767 | ai:claude
pub(crate) fn derived_status_annotation(
    effective_status: &str,
    stored_status: &str,
) -> Option<String> {
    use aida_core::lifecycle::State;
    let eff = State::from_status_str(effective_status)?;
    let stored = State::from_status_str(stored_status)?;
    if eff == stored {
        return None;
    }
    Some(format!(
        "derived from child rollup; stored: {}",
        crate::help_next::state_token(stored)
    ))
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
        assert_eq!(
            status_glyph_for_profile("Draft", GlyphProfile::Unicode),
            "○"
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
        assert_eq!(
            status_glyph_for_profile("Draft", GlyphProfile::Ascii),
            "( )"
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
        assert_eq!(status_glyph_literal("Draft"), "○");
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
    fn needs_attention_lens_splits_shelved_from_decision() {
        let mut shelved =
            aida_core::models::Requirement::new("stale base".to_string(), String::new());
        shelved.status = aida_core::RequirementStatus::NeedsAttention;
        shelved.failure_reason = Some(aida_core::FailureReason {
            phase: "preflight".to_string(),
            phase_index: 1,
            kind: "stale-base".to_string(),
            detail: "branch is behind main".to_string(),
            recovery_hint: Some("rebase and retry".to_string()),
            shelved_by: Some("codex".to_string()),
            shelved_at: chrono::Utc::now(),
        });
        assert_eq!(
            needs_attention_lens(&shelved).map(|lens| lens.label()),
            Some("Shelved (stale-base)".to_string())
        );

        let mut decision =
            aida_core::models::Requirement::new("design fork".to_string(), String::new());
        decision.status = aida_core::RequirementStatus::NeedsAttention;
        decision.attention_reason = Some(aida_core::AttentionReason {
            category: aida_core::PuntCategory::DesignFork,
            detail: "pick a migration path".to_string(),
            lean: None,
            raised_by: Some("codex".to_string()),
            raised_at: chrono::Utc::now(),
        });
        assert_eq!(
            needs_attention_lens(&decision).map(|lens| lens.label()),
            Some("Needs Decision (design-fork)".to_string())
        );
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

    /// TASK-670/TASK-1234: the work-routing glyph obeys the in-flight > blocked
    /// > queued-supervised > queued priority, and falls back to a single space
    /// (column stays aligned) when nothing applies.
    #[test]
    fn flow_glyph_priority_and_idle() {
        // Idle: no routing state.
        assert_eq!(flow_glyph(false, false, false, false), " ");
        // Each state alone.
        assert_eq!(flow_glyph(false, false, true, false), "↑", "queued");
        assert_eq!(
            flow_glyph(false, false, true, true),
            "⇈",
            "queued supervised"
        );
        assert_eq!(flow_glyph(false, true, false, false), "⊘", "blocked");
        assert_eq!(flow_glyph(true, false, false, false), "▶", "in-flight");
        // Priority: in-flight wins over everything.
        assert_eq!(flow_glyph(true, true, true, true), "▶");
        assert_eq!(flow_glyph(true, false, true, true), "▶");
        // Blocked beats queued/supervised queued.
        assert_eq!(flow_glyph(false, true, true, true), "⊘");
        for g in [
            flow_glyph(false, false, false, false),
            flow_glyph(false, false, true, false),
            flow_glyph(false, true, false, false),
            flow_glyph(true, false, false, false),
        ] {
            assert_eq!(g.chars().count(), 1, "flow glyph must be one column: {g:?}");
        }
    }

    #[test]
    fn flow_glyph_profile_covers_supervised_queue() {
        use crate::glyphs::{Glyph, GlyphProfile};
        assert_eq!(
            Glyph::FlowQueuedSupervised.render(GlyphProfile::Unicode),
            "⇈"
        );
        assert_eq!(
            Glyph::FlowQueuedSupervised.render(GlyphProfile::Ascii),
            "^^"
        );
    }

    // BUG-781: an accepted decision relabels to the terminal `Accepted`, and
    // the relabelled string drives the palette — checked glyph, closed-green
    // colour — so a ratified ADR can never be mistaken for a task that is
    // merely cleared to start. Every other (type, status) pair is untouched.
    // trace:BUG-781 | ai:claude
    #[test]
    fn accepted_decision_relabels_and_repaints() {
        // The relabel itself, tolerant of the spellings the cache hands in.
        assert_eq!(display_status_for_type("Decision", "Approved"), "Accepted");
        assert_eq!(display_status_for_type("decision", "approved"), "Accepted");
        // A proposed ADR is still open — untouched.
        assert_eq!(display_status_for_type("Decision", "Draft"), "Draft");
        // A work spec at Approved keeps the "cleared to start" reading.
        for t in ["Task", "Story", "Bug", "Epic", "Functional"] {
            assert_eq!(
                display_status_for_type(t, "Approved"),
                "Approved",
                "{t} @ Approved must not relabel"
            );
        }

        // The relabelled string selects the terminal palette.
        assert_eq!(status_glyph_literal("Accepted"), "☑");
        let painted = paint_status("Accepted", "Accepted");
        assert_eq!(painted.fgcolor, Some(colored::Color::Green));
        // ...and it is visibly NOT the cyan an un-started Approved wears.
        let approved = paint_status("Approved", "Approved");
        assert_eq!(approved.fgcolor, Some(colored::Color::Cyan));

        // The badge a decision-aware caller renders carries both signals.
        colored::control::set_override(false);
        let badge = status_badge(display_status_for_type("Decision", "Approved"));
        colored::control::unset_override();
        assert_eq!(badge, "☑ Accepted", "badge: {badge:?}");
    }

    // BUG-781: the accepted glyph honours the EPIC-45 profile like every other
    // status marker — ASCII terminals get a curated fallback, not a mojibake
    // box. trace:BUG-781 | ai:claude
    #[test]
    fn accepted_glyph_is_profile_aware() {
        use crate::glyphs::GlyphProfile;
        assert_eq!(
            status_glyph_for_profile("Accepted", GlyphProfile::Unicode),
            "☑"
        );
        assert_eq!(
            status_glyph_for_profile("Accepted", GlyphProfile::Ascii),
            "[+]"
        );
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

    /// Every token in the shared table splits out as a lens, with no residual
    /// left for `expand_filter_spec` to reject. This is the anti-drift guard
    /// AC3 asks for: adding a row to `LENS_FILTER_TOKENS` without wiring it up
    /// fails here rather than shipping a token the CLI denies exists.
    // trace:BUG-1771 | ai:claude
    #[test]
    fn every_lens_token_splits_out_as_a_lens() {
        for (token, key) in LENS_FILTER_TOKENS {
            let split = split_status_filter_spec(token);
            assert_eq!(
                split.lens_keys,
                vec![*key],
                "token {token} must select key {key}"
            );
            assert!(
                split.view_axes.is_empty(),
                "token {token} is a lens, not a view axis"
            );
            assert_eq!(split.residual, None, "token {token} must leave no residual");
        }
    }

    /// The view-axis twin of the guard above: every row of
    /// `VIEW_FILTER_TOKENS` splits out as its axis and leaves no residual, so
    /// adding a token without wiring it up fails here instead of shipping a
    /// token the CLI denies exists.
    // trace:BUG-1687 | ai:claude
    #[test]
    fn every_view_token_splits_out_as_a_view_axis() {
        for (token, axis) in VIEW_FILTER_TOKENS {
            let split = split_status_filter_spec(token);
            assert_eq!(
                split.view_axes,
                vec![*axis],
                "token {token} must open axis {axis:?}"
            );
            assert!(
                split.lens_keys.is_empty(),
                "token {token} is a view axis, not a parked lens"
            );
            assert_eq!(split.residual, None, "token {token} must leave no residual");
        }
    }

    /// The tokens the refusal prints are exactly the tokens the splitter takes.
    // trace:BUG-1771 | ai:claude
    // trace:BUG-1687 | ai:claude — now covers the view-axis table too.
    #[test]
    fn refusal_token_list_matches_the_accepted_set() {
        let listed = lens_filter_token_list();
        for (token, _) in LENS_FILTER_TOKENS {
            assert!(
                listed.contains(token),
                "refusal list {listed:?} omits the accepted token {token}"
            );
        }
        assert_eq!(listed.split(", ").count(), LENS_FILTER_TOKENS.len());

        let views = view_filter_token_list();
        for (token, _) in VIEW_FILTER_TOKENS {
            assert!(
                views.contains(token),
                "refusal list {views:?} omits the accepted token {token}"
            );
        }
        assert_eq!(views.split(", ").count(), VIEW_FILTER_TOKENS.len());
    }

    /// Spelling is normalized the way every other status token is, and a
    /// non-lens token is handed back untouched for the stored expansion.
    // trace:BUG-1771 | ai:claude
    #[test]
    fn split_normalizes_spelling_and_preserves_stored_tokens() {
        for spelling in [
            "needs-decision",
            "needs_decision",
            "NeedsDecision",
            " Needs Decision ",
        ] {
            let split = split_status_filter_spec(spelling);
            assert_eq!(
                (split.lens_keys, split.view_axes, split.residual),
                (vec!["NeedsDecision"], Vec::new(), None),
                "{spelling} must resolve to the needs-decision lens"
            );
        }
        let split = split_status_filter_spec("shelved,approved,shelved,,draft");
        assert_eq!(
            (split.lens_keys, split.view_axes, split.residual),
            (
                vec!["Shelved"],
                Vec::new(),
                Some("approved,draft".to_string())
            ),
            "a repeated lens dedups, empty tokens drop, stored tokens survive in order"
        );
        let split = split_status_filter_spec("draft,approved");
        assert_eq!(
            (split.lens_keys, split.view_axes, split.residual),
            (Vec::new(), Vec::new(), Some("draft,approved".to_string())),
            "a spec with no lens token must pass through unchanged"
        );
    }

    /// A mixed spec keeps all three channels separate: the view axis does not
    /// eat the lens token or the stored one.
    // trace:BUG-1687 | ai:claude
    #[test]
    fn split_keeps_view_lens_and_stored_tokens_apart() {
        for spelling in ["deferred", "Deferred", " DEFERRED "] {
            let split = split_status_filter_spec(spelling);
            assert_eq!(
                (split.lens_keys, split.view_axes, split.residual),
                (Vec::new(), vec![ViewFilterAxis::Deferred], None),
                "{spelling} must resolve to the deferred view axis"
            );
        }
        let split = split_status_filter_spec("deferred,shelved,approved,deferred");
        assert_eq!(
            (split.lens_keys, split.view_axes, split.residual),
            (
                vec!["Shelved"],
                vec![ViewFilterAxis::Deferred],
                Some("approved".to_string())
            ),
            "a repeated view token dedups and the other two channels survive"
        );
    }

    /// AC1's display override, at the unit level: a deferred spec stops
    /// advertising the status it was deferred OUT of, quotes its revisit
    /// trigger, and leaves the stored status untouched (AC3's precondition).
    // trace:BUG-1687 | ai:claude
    #[test]
    fn deferred_label_overrides_the_stored_status() {
        let mut req =
            aida_core::models::Requirement::new("deferred fixture".to_string(), String::new());
        req.status = aida_core::RequirementStatus::NeedsAttention;
        assert_eq!(
            deferred_display_label(&req),
            None,
            "a spec that is not deferred must wear no deferred label"
        );

        req.deferred = true;
        req.deferred_until = Some("after the stability push".to_string());
        assert_eq!(
            deferred_display_label(&req).as_deref(),
            Some("Deferred (after the stability push)"),
            "the badge must name the revisit trigger, not the stored status"
        );
        assert_eq!(
            req.status,
            aida_core::RequirementStatus::NeedsAttention,
            "the override is presentation-only: the stored status must survive"
        );
        let badge = parked_status_badge(&req);
        assert!(
            badge.contains("Deferred") && !badge.contains("Needs"),
            "deferral outranks the parked lens in the badge: {badge:?}"
        );

        // The legacy `deferred:*` parking tag is the same shelf, and supplies
        // the trigger when the field is empty — the list filter honours both,
        // so the label has to as well (AC4).
        let mut tagged =
            aida_core::models::Requirement::new("tag-deferred".to_string(), String::new());
        tagged.status = aida_core::RequirementStatus::Approved;
        tagged.tags = ["deferred:post-stability".to_string()]
            .into_iter()
            .collect();
        assert_eq!(
            deferred_display_label(&tagged).as_deref(),
            Some("Deferred (post-stability)")
        );

        // No trigger recorded: still deferred, just unqualified.
        let mut bare = aida_core::models::Requirement::new("bare-defer".to_string(), String::new());
        bare.deferred = true;
        assert_eq!(deferred_display_label(&bare).as_deref(), Some("Deferred"));
    }
}
