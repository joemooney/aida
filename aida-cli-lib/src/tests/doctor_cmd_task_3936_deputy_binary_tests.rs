// trace:TASK-3936 | ai:claude
use super::*;
use crate::review_watch::{service, train_service};

/// The review-watch and auto-train services hosted by one deputy on a deleted
/// image both report the same `deputy:pid` id; doctor reports it once. The
/// host is this test process and its image link is injected, so the fixture
/// does not depend on the runner's processes or /proc layout.
#[test]
fn task3936_one_deputy_hosting_both_services_is_reported_once() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut env = crate::test_env::AmbientGuard::hermetic(root, None);
    env.set_key("AIDA_DEV_LIVE_DIR", root);
    let (exe, _) = service::deleted_image_link(root);
    let pid = std::process::id();
    service::write_test_host_receipt(root, "night", pid);
    train_service::write_test_host_receipt(root, "night", pid);
    let both = || {
        [
            service::deputy_binary_finding_at(root, |_| exe.clone()),
            train_service::deputy_binary_finding_at(root, |_| exe.clone()),
        ]
    };

    let id = format!("night:{pid}");
    for (name, finding) in ["review-watch", "auto-train"].into_iter().zip(both()) {
        let finding = finding.unwrap_or_else(|| panic!("{name} reports the deputy"));
        assert_eq!(finding.id, id, "{name}");
        assert!(
            finding.summary.contains(name),
            "{name}: {}",
            finding.summary
        );
    }

    let mut findings = Vec::new();
    extend_deputy_binary_findings(&mut findings, both());
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert_eq!(findings[0].category, service::DEPUTY_BINARY_CATEGORY);
    assert_eq!(findings[0].id, id);
    extend_deputy_binary_findings(&mut findings, both());
    assert_eq!(findings.len(), 1, "an already-reported id: {findings:#?}");
    // The `stale-deputy` alias selects the same category.
    assert!(
        doctor_category_selected(Some("stale-deputy"), service::DEPUTY_BINARY_CATEGORY).unwrap()
    );
}
