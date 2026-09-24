//! STORY-1434: a carved-out acceptance criterion previously left two halves
//! that could drift apart — stale text staying in the description as the
//! authoritative gate, and the correction landing only in a comment the
//! default `aida show` never surfaced (buried hundreds of lines into the
//! YAML in the real instance the spec cites).
//!
//! These tests pin the pure pieces of the fix:
//!
//! - [`carve_out_description`] strikes the exact criterion text and points
//!   at the spec that now carries it, or refuses (returns `None`) when the
//!   text isn't found verbatim.
//! - [`is_default_visible_comment`] / [`collect_default_visible_comments`]
//!   recognize the `CARVE-OUT:` / `CORRECTION:` / `PROXY DECISION:` comment
//!   markers `aida show`'s default view now surfaces without `-c`.
//! - [`carved_out_targets`] reads the typed `Custom("carved-out-to")` edge —
//!   a `RelationshipType::Custom` variant, so no enum-wide match needed
//!   updating, and already walkable by `aida graph --follow carved-out-to`.
//! - [`do_dispatch::carve_out_pickup_warning`] is the AT-PICKUP visibility
//!   half: the banner `aida do` prints for a drain/operator spec that
//!   carries a carve-out.
//!
//! trace:STORY-1434 | ai:claude

use super::*;
use aida_core::models::{Relationship, RelationshipType};
use aida_core::{Comment, Requirement};

// ── carve_out_description ──────────────────────────────────────────────

#[test]
fn carve_out_description_strikes_the_exact_text_and_points_at_the_target() {
    let desc =
        "## Acceptance\n- a green nightly proves the GitLab path end to end.\n- other stuff.";
    let out = carve_out_description(
        desc,
        "a green nightly proves the GitLab path end to end.",
        "TASK-1425",
    )
    .expect("criterion text is present verbatim");

    assert!(
        !out.contains("a green nightly proves the GitLab path end to end."),
        "the stale criterion text must not survive in the new description: {out}"
    );
    assert!(
        out.contains("TASK-1425"),
        "the pointer must name the spec that now carries the criterion: {out}"
    );
    assert!(
        out.contains("- other stuff."),
        "unrelated text is untouched"
    );
}

#[test]
fn carve_out_description_refuses_when_text_is_not_found_verbatim() {
    let desc = "## Acceptance\n- a green nightly proves the GitLab path end to end.";
    // Genuinely different wording — must not match a substring that isn't
    // actually present.
    assert_eq!(
        carve_out_description(desc, "a red nightly proves the Bitbucket path", "TASK-1425"),
        None
    );
}

#[test]
fn carve_out_description_only_replaces_the_first_occurrence() {
    let desc = "dup dup dup";
    let out = carve_out_description(desc, "dup", "TASK-2").unwrap();
    assert_eq!(
        out.matches("dup").count(),
        2,
        "one `dup` struck, two remain: {out}"
    );
}

#[test]
fn carve_out_description_rejects_empty_criterion() {
    assert_eq!(carve_out_description("anything", "", "TASK-2"), None);
}

// ── is_default_visible_comment / collect_default_visible_comments ───────

#[test]
fn recognizes_every_default_visible_marker_case_insensitively() {
    for body in [
        "CARVE-OUT: \"x\" is now carried by TASK-2.",
        "carve-out: lowercase still counts",
        "Correction: the description is stale.",
        "PROXY DECISION: repointed the closure criterion.",
        "proxy decision: same, lowercase",
    ] {
        assert!(is_default_visible_comment(body), "should match: {body:?}");
    }
}

#[test]
fn does_not_false_positive_on_a_word_that_merely_starts_with_a_marker() {
    // "CORRECTIONAL" must not be treated as an instance of "CORRECTION".
    assert!(!is_default_visible_comment("CORRECTIONAL facility budget"));
    assert!(!is_default_visible_comment("just a normal comment"));
    assert!(!is_default_visible_comment(""));
}

#[test]
fn collects_markers_from_top_level_and_nested_replies() {
    let mut reply = Comment::new("bob".into(), "CORRECTION: actually it's F3 alone.".into());
    reply.parent_id = Some(uuid::Uuid::now_v7());
    let mut top = Comment::new("alice".into(), "just chatting, nothing to see here".into());
    top.replies.push(reply);
    let marked = Comment::new(
        "carol".into(),
        "CARVE-OUT: \"x\" now carried by TASK-9.".into(),
    );

    let comments = vec![top, marked];
    let mut out: Vec<&Comment> = Vec::new();
    collect_default_visible_comments(&comments, &mut out);

    assert_eq!(
        out.len(),
        2,
        "the nested reply AND the top-level marker both surface"
    );
    assert!(out.iter().any(|c| c.content.starts_with("CORRECTION")));
    assert!(out.iter().any(|c| c.content.starts_with("CARVE-OUT")));
}

// ── carved_out_targets ───────────────────────────────────────────────────

#[test]
fn carved_out_targets_reads_the_typed_custom_edge() {
    let mut req = Requirement::new("t".into(), "d".into());
    let target = uuid::Uuid::now_v7();
    let unrelated = uuid::Uuid::now_v7();
    req.relationships.push(Relationship {
        rel_type: RelationshipType::Custom("carved-out-to".to_string()),
        target_id: target,
        created_at: None,
        created_by: None,
    });
    // A same-shaped but different custom edge must not be picked up.
    req.relationships.push(Relationship {
        rel_type: RelationshipType::Custom("carved-from".to_string()),
        target_id: unrelated,
        created_at: None,
        created_by: None,
    });
    // Case-insensitive match (edges may be hand-written via `aida rel add`).
    req.relationships.push(Relationship {
        rel_type: RelationshipType::Custom("CARVED-OUT-TO".to_string()),
        target_id: target,
        created_at: None,
        created_by: None,
    });

    let targets = carved_out_targets(&req);
    assert!(targets.contains(&target));
    assert!(!targets.contains(&unrelated));
}

#[test]
fn carved_out_targets_is_empty_for_a_plain_spec() {
    let req = Requirement::new("t".into(), "d".into());
    assert!(carved_out_targets(&req).is_empty());
}

// ── apply_carve_out (atomicity: edge + comment on the SAME req) ─────────

#[test]
fn apply_carve_out_writes_the_edge_and_a_default_visible_comment_on_one_req() {
    let mut req = Requirement::new("source".into(), "d".into());
    let target_id = uuid::Uuid::now_v7();

    apply_carve_out(
        &mut req,
        target_id,
        "TASK-1",
        "TASK-2",
        "a green nightly proves the GitLab path end to end.",
        Some("live smoke fails before forge-smoke markers emit"),
    );

    assert_eq!(carved_out_targets(&req), vec![target_id]);
    assert_eq!(req.comments.len(), 1, "exactly one comment landed on req");
    let body = &req.comments[0].content;
    assert!(body.starts_with("CARVE-OUT:"), "{body}");
    assert!(body.contains("TASK-2"), "{body}");
    assert!(
        body.contains("live smoke fails before forge-smoke markers emit"),
        "{body}"
    );
    assert!(
        is_default_visible_comment(body),
        "the comment apply_carve_out writes must itself pass the default-visible check: {body}"
    );
}

#[test]
fn apply_carve_out_is_idempotent_on_the_edge() {
    let mut req = Requirement::new("source".into(), "d".into());
    let target_id = uuid::Uuid::now_v7();
    apply_carve_out(&mut req, target_id, "TASK-1", "TASK-2", "x", None);
    apply_carve_out(&mut req, target_id, "TASK-1", "TASK-2", "x", None);
    assert_eq!(
        carved_out_targets(&req),
        vec![target_id],
        "the edge must not duplicate"
    );
    assert_eq!(
        req.comments.len(),
        2,
        "each call still logs its own comment"
    );
}

#[test]
fn apply_carve_out_defaults_the_reason_when_none_given() {
    let mut req = Requirement::new("source".into(), "d".into());
    apply_carve_out(
        &mut req,
        uuid::Uuid::now_v7(),
        "TASK-1",
        "TASK-2",
        "x",
        None,
    );
    assert!(req.comments[0].content.contains("no reason given"));
}

// ── do_dispatch::carve_out_pickup_warning ────────────────────────────────

#[test]
fn pickup_warning_is_absent_when_nothing_is_carved_out() {
    assert_eq!(do_dispatch::carve_out_pickup_warning(&[]), None);
}

#[test]
fn pickup_warning_names_every_carve_out_target() {
    let carried_by = vec![
        ("TASK-1425".to_string(), "live-smoke acceptance".to_string()),
        ("F3".to_string(), String::new()),
    ];
    let warning = do_dispatch::carve_out_pickup_warning(&carried_by).expect("non-empty input");
    assert!(warning.contains("TASK-1425"));
    assert!(warning.contains("live-smoke acceptance"));
    assert!(warning.contains("F3"));
    assert!(
        warning.to_lowercase().contains("no longer gates"),
        "the warning must say the description is not the live gate: {warning}"
    );
}
