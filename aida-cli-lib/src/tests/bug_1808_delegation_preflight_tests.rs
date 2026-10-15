//! BUG-1808: `aida queue work` preflights child-seat delegation for every
//! launch arm that later mints a child grant, and its refusal separates the
//! delegation decision from queue selection.
// trace:BUG-1808 | ai:claude

use super::{queue_work_delegation_refusal, queue_work_issues_child_grant};

#[test]
fn every_child_granting_launch_arm_is_preflighted() {
    // (no_launch, no_human, resume, standalone_reviewer) -> issues a child grant
    let cases = [
        // --no-launch never launches a child, whatever else is set.
        ((true, false, false, false), false),
        ((true, true, true, true), false),
        // Interactive launches always mint a child grant.
        ((false, false, false, false), true),
        ((false, false, true, false), true),
        ((false, false, false, true), true),
        // Headless: resume and standalone reviewer mint one; a fresh
        // headless pickup execs the vendor without one.
        ((false, true, true, false), true),
        ((false, true, false, true), true),
        ((false, true, false, false), false),
    ];
    for ((no_launch, no_human, resume, reviewer), want) in cases {
        assert_eq!(
            queue_work_issues_child_grant(no_launch, no_human, resume, reviewer),
            want,
            "no_launch={no_launch} no_human={no_human} resume={resume} reviewer={reviewer}"
        );
    }
}

#[test]
fn refusal_names_delegation_not_queue_selection() {
    let cause = "the active `implementer` grant does not delegate `implementer`";
    let text = queue_work_delegation_refusal(
        "implementer",
        "cluster-derived",
        Some(("implementer", &[])),
        cause,
    );
    assert!(text.starts_with("child-seat delegation refused"), "{text}");
    assert!(text.contains(cause), "{text}");
    assert!(
        text.contains("queue selection succeeded (cluster-derived chose role `implementer`)"),
        "{text}"
    );
    assert!(
        !text.contains('\n'),
        "agent mode keeps only the first line: {text}"
    );
    // An implementer launcher gets the refusal only — no role-selector advice.
    assert!(!text.contains("explicit role selector"), "{text}");
    assert!(!text.contains("--role"), "{text}");
}

#[test]
fn coordinating_launcher_is_told_to_pick_an_explicit_role() {
    for seat in ["advisor", "product", "integrator"] {
        let delegable = vec!["implementer".to_string(), "reviewer".to_string()];
        let text = queue_work_delegation_refusal(
            "architect",
            "cluster-derived",
            Some((seat, &delegable)),
            "cause",
        );
        assert!(
            text.contains(&format!(
                "choose a seat the `{seat}` grant delegates with an explicit role selector: \
                 `--role implementer`, `--role reviewer`"
            )),
            "{text}"
        );
    }
}

#[test]
fn coordinating_launcher_with_no_delegable_seat_is_told_to_re_enter() {
    let text = queue_work_delegation_refusal(
        "implementer",
        "cluster-derived",
        Some(("advisor", &[])),
        "cause",
    );
    assert!(
        text.contains("the `advisor` grant delegates no seat, so re-enter it with `--delegate-seat implementer`"),
        "{text}"
    );
}

#[test]
fn missing_launcher_grant_still_separates_the_two_decisions() {
    let text = queue_work_delegation_refusal(
        "implementer",
        "--role flag",
        None,
        "no valid parent seat grant is active",
    );
    assert!(
        text.contains("no valid parent seat grant is active"),
        "{text}"
    );
    assert!(text.contains("queue selection succeeded"), "{text}");
    assert!(!text.contains("--role implementer"), "{text}");
}
