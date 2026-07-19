use super::*;

/// Build a finding requirement: a draft/non-draft TASK carrying a
/// `from-<source>:<origin>` tag, mirroring how the drain phases file them.
fn finding(spec_id: &str, origin_tag: &str, status: RequirementStatus) -> Requirement {
    let mut r = Requirement::new(spec_id.to_string(), String::new());
    r.spec_id = Some(spec_id.into());
    r.req_type = RequirementType::Task;
    r.status = status;
    r.tags.insert(origin_tag.to_string());
    r
}

/// The count `aida findings list` reports as "awaiting triage": draft
/// requirements carrying a finding tag (`ListFilter { status: draft }` +
/// `is_finding`). Independent re-derivation of the `findings list`
/// predicate so the test pins both surfaces to the same definition.
fn findings_list_awaiting_count(store: &aida_core::RequirementsStore) -> usize {
    store
        .requirements
        .iter()
        .filter(|r| !r.archived)
        .filter(|r| matches!(r.status, RequirementStatus::Draft))
        .filter(|r| findings::is_finding(&r.tags.iter().cloned().collect::<Vec<_>>()))
        .count()
}

/// The count `aida human` reports as open findings (its `open_findings`
/// computation: distinct findings across `collect_findings_by_spec`).
fn human_open_findings_count(store: &aida_core::RequirementsStore) -> usize {
    collect_findings_by_spec(store)
        .values()
        .flat_map(|ids| ids.iter())
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Regression for BUG-544: `aida human`'s open-findings count must equal
/// `aida findings list`'s awaiting-triage count. Before the fix
/// `collect_findings_by_spec` counted EVERY non-archived finding row
/// (including resolved/escalated/decided ones that left draft), inflating
/// the top-line "N open items" total while `findings list` correctly
// showed 0. trace:BUG-544
#[test]
fn human_open_findings_matches_findings_list_awaiting() {
    let store = aida_core::RequirementsStore {
        requirements: vec![
            // One genuinely awaiting-triage finding (still draft).
            finding(
                "TASK-1",
                "from-implementer:STORY-9",
                RequirementStatus::Draft,
            ),
            // A triaged-and-promoted finding: kept its `from-*` tag but
            // left draft. Must NOT count as awaiting triage.
            finding(
                "TASK-2",
                "from-implementer:STORY-9",
                RequirementStatus::Completed,
            ),
            // A rejected (dismissed) advisor finding — also not awaiting.
            finding(
                "TASK-3",
                "from-advisor:STORY-9",
                RequirementStatus::Rejected,
            ),
            // An escalated finding flipped to approved — not awaiting.
            finding("TASK-4", "from-review:PR-7", RequirementStatus::Approved),
        ],
        ..Default::default()
    };

    // The awaiting-triage set is the single draft finding.
    assert_eq!(findings_list_awaiting_count(&store), 1);
    // `aida human` must agree — not count the three triaged rows.
    assert_eq!(
        human_open_findings_count(&store),
        findings_list_awaiting_count(&store),
        "aida human open-findings count must equal aida findings list awaiting count"
    );
}

/// All findings triaged away from draft → both surfaces report zero (the
/// exact operator symptom: human said 17, findings list said 0).
#[test]
fn all_triaged_findings_count_zero_on_both_surfaces() {
    let store = aida_core::RequirementsStore {
        requirements: vec![
            finding(
                "TASK-1",
                "from-advisor:STORY-9",
                RequirementStatus::Completed,
            ),
            finding(
                "TASK-2",
                "from-advisor:STORY-9",
                RequirementStatus::Rejected,
            ),
            finding(
                "TASK-3",
                "from-implementer:STORY-9",
                RequirementStatus::Approved,
            ),
        ],
        ..Default::default()
    };
    assert_eq!(findings_list_awaiting_count(&store), 0);
    assert_eq!(human_open_findings_count(&store), 0);
}
