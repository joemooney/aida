//! BUG-781: an ACCEPTED decision (an ADR at `Approved`) is terminal — it must
//! read as terminal and must not sit in the default open lens forever.
//!
//! The operator saw two ratified ADRs in a listing wearing the same `Approved`
//! badge a task wears BEFORE anyone starts it, and asked "when will these
//! complete?". BUG-761 fixed the archive *gate*; these tests pin the *rendering
//! + default-view* half:
//!
//! - the default `aida list` lens drops accepted decisions,
//! - `--all` / `--type decision` / an explicit `--status` bring them back,
//! - they render as the terminal `Accepted` (checked glyph, closed colour) in
//!   the human table and as the `accepted` token in the agent/TOON surface,
//! - and the `aida status` open tally stops counting them as open work.
//!
//! trace:BUG-781 | ai:claude

use super::*;

/// A minimal cache-projection row. Only `req_type` / `status` matter to the
/// lens; the rest is inert filler so the tests read as data, not construction.
fn row(spec_id: &str, req_type: &str, status: &str) -> aida_core::RequirementSummary {
    aida_core::RequirementSummary {
        id: uuid::Uuid::new_v4(),
        spec_id: Some(spec_id.to_string()),
        agreed_id: Some(spec_id.to_string()),
        title: format!("{spec_id} title"),
        description: String::new(),
        status: status.to_string(),
        priority: "medium".to_string(),
        owner: String::new(),
        assignee: None,
        feature: "Uncategorized".to_string(),
        req_type: req_type.to_string(),
        tags: Vec::new(),
        created_at: String::new(),
        modified_at: String::new(),
        archived: false,
        archived_at: None,
        deferred: false,
        deferred_at: None,
        deferred_until: None,
        in_degree: 0,
        out_degree: 0,
        heft: 0,
        blocked: false,
        has_pending_decision: false,
        execution_mode: None,
        weight: None,
        origin: None,
        yaml_path: String::new(),
    }
}

fn ids(reqs: &[aida_core::RequirementSummary]) -> Vec<String> {
    reqs.iter()
        .map(|r| r.spec_id.clone().unwrap_or_default())
        .collect()
}

/// A representative listing: an accepted ADR, a proposed ADR, an approved task
/// (still real open work), and an in-progress bug.
fn listing() -> Vec<aida_core::RequirementSummary> {
    vec![
        row("ADR-21", "Decision", "Approved"),
        row("ADR-30", "Decision", "Draft"),
        row("TASK-9", "Task", "Approved"),
        row("BUG-3", "Bug", "InProgress"),
    ]
}

/// THE bug: under the default open lens the accepted ADR must be gone, while
/// every genuinely-open row — including a *proposed* ADR and an approved task —
/// stays. Fails without the fix (ADR-21 survives the lens).
#[test]
fn default_open_lens_hides_accepted_decisions_only() {
    let mut reqs = listing();
    let hidden = hide_accepted_decisions(&mut reqs, true, false);

    assert_eq!(hidden, 1, "exactly the one accepted ADR is hidden");
    assert_eq!(
        ids(&reqs),
        vec!["ADR-30", "TASK-9", "BUG-3"],
        "a proposed ADR and an approved task are still open work"
    );
}

/// `--type decision` is an explicit ask for the decision class, so accepted
/// ADRs must come back — otherwise the class would be unreachable.
#[test]
fn type_decision_ask_keeps_accepted_decisions() {
    let mut reqs = listing();
    let hidden = hide_accepted_decisions(&mut reqs, true, true);

    assert_eq!(hidden, 0);
    assert_eq!(ids(&reqs), vec!["ADR-21", "ADR-30", "TASK-9", "BUG-3"]);
}

/// Widening the view (`--all`, `--archived`, `--deferred`, or any explicit
/// `--status`) clears the default open lens, and with it this exclusion.
#[test]
fn widened_view_keeps_accepted_decisions() {
    for asked_for_decision_type in [false, true] {
        let mut reqs = listing();
        let hidden = hide_accepted_decisions(&mut reqs, false, asked_for_decision_type);
        assert_eq!(hidden, 0, "no lens ⇒ nothing hidden");
        assert_eq!(reqs.len(), 4);
    }
}

/// Case tolerance: the cache spells the Debug form (`Decision` / `Approved`),
/// but a hand-built or legacy row may be lowercase. Both must be recognised.
#[test]
fn lens_is_case_insensitive_over_cache_spellings() {
    for (t, s) in [
        ("Decision", "Approved"),
        ("decision", "approved"),
        ("DECISION", "APPROVED"),
    ] {
        let mut reqs = vec![row("ADR-1", t, s)];
        assert_eq!(
            hide_accepted_decisions(&mut reqs, true, false),
            1,
            "{t} @ {s} must be recognised as accepted"
        );
        assert!(reqs.is_empty());
    }
}

/// The rendering half: an accepted decision must not wear the ambiguous,
/// task-style `Approved` badge anywhere the two surfaces share the lens.
#[test]
fn accepted_decision_renders_as_terminal_in_human_and_agent_surfaces() {
    let accepted = row("ADR-21", "Decision", "Approved");
    let task = row("TASK-9", "Task", "Approved");
    let idle = (false, false, false);

    // Agent / TOON surface: the recognised terminal ADR verb, not `approved`.
    assert_eq!(toon_list_cell(&accepted, idle, "status"), "accepted");
    assert_eq!(
        toon_list_cell(&task, idle, "status"),
        "approved",
        "a task at Approved is unchanged"
    );

    // Human surface: the checked, terminal label — never the cyan `Approved`.
    colored::control::set_override(false);
    let cell = status_display::status_cell(
        status_display::display_status_for_type(&accepted.req_type, &accepted.status),
        11,
    );
    colored::control::unset_override();
    assert!(cell.contains("Accepted"), "cell: {cell:?}");
    assert!(cell.contains('☑'), "cell: {cell:?}");
    assert!(!cell.contains("Approved"), "cell: {cell:?}");
}

/// `aida status`'s open tally shares the lens: an accepted ADR is terminal, so
/// it must not inflate the open backlog. Fails without the fix (`open` = 3).
#[test]
fn accepted_decision_is_not_open_work_in_status_counts() {
    let counts = fast_status_counts([
        ("Approved", "Decision"),
        ("Draft", "Decision"),
        ("Approved", "Task"),
    ]);

    assert_eq!(counts.total, 3, "all three are still real specs");
    assert_eq!(
        counts.open, 2,
        "the accepted ADR is terminal; the proposed ADR and the task are open"
    );
    assert_eq!(counts.draft, 1);
}

/// A listing emptied *only* because accepted ADRs were hidden is not a fresh
/// repo — it must not be told to file its first spec.
#[test]
fn hidden_accepted_decisions_suppress_the_empty_repo_signpost() {
    assert!(empty_list_hint_line(0, 0, 0, 1).is_none());
    assert!(empty_list_hint_line(0, 0, 0, 0).is_some());
}
