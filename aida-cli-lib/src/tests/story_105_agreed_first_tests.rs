//! Resolution order once a local spec has an agreed ID (STORY-HOME-105,
//! advisor ruling Q1): a bare `STORY-NNN` opens the agreed spec first, the
//! show signpost names the local twin by its own agreed ID, and every name of
//! the backfilled spec (agreed, local, label form) still opens it. This
//! deliberately supersedes BUG-HOME-036's "write the local form" advice for
//! backfilled twins; 036's own cases (twins with no agreed ID) are unchanged.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude

use crate::requirements_cli::local_twin_note;
use aida_core::db::DatabaseBackend;
use aida_core::node_alias::{
    agreed_and_local, agreed_id_pending, install_for_thread, NodeAliasMap, ThreadMapGuard,
};
use aida_core::Requirement;

const NODE: &str = "B6B1278890774EF993D2EBC327862470";

fn home() -> ThreadMapGuard {
    let mut map = NodeAliasMap::default();
    map.set("HOME", NODE).unwrap();
    install_for_thread(map)
}

fn spec(id: &str, agreed: Option<&str>, title: &str) -> Requirement {
    let mut req = Requirement::new(title.to_string(), String::new());
    req.spec_id = Some(id.to_string());
    req.agreed_id = agreed.map(str::to_string);
    req
}

#[test]
fn bare_id_opens_the_agreed_spec_and_every_name_of_a_backfilled_spec_resolves() {
    let _guard = home();
    let tmp = tempfile::TempDir::new().unwrap();
    let objects = tmp.path().join("objects");
    let flat = spec("STORY-105", None, "legacy flat spec");
    let local_id = format!("STORY-{NODE}-105");
    let backfilled = spec(&local_id, Some("STORY-5801"), "backfilled local spec");
    for req in [&flat, &backfilled] {
        aida_core::object_store::write_object(&objects, req).unwrap();
    }
    let backend = aida_core::GitBackend::new(tmp.path()).unwrap();
    let open = |id: &str| {
        backend
            .get_requirement_by_spec_id(id)
            .unwrap()
            .map(|r| r.id)
    };
    // Agreed-first: the bare number is the flat spec's agreed id.
    assert_eq!(open("STORY-105"), Some(flat.id));
    // The backfilled spec opens by its agreed id, its canonical local id
    // and its label form.
    assert_eq!(open("STORY-5801"), Some(backfilled.id));
    assert_eq!(open(&local_id), Some(backfilled.id));
    assert_eq!(open("STORY-HOME-105"), Some(backfilled.id));

    // The one-line hint on a bare hit names the twin by its agreed id.
    let note = local_twin_note(&objects, "STORY-105", &flat).expect("a local twin exists");
    assert!(note.contains("STORY-5801"), "{note}");
    assert!(!note.contains(NODE), "{note}");
    // Asking for the twin's own agreed id prints no hint.
    assert_eq!(local_twin_note(&objects, "STORY-5801", &backfilled), None);
}

// trace:STORY-B6B1278890774EF993D2EBC327862470-105.acbd0123 | ai:claude
#[test]
fn display_puts_the_agreed_id_first_and_marks_a_pending_one() {
    let _guard = home();
    let local_id = format!("STORY-{NODE}-105");
    assert_eq!(
        agreed_and_local(Some("STORY-5801"), Some(&local_id)),
        "STORY-5801 (STORY-HOME-105)"
    );
    // A flat spec has one id and renders it alone (advisor ruling Q2).
    assert_eq!(
        agreed_and_local(Some("STORY-105"), Some("STORY-105")),
        "STORY-105"
    );
    assert_eq!(agreed_and_local(None, Some(&local_id)), "STORY-HOME-105");
    assert!(agreed_id_pending(None, Some(&local_id)));
    assert!(!agreed_id_pending(Some("STORY-5801"), Some(&local_id)));
    assert!(
        !agreed_id_pending(None, Some("STORY-105")),
        "a flat id is not pending"
    );
}
