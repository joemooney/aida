use super::*;
use aida_core::{Relationship, RelationshipType};

#[test]
fn test_parse_pr_arg() {
    // Valid shapes: PR-N, pr-N, Pr-N, #N, N, prN
    assert_eq!(parse_pr_arg("PR-123"), Some(123));
    assert_eq!(parse_pr_arg("pr-456"), Some(456));
    assert_eq!(parse_pr_arg("Pr-789"), Some(789));
    assert_eq!(parse_pr_arg("#1234"), Some(1234));
    assert_eq!(parse_pr_arg("5678"), Some(5678));
    assert_eq!(parse_pr_arg("pr123"), Some(123));

    // Spaces
    assert_eq!(parse_pr_arg("  PR-123  "), Some(123));

    // Invalid shapes
    assert_eq!(parse_pr_arg("abc"), None);
    assert_eq!(parse_pr_arg("TASK-123"), None);
    assert_eq!(parse_pr_arg(""), None);
    assert_eq!(parse_pr_arg("PR-"), None);
    assert_eq!(parse_pr_arg("#"), None);
}

fn mock_req(title: &str, description: &str, spec_id: Option<&str>) -> Requirement {
    let mut req = Requirement::new(title.to_string(), description.to_string());
    req.spec_id = spec_id.map(|s| s.to_string());
    req.status = RequirementStatus::Approved;
    req.req_type = RequirementType::Task;
    req
}

#[test]
fn test_resolve_pr_to_spec_success_title() {
    let mut store = RequirementsStore::new();
    // A review story that backs PR 123 and names STORY-101 in title
    let review_story = mock_req(
        "Review PR-123: implements STORY-101",
        "This is a review story",
        None,
    );
    store.requirements.push(review_story);

    let root = std::path::Path::new("/tmp");
    let resolved = resolve_pr_to_spec(root, 123, &store).unwrap();
    assert_eq!(resolved, "STORY-101");
}

#[test]
fn test_resolve_pr_to_spec_success_description() {
    let mut store = RequirementsStore::new();
    // A review story that backs PR 123 and names TASK-102 in description
    let review_story = mock_req(
        "Review PR-123: some pr title",
        "This PR implements TASK-102 nicely",
        None,
    );
    store.requirements.push(review_story);

    let root = std::path::Path::new("/tmp");
    let resolved = resolve_pr_to_spec(root, 123, &store).unwrap();
    assert_eq!(resolved, "TASK-102");
}

#[test]
fn reduce_to_most_specific_drops_epic_keeps_child() {
    // BUG-431 #2: a PR backing an epic + its child story resolves to the
    // child, not "multiple backing specs".
    let mut store = RequirementsStore::new();
    let mut epic = mock_req("Epic", "", Some("EPIC-11"));
    let story = mock_req("Story", "", Some("STORY-76"));
    // EPIC is parent of STORY.
    epic.relationships.push(Relationship {
        rel_type: RelationshipType::Parent,
        target_id: story.id,
        created_at: None,
        created_by: None,
    });
    store.requirements.push(epic);
    store.requirements.push(story);

    assert_eq!(
        reduce_to_most_specific_specs(&store, &["EPIC-11".into(), "STORY-76".into()]),
        vec!["STORY-76".to_string()]
    );
    // Order-independent.
    assert_eq!(
        reduce_to_most_specific_specs(&store, &["STORY-76".into(), "EPIC-11".into()]),
        vec!["STORY-76".to_string()]
    );
}

#[test]
fn reduce_to_most_specific_keeps_genuinely_unrelated() {
    // No ancestry between them → both survive → caller still bails (real
    // ambiguity, not an epic/child collapse).
    let mut store = RequirementsStore::new();
    store.requirements.push(mock_req("A", "", Some("STORY-1")));
    store.requirements.push(mock_req("B", "", Some("STORY-2")));
    assert_eq!(
        reduce_to_most_specific_specs(&store, &["STORY-1".into(), "STORY-2".into()]).len(),
        2
    );
}

#[test]
fn test_resolve_pr_to_spec_success_relationship() {
    let mut store = RequirementsStore::new();
    let target_req = mock_req(
        "Target requirement title",
        "Target requirement description",
        Some("BUG-103"),
    );
    let mut review_story = mock_req(
        "Review PR-123: some pr title",
        "Description without spec id",
        None,
    );
    review_story.relationships.push(Relationship {
        rel_type: RelationshipType::References,
        target_id: target_req.id,
        created_at: None,
        created_by: None,
    });
    store.requirements.push(target_req);
    store.requirements.push(review_story);

    let root = std::path::Path::new("/tmp");
    let resolved = resolve_pr_to_spec(root, 123, &store).unwrap();
    assert_eq!(resolved, "BUG-103");
}

#[test]
fn test_resolve_pr_to_spec_no_backing_spec() {
    let mut store = RequirementsStore::new();
    // Review story backs PR 123 but has no backing spec mentioned anywhere
    let review_story = mock_req("Review PR-123: empty", "No description", None);
    store.requirements.push(review_story);

    let root = std::path::Path::new("/tmp");
    let err = resolve_pr_to_spec(root, 123, &store).unwrap_err();
    assert!(err.to_string().contains("has no backing specs"));
}

#[test]
fn test_resolve_pr_to_spec_ambiguous() {
    let mut store = RequirementsStore::new();
    // Review story has multiple distinct spec IDs
    let review_story = mock_req(
        "Review PR-123: STORY-101 and TASK-102",
        "No description",
        None,
    );
    store.requirements.push(review_story);

    let root = std::path::Path::new("/tmp");
    let err = resolve_pr_to_spec(root, 123, &store).unwrap_err();
    assert!(err.to_string().contains("has multiple backing specs"));
}

/// BUG-440: delivered wins over referenced; falls back to referenced when
/// nothing is delivered; ambiguity is reported within the chosen pool.
#[test]
fn pick_pr_spec_prefers_delivered() {
    let d = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // Delivers TASK-642, merely traces STORY-492 → TASK-642.
    assert_eq!(
        pick_pr_spec(&d(&["TASK-642"]), &d(&["STORY-492"])),
        PrSpecChoice::One("TASK-642".into())
    );
    // No delivery signal → fall back to the single referenced spec.
    assert_eq!(
        pick_pr_spec(&d(&[]), &d(&["STORY-492"])),
        PrSpecChoice::One("STORY-492".into())
    );
    // Nothing anywhere.
    assert_eq!(pick_pr_spec(&d(&[]), &d(&["x"; 0])), PrSpecChoice::None);
    // Two genuinely-delivered specs still ambiguous (referenced ignored).
    assert_eq!(
        pick_pr_spec(&d(&["A-1", "B-2"]), &d(&["C-3"])),
        PrSpecChoice::Ambiguous(d(&["A-1", "B-2"]))
    );
    // No delivery, ambiguous references → ambiguous.
    assert_eq!(
        pick_pr_spec(&d(&[]), &d(&["A-1", "B-2"])),
        PrSpecChoice::Ambiguous(d(&["A-1", "B-2"]))
    );
}

/// BUG-440: a review story that COVERS one spec (relationship = delivered)
/// while its free-text description merely traces another resolves to the
/// covered spec — not an ambiguity error. The STORY-501 / TASK-642 case.
#[test]
fn resolve_pr_to_spec_prefers_covered_over_traced() {
    let mut store = RequirementsStore::new();
    let delivered = mock_req("Delivered spec", "", Some("TASK-642"));
    // A second spec exists in the store and is only *traced* in the desc.
    store
        .requirements
        .push(mock_req("Traced spec", "", Some("STORY-492")));
    let mut review_story = mock_req(
        "Review PR-456: ship the thing",
        "Implements per design. trace:STORY-492 (informational).",
        None,
    );
    review_story.relationships.push(Relationship {
        rel_type: RelationshipType::Custom("implements".into()),
        target_id: delivered.id,
        created_at: None,
        created_by: None,
    });
    store.requirements.push(delivered);
    store.requirements.push(review_story);

    let root = std::path::Path::new("/nonexistent-no-gh");
    let resolved = resolve_pr_to_spec(root, 456, &store).unwrap();
    assert_eq!(resolved, "TASK-642");
}
