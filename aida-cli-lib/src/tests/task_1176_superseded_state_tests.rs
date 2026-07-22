//! TASK-1176: `Superseded` — the terminal-but-ADOPTED state.
//!
//! Before this, a spec that was adopted and then replaced by a later one had no
//! honest state. Downstream shipped `status=rejected` + a `superseded-by:ADR-N`
//! string tag, which renders identically to a DECLINED decision: a decision
//! followed for months looked exactly like one that was turned down.
//!
//! These tests pin the four halves of the fix that live on the CLI side:
//!
//! - the status parses / validates and echoes in the valid-status list,
//! - it is TERMINAL — dropped by the default `aida list` open lens and by the
//!   `aida status` open tally, exactly like Completed/Rejected,
//! - it RENDERS distinctly from Rejected (never the red ✗ that means declined),
//! - and its successor link is a typed relationship, not a tag.
//!
//! The persistence round-trip (YAML + SQLite cache) and the epic-rollup half
//! live beside the code they exercise, in `aida-core`
//! (`db::cache::tests::superseded_round_trips_through_yaml_and_cache`,
//! `rollup::tests::superseded_child_is_resolved_in_the_epic_rollup`).
//!
//! trace:TASK-1176 | ai:claude

use super::*;

/// The default (Unicode) status glyph. `status_glyph_literal` is private to
/// `status_display`, so route through the profile-aware entry point — the
/// Unicode profile reproduces the literal byte-for-byte.
fn unicode_glyph(status: &str) -> &'static str {
    status_display::status_glyph_for_profile(status, crate::glyphs::GlyphProfile::Unicode)
}

/// The status is a first-class member of the canonical set: both validators
/// accept every spelling a user might type, and the refusal message advertises
/// it (otherwise the state exists but is undiscoverable).
#[test]
fn superseded_parses_validates_and_is_advertised() {
    for spelling in ["superseded", "Superseded", "SUPERSEDED", " superseded "] {
        assert_eq!(
            validate_status_input(spelling),
            Ok("Superseded"),
            "{spelling:?} must validate"
        );
    }
    assert_eq!(
        parse_status("superseded").unwrap(),
        aida_core::RequirementStatus::Superseded
    );

    // The valid-status list every refusal echoes names it.
    let err = validate_status_input("supercede").unwrap_err();
    assert!(
        err.contains("superseded"),
        "the valid-status list must advertise it: {err}"
    );

    // Round-trip through the model's own setter — a canonical status must land
    // as the typed variant and never leak into `custom_status`.
    let mut req = aida_core::Requirement::new("t".into(), "d".into());
    req.set_status_from_str("superseded");
    assert_eq!(req.status, aida_core::RequirementStatus::Superseded);
    assert!(req.custom_status.is_none());
    assert_eq!(req.effective_status(), "Superseded");
}

/// Acceptance 3, part 1: Superseded is TERMINAL. Every gate that asks "is this
/// still open?" — the archive invariant, the BUG-64 parent guard, the queue
/// projections — routes through these two predicates, so pinning them pins the
/// whole class of surfaces at once.
#[test]
fn superseded_is_terminal_like_completed_and_rejected() {
    use aida_core::RequirementStatus as S;

    assert!(is_terminal_status(&S::Superseded));
    assert!(is_terminal_status_str("Superseded"));
    assert!(is_terminal_status_str("superseded"));

    // ...and the lifecycle model — the single source both read — agrees.
    assert!(aida_core::lifecycle::State::from_status(&S::Superseded).is_terminal());

    // The open/closed alias split is the mechanism the default `aida list`
    // lens uses: `open` is a POSITIVE filter, so being absent from it is what
    // keeps a superseded spec out of the default view.
    assert!(
        !S::open_statuses().contains(&S::Superseded),
        "superseded must NOT be an open status"
    );
    assert!(
        S::closed_statuses().contains(&S::Superseded),
        "superseded must be reachable via `aida list closed`"
    );
    assert_eq!(
        S::expand_filter_token("closed"),
        Some(vec!["Done", "Completed", "Rejected", "Superseded"])
    );
    // A still-open state is untouched by all of this.
    assert!(!is_terminal_status(&S::Approved));
}

/// Acceptance 3, part 2: the `aida status` open tally must not count a
/// superseded spec as open work — the same rule BUG-781 applied to accepted
/// decisions. Fails without the fix (`open` = 3).
#[test]
fn superseded_is_not_open_work_in_status_counts() {
    let counts = fast_status_counts([
        ("Superseded", "Decision"), // adopted, then replaced — closed
        ("Rejected", "Decision"),   // declined — closed
        ("Draft", "Decision"),      // proposed — open
        ("Approved", "Task"),       // cleared to start — open
    ]);

    assert_eq!(counts.total, 4, "all four are still real specs");
    assert_eq!(
        counts.open, 2,
        "superseded and rejected are both closed; the draft ADR and the task are open"
    );
}

/// Acceptance 2, the heart of the spec: a superseded spec must be visibly
/// DISTINCT from a rejected one. Rejected means "we said no"; superseded means
/// "we said yes, then handed off". They must not share a glyph, a colour, or a
/// label.
#[test]
fn superseded_renders_distinctly_from_rejected() {
    // Glyph: the adopted box family (☑ → ⊡), never the ✗ that means declined.
    assert_eq!(unicode_glyph("Superseded"), "⊡");
    assert_eq!(unicode_glyph("superseded"), "⊡");
    assert_ne!(
        unicode_glyph("Superseded"),
        unicode_glyph("Rejected"),
        "superseded must not wear the rejected glyph"
    );

    // Colour: the closed-GREEN family (the spec WAS followed), dimmed because
    // a successor governs — emphatically not the red a declined spec wears.
    let superseded = status_display::paint_status("Superseded", "Superseded");
    assert_eq!(superseded.fgcolor, Some(colored::Color::Green));
    assert!(superseded.style.contains(colored::Styles::Dimmed));
    let rejected = status_display::paint_status("Rejected", "Rejected");
    assert_eq!(rejected.fgcolor, Some(colored::Color::Red));

    // ...and it is also distinguishable from the two green neighbours, so the
    // eye can tell "merged", "ratified" and "replaced" apart.
    assert_ne!(unicode_glyph("Superseded"), unicode_glyph("Completed"));
    assert_ne!(unicode_glyph("Superseded"), unicode_glyph("Accepted"));

    // The badge a single-status surface renders carries both signals, and the
    // vocabulary stays honest: "Superseded", never "Rejected".
    colored::control::set_override(false);
    let badge = status_display::status_badge("Superseded");
    colored::control::unset_override();
    assert_eq!(badge, "⊡ Superseded", "badge: {badge:?}");
    assert!(!badge.contains("Rejected"));
}

/// The ASCII profile must carry the distinction too — an operator on a
/// non-Unicode terminal is exactly the person who cannot afford a mojibake box
/// where the declined-vs-replaced difference lives.
#[test]
fn superseded_glyph_is_profile_aware_and_distinct_in_ascii() {
    use crate::glyphs::GlyphProfile;
    assert_eq!(
        status_display::status_glyph_for_profile("Superseded", GlyphProfile::Unicode),
        "⊡"
    );
    assert_eq!(
        status_display::status_glyph_for_profile("Superseded", GlyphProfile::Ascii),
        "[=]"
    );
    assert_ne!(
        status_display::status_glyph_for_profile("Superseded", GlyphProfile::Ascii),
        status_display::status_glyph_for_profile("Rejected", GlyphProfile::Ascii),
        "the ASCII fallback must preserve the distinction"
    );
}

/// Acceptance 4: the superseded-by link is a FIRST-CLASS typed edge. The CLI
/// spellings (`aida rel add --type superseded-by`, `--superseded-by`) must land
/// on the typed variant — a `Custom("superseded-by")` edge would be invisible
/// to `aida graph` / `query_graph`, i.e. no better than the string tag this
/// spec exists to replace.
#[test]
fn superseded_by_is_a_typed_edge_not_a_custom_string() {
    use aida_core::models::RelationshipType as R;

    for spelling in ["superseded-by", "superseded_by", "supersededby"] {
        assert_eq!(R::from_str(spelling), R::SupersededBy, "{spelling}");
        assert!(
            !matches!(R::from_str(spelling), R::Custom(_)),
            "{spelling} must not degrade to a Custom edge"
        );
    }
    assert_eq!(R::from_str("supersedes"), R::Supersedes);

    // It is in the standard set, so `aida rel add --type` neither warns about
    // an "invisible edge" nor offers a did-you-mean for a correct spelling.
    assert!(STANDARD_REL_TYPES.contains(&"superseded-by"));
    assert!(STANDARD_REL_TYPES.contains(&"supersedes"));
    assert_eq!(nearest_standard_rel_type("superseded-by"), None);

    // Both directions render a human phrase (the `aida show` Relationships
    // section), so neither endpoint prints a bare enum name.
    assert_eq!(relationship_phrase(&R::SupersededBy), "is superseded by");
    assert_eq!(relationship_phrase(&R::Supersedes), "supersedes");
    assert_eq!(rel_type_label(&R::SupersededBy), "superseded-by");
}

/// The lifecycle model declares the transition, so `aida show` / the agent
/// next-step block suggest the move — and the suggestion carries the flag that
/// records the successor, not a bare status flip that would leave the link
/// unrecorded.
#[test]
fn superseded_is_a_declared_transition_with_a_successor_carrying_nudge() {
    use aida_core::lifecycle::{is_declared, State};

    assert!(
        is_declared(State::Approved, State::Superseded),
        "an accepted (Approved) spec must be able to reach Superseded"
    );

    let steps = crate::help_next::spec_next("approved", "ADR-3");
    let cmds: Vec<&str> = steps.iter().map(|s| s.cmd.as_str()).collect();
    assert!(
        cmds.contains(&"aida edit ADR-3 --status superseded --superseded-by <NEW-ID>"),
        "the nudge must carry --superseded-by: {cmds:?}"
    );

    // A superseded spec is terminal, so its only remaining move is the archive
    // off-ramp — no forward transition is suggested.
    let terminal = crate::help_next::spec_next("superseded", "ADR-3");
    let cmds: Vec<&str> = terminal.iter().map(|s| s.cmd.as_str()).collect();
    assert_eq!(cmds, vec!["aida archive ADR-3"], "cmds: {cmds:?}");
}
