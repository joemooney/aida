//! BUG-788: `aida list open` (the explicit open shortcut) must hide accepted
//! decisions EXACTLY as bare `aida list` does.
//!
//! BUG-781 taught the DEFAULT open lens that an accepted ADR (a `decision` spec
//! at `Approved`) is terminal and drops it. But the exclusion was gated on the
//! `default_open_lens` flag, which is only set when NO status filter is in play.
//! The `open` shortcut SETS a status filter (it expands to the open status set),
//! so `default_open_lens` was `false` and the accepted ADRs leaked back in — the
//! two verbs disagreed about whether an accepted ADR is open work.
//!
//! These tests pin the fix at the pure-logic layer the two `aida list` code
//! paths share: the alias detector, the folded lens, and — the acceptance
//! criterion — that both verbs return the SAME open set w.r.t. accepted ADRs,
//! while `--type decision` still lists them.
//!
//! trace:BUG-788 | ai:claude

use super::*;

/// A minimal cache-projection row — only `req_type` / `status` matter to the
/// lens; the rest is inert filler. (Mirrors the BUG-781 test helper.)
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

/// A representative listing: an accepted ADR, a proposed ADR, an approved task,
/// and an in-progress bug.
fn listing() -> Vec<aida_core::RequirementSummary> {
    vec![
        row("ADR-23", "Decision", "Approved"),
        row("ADR-24", "Decision", "Approved"),
        row("ADR-30", "Decision", "Draft"),
        row("TASK-9", "Task", "Approved"),
        row("BUG-3", "Bug", "InProgress"),
    ]
}

/// Resolve the same `open_work_lens` the `aida list` command computes, given the
/// raw status spec (`None` for bare list, `Some("open")` for the shortcut) and
/// the view-widening flags. Mirrors the call sequence in `git_backend_cmd.rs`:
/// `default_open_lens` reflects whether an EXPANDED status filter is in play, so
/// bare list (no filter) has it, and the `open` shortcut (a filter) does not.
fn open_work_lens_for(raw_status: Option<&str>, all: bool, archived: bool, deferred: bool) -> bool {
    let explicit_open_alias = status_spec_is_open_alias(raw_status);
    // The command sets a status filter for any non-None raw spec (bare list is
    // the only status-free entry), which is exactly what turns the default lens
    // off — model it the same way here.
    let has_status = raw_status.is_some();
    let default_open_lens = list_default_open_lens(has_status, all, archived, deferred);
    list_applies_open_work_lens(
        default_open_lens,
        explicit_open_alias,
        all,
        archived,
        deferred,
    )
}

/// Apply the lens the way the command does and return the surviving ids.
fn open_set(raw_status: Option<&str>, asked_for_decision_type: bool) -> Vec<String> {
    let mut reqs = listing();
    let lens = open_work_lens_for(raw_status, false, false, false);
    hide_accepted_decisions(&mut reqs, lens, asked_for_decision_type);
    ids(&reqs)
}

/// THE acceptance criterion: bare `aida list` and `aida list open` return the
/// SAME open set, and both drop the accepted ADRs. Fails before the fix — the
/// `open` shortcut kept ADR-23 / ADR-24 while bare list hid them.
#[test]
fn bare_list_and_open_shortcut_agree_on_accepted_decisions() {
    let bare = open_set(None, false);
    let open_shortcut = open_set(Some("open"), false);

    assert_eq!(
        bare, open_shortcut,
        "`aida list` and `aida list open` must return the same open set"
    );
    assert_eq!(
        open_shortcut,
        vec!["ADR-30", "TASK-9", "BUG-3"],
        "both verbs hide the accepted ADRs and keep the proposed ADR + open work"
    );
}

/// The `open` shortcut alone must hide accepted ADRs (the reported defect).
#[test]
fn open_shortcut_hides_accepted_decisions() {
    let mut reqs = listing();
    let lens = open_work_lens_for(Some("open"), false, false, false);
    assert!(
        lens,
        "the explicit `open` shortcut takes the open-work lens"
    );

    let hidden = hide_accepted_decisions(&mut reqs, lens, false);
    assert_eq!(hidden, 2, "both accepted ADRs are hidden");
    assert!(!ids(&reqs).iter().any(|id| id == "ADR-23" || id == "ADR-24"));
}

/// `--type decision` still lists them, whether the verb was bare list or the
/// `open` shortcut — the explicit class ask overrides the lens.
#[test]
fn type_decision_ask_keeps_accepted_decisions_under_both_verbs() {
    assert_eq!(
        open_set(None, true),
        vec!["ADR-23", "ADR-24", "ADR-30", "TASK-9", "BUG-3"],
    );
    assert_eq!(
        open_set(Some("open"), true),
        vec!["ADR-23", "ADR-24", "ADR-30", "TASK-9", "BUG-3"],
    );
}

/// The alias detector: only the lone `open` token (case-insensitive) counts. A
/// wider or different spec is a deliberate ask and keeps the terminals visible.
#[test]
fn status_spec_is_open_alias_matches_only_the_lone_open_token() {
    assert!(status_spec_is_open_alias(Some("open")));
    assert!(status_spec_is_open_alias(Some("Open")));
    assert!(status_spec_is_open_alias(Some("OPEN")));
    assert!(status_spec_is_open_alias(Some("  open  ")));

    assert!(!status_spec_is_open_alias(None));
    assert!(!status_spec_is_open_alias(Some("closed")));
    assert!(!status_spec_is_open_alias(Some("draft")));
    assert!(!status_spec_is_open_alias(Some("open,closed")));
    assert!(!status_spec_is_open_alias(Some("draft,approved")));
    assert!(!status_spec_is_open_alias(Some("")));
}

/// Widening the view (`--all`) clears the lens even for the `open` shortcut —
/// BUG-781's guarantee that `--all` shows accepted ADRs must survive
/// `aida list open --all`. Without the widening yield the explicit-open arm
/// would wrongly hide them.
#[test]
fn all_flag_keeps_accepted_decisions_even_with_open_shortcut() {
    assert!(
        !open_work_lens_for(None, true, false, false),
        "aida list --all"
    );
    assert!(
        !open_work_lens_for(Some("open"), true, false, false),
        "aida list open --all must still show accepted ADRs"
    );

    let mut reqs = listing();
    let lens = open_work_lens_for(Some("open"), true, false, false);
    assert_eq!(hide_accepted_decisions(&mut reqs, lens, false), 0);
    assert_eq!(reqs.len(), 5, "--all keeps every row, incl. accepted ADRs");
}
