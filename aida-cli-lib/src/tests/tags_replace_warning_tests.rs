use super::{
    collect_doctor_findings, doctor_cmd::heal_doctor_parent_tag_drift, dropped_structural_tags,
    enforce_structural_tag_replacement, tags_replace_warning,
};
use aida_core::{models::Relationship, RelationshipType, Requirement, RequirementsStore, Storage};
use std::collections::HashSet;

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

// BUG-545: the loud-on-clobber warning fires when `--tags` shrinks a
// multi-tag set down to one, and shows the old→new diff (sorted).
// trace:BUG-545 | ai:claude
#[test]
fn warns_with_sorted_old_and_new_on_clobber() {
    let old = set(&["papercut", "from-friction", "safety", "aida:edit"]);
    let new = set(&["supervised"]);
    let msg = tags_replace_warning(&old, &new).expect("clobber should warn");
    assert!(
        msg.contains("was: aida:edit,from-friction,papercut,safety"),
        "old set should be listed sorted: {msg}"
    );
    assert!(
        msg.contains("now: supervised"),
        "new set should be listed: {msg}"
    );
    assert!(
        msg.contains("--add-tag/--remove-tag"),
        "warning should point at the incremental flags: {msg}"
    );
}

// BUG-545: no warning when the replace is a true no-op (same set).
// trace:BUG-545 | ai:claude
#[test]
fn no_warning_when_set_unchanged() {
    let old = set(&["a", "b"]);
    let new = set(&["b", "a"]);
    assert!(tags_replace_warning(&old, &new).is_none());
}

// BUG-545: clearing all tags renders the new set as `(none)`.
// trace:BUG-545 | ai:claude
#[test]
fn empty_new_set_renders_none() {
    let old = set(&["a"]);
    let new: HashSet<String> = HashSet::new();
    let msg = tags_replace_warning(&old, &new).expect("clearing should warn");
    assert!(
        msg.contains("now: (none)"),
        "empty new set is (none): {msg}"
    );
}

// trace:BUG-1252 | ai:codex
#[test]
fn refuses_each_structural_family_but_allows_force() {
    for tag in [
        "parent:EPIC-1",
        "batch:nightly",
        "lane:research",
        "severity:high",
        "lifecycle:skip-review",
        "aida:internal",
    ] {
        let old = set(&[tag, "ordinary"]);
        let new = set(&["ordinary"]);
        let error = enforce_structural_tag_replacement(&old, &new, false).unwrap_err();
        assert!(
            error.to_string().contains(tag),
            "missing dropped tag: {error}"
        );
        assert_eq!(
            enforce_structural_tag_replacement(&old, &new, true).unwrap(),
            vec![tag]
        );
    }
}

#[test]
fn reports_only_removed_structural_tags_sorted() {
    let old = set(&["severity:high", "parent:EPIC-2", "ordinary"]);
    let new = set(&["ordinary", "batch:new"]);
    assert_eq!(
        dropped_structural_tags(&old, &new),
        vec!["parent:EPIC-2", "severity:high"]
    );
}

#[test]
fn headless_refusal_has_machine_readable_shape_and_remediation() {
    let error = enforce_structural_tag_replacement(
        &set(&["parent:EPIC-28", "lane:research"]),
        &set(&["ordinary"]),
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.starts_with(
            "AIDA_AGENT_OUTPUT structural_tags_dropped=[lane:research,parent:EPIC-28] refusal="
        ),
        "{error}"
    );
    assert!(
        error.contains("use_--add-tag/--remove-tag_or_pass_--force"),
        "{error}"
    );
}

#[test]
fn doctor_detects_and_heals_all_thirteen_incident_specs() {
    let tmp = tempfile::tempdir().unwrap();
    let store_root = tmp.path().join(".aida-store");
    std::fs::create_dir_all(&store_root).unwrap();
    let storage = Storage::new(&store_root);
    let mut store = RequirementsStore::default();
    let mut parent = Requirement::new("Incident parent".into(), String::new());
    parent.spec_id = Some("EPIC-28".into());
    let parent_id = parent.id;
    store.requirements.push(parent);
    let mut incident_ids: Vec<String> = [
        "BUG-1222",
        "BUG-1224",
        "BUG-1226",
        "BUG-1227",
        "BUG-1229",
        "BUG-1230",
        "BUG-1233",
        "SPIKE-82",
        "STORY-1221",
        "TASK-1271",
        "TASK-1276",
        "TASK-1278",
        "TASK-1-140",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    incident_ids.sort();
    for id in &incident_ids {
        let mut child = Requirement::new(format!("incident {id}"), String::new());
        child.spec_id = Some(id.clone());
        child.relationships.push(Relationship {
            rel_type: RelationshipType::Child,
            target_id: parent_id,
            created_at: None,
            created_by: None,
        });
        store.requirements.push(child);
    }
    storage.save(&store).unwrap();

    let findings = collect_doctor_findings(tmp.path(), &store, Some("parent-tag-drift")).unwrap();
    let found: Vec<_> = findings.iter().map(|f| f.id.clone()).collect();
    assert_eq!(found, incident_ids);
    for finding in &findings {
        heal_doctor_parent_tag_drift(tmp.path(), finding).unwrap();
    }
    let healed = storage.load().unwrap();
    for id in &incident_ids {
        assert!(
            healed
                .get_requirement_by_spec_id(id)
                .unwrap()
                .tags
                .contains("parent:EPIC-28"),
            "{id}"
        );
    }
    assert!(
        collect_doctor_findings(tmp.path(), &healed, Some("parent-tag-drift"))
            .unwrap()
            .is_empty()
    );
}
