//! Bare ids vs this node's local numbers: the three operator cases
//! (STORY-100, TASK-123, BUG-030) on the show signpost and the doctor lint.
// trace:BUG-B6B1278890774EF993D2EBC327862470-036.ac4dbc90 | ai:claude

use crate::doctor_cmd::ambiguous_bare_id_lines;
use crate::requirements_cli::local_twin_note;
use aida_core::node_alias::{install_for_thread, NodeAliasMap, ThreadMapGuard};
use aida_core::Requirement;

const NODE: &str = "B6B1278890774EF993D2EBC327862470";

fn home() -> ThreadMapGuard {
    let mut map = NodeAliasMap::default();
    map.set("HOME", NODE).unwrap();
    install_for_thread(map)
}

fn spec(id: &str, title: &str, description: &str) -> Requirement {
    let mut req = Requirement::new(title.to_string(), description.to_string());
    req.spec_id = Some(id.to_string());
    req
}

/// The operator's store: agreed STORY-100 and TASK-123 (legacy), and this
/// node's STORY-HOME-100, TASK-HOME-123 and BUG-HOME-030.
fn store() -> Vec<Requirement> {
    vec![
        spec("STORY-100", "Review PR-10", ""),
        spec("TASK-123", "AGY failure modes", ""),
        spec(&format!("STORY-{NODE}-100"), "agent handshake", ""),
        spec(&format!("TASK-{NODE}-123"), "agent list", ""),
        spec(&format!("BUG-{NODE}-030"), "local bug", ""),
    ]
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-036.ac4dbc90 | ai:claude
#[test]
fn show_note_names_the_local_twin_of_an_agreed_bare_id() {
    let _guard = home();
    let tmp = tempfile::TempDir::new().unwrap();
    let objects = tmp.path().join("objects");
    for req in store() {
        aida_core::object_store::write_object(&objects, &req).unwrap();
    }
    let reqs = store();
    let note = local_twin_note(&objects, "STORY-100", &reqs[0]).expect("STORY-100 has a twin");
    assert!(note.contains("STORY-HOME-100"), "{note}");
    assert!(!note.contains(NODE), "never the canonical node id: {note}");
    let note = local_twin_note(&objects, "task-123", &reqs[1]).expect("TASK-123 has a twin");
    assert!(note.contains("TASK-HOME-123"), "{note}");
    // A local id itself, or a bare id with no local twin, prints nothing.
    assert_eq!(local_twin_note(&objects, "STORY-HOME-100", &reqs[2]), None);
    assert_eq!(local_twin_note(&objects, "STORY-101", &reqs[0]), None);
    // A twin that has an agreed id is named by it (ruling C).
    let mut agreed_twin = reqs[3].clone();
    agreed_twin.agreed_id = Some("TASK-4120".into());
    aida_core::object_store::write_object(&objects, &agreed_twin).unwrap();
    let note = local_twin_note(&objects, "TASK-123", &reqs[1]).expect("TASK-123 has a twin");
    assert!(note.contains("TASK-4120"), "{note}");
    assert!(!note.contains("TASK-HOME-123"), "{note}");
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-036.ac12b260 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-036.ac4dbc90 | ai:claude
#[test]
fn doctor_lint_flags_bare_ids_that_also_name_a_local_spec() {
    let _guard = home();
    let mut reqs = store();
    reqs.push(spec(
        "TASK-900",
        "ruling",
        "See STORY-100 and BUG-030.\nAlso STORY-HOME-100, TASK-1-096 and EPIC-7.",
    ));
    reqs[0].comments.push(aida_core::Comment::new(
        "me".into(),
        "blocked on TASK-123".into(),
    ));
    let lines = ambiguous_bare_id_lines(&reqs);
    assert_eq!(
        lines,
        vec![
            "STORY-100:comment:1: TASK-123 also names local TASK-HOME-123 (bare TASK-123 opens the agreed spec)",
            "TASK-900:description:1: STORY-100 also names local STORY-HOME-100 (bare STORY-100 opens the agreed spec)",
            "TASK-900:description:1: BUG-030 also names local BUG-HOME-030 (bare BUG-030 is not found)",
        ],
    );
    // A twin with an agreed id is named by it (ruling C).
    reqs[4].agreed_id = Some("BUG-4120".into());
    assert_eq!(
        ambiguous_bare_id_lines(&reqs)[2],
        "TASK-900:description:1: BUG-030 also names local BUG-4120 (bare BUG-030 is not found)",
    );
    // A bare id that is the twin's own agreed id opens that very spec, so it
    // is not ambiguous.
    reqs[4].agreed_id = Some("BUG-030".into());
    assert_eq!(ambiguous_bare_id_lines(&reqs).len(), 2);
}
