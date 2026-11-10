// trace:TASK-3780 | ai:codex
use super::*;

#[test]
fn bug_2490_doctor_flags_markered_object_and_continues_loading() {
    let dir = tempfile::tempdir().unwrap();
    // Build the read-only fixture directly: a whole-store save allocates
    // metadata IDs and requires writer authority unrelated to this scan.
    aida_core::GitBackend::new(dir.path()).unwrap();
    let path = dir.path().join("objects/STORY/000/STORY-bad.yaml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "<<<<<<< Updated upstream\ntitle: conflict\n=======\ntitle: other\n>>>>>>> Stashed changes\n").unwrap();
    // The same tolerant load used by doctor still succeeds.
    Storage::new(dir.path()).load().unwrap();
    let findings = scan_store_objects(dir.path());
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].category, "store-objects");
    assert!(findings[0].summary.contains("conflict markers"));
    assert!(findings[0].id.contains("STORY-bad.yaml"));
    assert!(findings[0].action.contains("STORY-bad.yaml"));
    assert!(!findings[0].safe_heal);
    assert!(doctor_category_selected(Some("store-objects"), "store-objects").unwrap());
}
