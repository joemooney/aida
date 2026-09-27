//! BUG-1687: a deferred spec must report ONE state.
//!
//! `aida defer` sets a view-level flag and deliberately leaves the lifecycle
//! status alone (that orthogonality is what makes `aida undefer` lossless). The
//! cost was that every read surface kept printing the stored `Needs Attention` /
//! `In Progress`, so `aida show BUG-1679` and `aida show BUG-1679 --json`
//! answered "what state is this in?" differently and two agents in one session
//! reported the contradiction at each other.
//!
//! These pin the presentation layer the fix routes every surface through. The
//! end-to-end halves — the defer/show/list agreement and the undefer round trip
//! against a real store — live in `aida-cli/tests/bug_1687_deferred_one_state.rs`;
//! the `--status deferred` token split lives beside the enum in
//! `aida-core` (`models::tests::split_filter_spec_peels_the_deferred_view_axis`).
//!
//! trace:BUG-1687 | ai:claude

use super::*;

fn spec(status: RequirementStatus, deferred: bool) -> aida_core::models::Requirement {
    let mut r = Requirement::new("BUG-1687 fixture".to_string(), String::new());
    r.spec_id = Some("BUG-9997".into());
    r.req_type = RequirementType::Bug;
    r.status = status;
    r.deferred = deferred;
    r
}

/// The presented status of a deferred spec is `Deferred`, whatever lifecycle
/// status is stored under it — including the two the spec calls out as implying
/// "act now". A non-deferred spec is unchanged, so the relabel cannot leak.
#[test]
fn presented_status_relabels_only_deferred_specs() {
    for stored in [
        "In Progress",
        "Needs Attention",
        "Approved",
        "Draft",
        "Completed",
    ] {
        assert_eq!(
            status_display::presented_status("Bug", stored, true),
            "Deferred",
            "stored {stored} deferred"
        );
        assert_eq!(
            status_display::presented_status("Bug", stored, false),
            stored,
            "stored {stored} not deferred"
        );
    }
    // The BUG-781 decision relabel still applies to a non-deferred decision,
    // and deferral wins over it when both are in play.
    assert_eq!(
        status_display::presented_status("Decision", "Approved", false),
        "Accepted"
    );
    assert_eq!(
        status_display::presented_status("Decision", "Approved", true),
        "Deferred"
    );
}

/// `Deferred` has its own glyph and colour, and specifically does NOT wear the
/// bold magenta `Needs Attention` wears — the whole point of the relabel is that
/// a parked spec must not read as "decide something here".
#[test]
fn deferred_renders_distinctly_from_needs_attention() {
    assert_eq!(
        status_display::status_glyph_for_profile("Deferred", crate::glyphs::GlyphProfile::Unicode),
        "⏳"
    );
    assert_eq!(
        status_display::status_glyph_for_profile("Deferred", crate::glyphs::GlyphProfile::Ascii),
        "..."
    );
    let deferred = status_display::paint_status("Deferred", "Deferred");
    assert_eq!(deferred.fgcolor, Some(colored::Color::Blue));
    assert!(
        deferred.style.contains(colored::Styles::Dimmed),
        "deferred paints dimmed — parked, not actionable"
    );
    let punted = status_display::paint_status("Needs Attention", "Needs Attention");
    assert_ne!(
        deferred.fgcolor, punted.fgcolor,
        "deferred must not share the punted colour"
    );
    // The badge is the glyph + the label, so the `aida show` Status line and the
    // spec card one-liner both carry the state in plain text too.
    colored::control::set_override(false);
    let badge = status_display::status_badge("Deferred");
    colored::control::unset_override();
    assert_eq!(badge, "⏳ Deferred", "badge: {badge:?}");
}

/// The `--card` / `--brief` badge reads the requirement directly, so deferral has
/// to win there as well — "Shelved (ci-red)" on a parked spec would still read as
/// something to pick up.
#[test]
fn parked_badge_prefers_deferral_over_the_needs_attention_lens() {
    colored::control::set_override(false);
    let deferred =
        status_display::parked_status_badge(&spec(RequirementStatus::NeedsAttention, true));
    let punted =
        status_display::parked_status_badge(&spec(RequirementStatus::NeedsAttention, false));
    colored::control::unset_override();
    assert!(
        deferred.contains("Deferred"),
        "card badge must say Deferred for a deferred spec: {deferred:?}"
    );
    assert!(
        !punted.contains("Deferred"),
        "a merely punted spec must not be labelled Deferred: {punted:?}"
    );
}

/// `aida list --format json` keeps `status` as the stored cache token (the
/// STORY-1352 machine contract) and carries the presented state in the
/// `status_label` channel NeedsAttention rows already use. Deferral outranks both
/// the rework annotation and the parked lens.
#[test]
fn list_json_status_label_prefers_deferral() {
    let (label, lens) = crate::git_backend_cmd::list_json_status_label_lens(true, false, None);
    assert_eq!(label.as_deref(), Some("Deferred"));
    assert_eq!(lens, Some("Deferred"));

    // Deferred + rework: still Deferred, because the spec is parked out of work.
    let (label, _) = crate::git_backend_cmd::list_json_status_label_lens(true, true, None);
    assert_eq!(label.as_deref(), Some("Deferred"));

    // Not deferred → the pre-existing precedence is untouched.
    let (label, lens) = crate::git_backend_cmd::list_json_status_label_lens(false, true, None);
    assert_eq!(label.as_deref(), Some("Rework Needed"));
    assert_eq!(lens, Some("ReworkNeeded"));
    assert_eq!(
        crate::git_backend_cmd::list_json_status_label_lens(false, false, None),
        (None, None)
    );
}

/// The next-step block leads with `aida undefer <id>` on a deferred spec: the
/// lifecycle transitions stay listed and valid, but offering
/// `aida edit <id> --status in-progress` as the headline move on deliberately
/// dequeued work is the same contradiction the status line had.
#[test]
fn next_steps_lead_with_undefer_for_a_deferred_spec() {
    let mut next = crate::help_next::spec_next("In Progress", "BUG-9997");
    let without = next.len();
    assert!(without > 0, "in-progress has lifecycle next steps");
    crate::help_next::lead_with_undefer(&mut next, true, "BUG-9997");
    assert_eq!(next[0].cmd, "aida undefer BUG-9997");
    assert_eq!(
        next.len(),
        without + 1,
        "the transitions are kept, not replaced"
    );

    let mut untouched = crate::help_next::spec_next("In Progress", "BUG-9997");
    crate::help_next::lead_with_undefer(&mut untouched, false, "BUG-9997");
    assert_eq!(untouched.len(), without);
}
