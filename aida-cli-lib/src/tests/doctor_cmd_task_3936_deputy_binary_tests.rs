// trace:TASK-3936 | ai:claude
use super::*;
use crate::review_watch::{service, train_service};

/// The review-watch and auto-train services hosted by one deputy on a deleted
/// image both report the same `deputy:pid` id; doctor reports it once.
#[test]
fn task3936_one_deputy_hosting_both_services_is_reported_once() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut env = crate::test_env::AmbientGuard::hermetic(root, None);
    env.set_key("AIDA_DEV_LIVE_DIR", root);
    let deputy = service::spawn_deleted_image_child(root);
    let pid = deputy.child.id();
    service::write_test_host_receipt(root, "night", pid);
    train_service::write_test_host_receipt(root, "night", pid);

    let id = format!("night:{pid}");
    for (name, finding) in [
        ("review-watch", service::deputy_binary_finding(root)),
        ("auto-train", train_service::deputy_binary_finding(root)),
    ] {
        let finding = finding.unwrap_or_else(|| panic!("{name} reports the deputy"));
        assert_eq!(finding.id, id, "{name}");
        assert!(
            finding.summary.contains(name),
            "{name}: {}",
            finding.summary
        );
    }

    let mut findings = Vec::new();
    extend_deputy_binary_findings(root, &mut findings);
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert_eq!(findings[0].category, service::DEPUTY_BINARY_CATEGORY);
    assert_eq!(findings[0].id, id);
    extend_deputy_binary_findings(root, &mut findings);
    assert_eq!(findings.len(), 1, "an already-reported id: {findings:#?}");
    // The `stale-deputy` alias selects the same category.
    assert!(
        doctor_category_selected(Some("stale-deputy"), service::DEPUTY_BINARY_CATEGORY).unwrap()
    );
}
