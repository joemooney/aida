// trace:TASK-3780 | ai:codex
use super::*;
use crate::gate::merge_hold_core::LabelState;
use crate::merge_hold::LabelDefinitions;

#[test]
fn bug_1747_label_definition_results_map_to_findings() {
    assert!(
        merge_hold_label_definition_findings(LabelDefinitions::Read {
            present: Vec::new(),
            missing: Vec::new()
        })
        .is_empty()
    );
    let findings = merge_hold_label_definition_findings(LabelDefinitions::Read {
        present: Vec::new(),
        missing: vec!["aida:merge-hold", "aida:merge-hold-recorded"],
    });
    assert_eq!(findings.len(), 2);
    for (finding, name) in findings
        .iter()
        .zip(["aida:merge-hold", "aida:merge-hold-recorded"])
    {
        assert_eq!(finding.id, format!("merge-hold-label-{name}-undefined"));
        assert!(finding.summary.contains(name));
        assert_eq!(finding.action, "aida merge-hold labels --create-missing");
        assert!(!finding.safe_heal);
    }
    let unknown =
        merge_hold_label_definition_findings(LabelDefinitions::Unknown("forge unavailable".into()));
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0].id, "merge-hold-labels-undetermined");
    assert!(unknown[0].summary.contains("forge unavailable"));
    assert!(merge_hold_label_definition_findings(LabelDefinitions::NoForge).is_empty());
}

#[test]
fn bug_1747_only_unsynced_live_holds_are_findings() {
    let dir = tempfile::tempdir().unwrap();
    for pr in [101, 102, 103] {
        crate::gate::merge_hold_core::write_hold(dir.path(), pr, "fixture").unwrap();
    }
    crate::gate::merge_hold_core::record_label_state(
        dir.path(),
        101,
        &LabelState::Unsynced("missing label definition(s): aida:merge-hold".into()),
    )
    .unwrap();
    crate::gate::merge_hold_core::record_label_state(dir.path(), 102, &LabelState::Synced).unwrap();
    let findings = unsynced_merge_hold_findings(dir.path());
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "PR-101-hold-label-unsynced");
    assert_eq!(
        findings[0].action,
        "aida merge-hold list --fix; aida merge-hold labels --create-missing"
    );
}

// Pin the doctor action sniff to the producer's actual Display wording.
// trace:BUG-1747 | ai:codex
#[test]
fn bug_1747_definition_missing_display_recommends_provisioning() {
    let dir = tempfile::tempdir().unwrap();
    crate::gate::merge_hold_core::write_hold(dir.path(), 104, "fixture").unwrap();
    let error = crate::merge_hold::LabelSyncError::DefinitionMissing {
        names: vec!["aida:merge-hold".into()],
        detail: "'aida:merge-hold' not found".into(),
    }
    .to_string();
    crate::gate::merge_hold_core::record_label_state(dir.path(), 104, &LabelState::Unsynced(error))
        .unwrap();
    let findings = unsynced_merge_hold_findings(dir.path());
    assert_eq!(findings.len(), 1);
    assert!(findings[0]
        .action
        .ends_with("aida merge-hold labels --create-missing"));
}
