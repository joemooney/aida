// trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
use super::*;

const NODE_A: &str = "b6b1278890774ef993d2ebc327862470";
const NODE_B: &str = "0123456789abcdef0123456789abcdef";

fn home() -> NodeAliasMap {
    let mut map = NodeAliasMap::default();
    assert!(map.set("HOME", NODE_A).unwrap());
    map
}

#[test]
fn node_alias_label_validation() {
    assert_eq!(validate_label("home").unwrap(), "HOME");
    assert_eq!(validate_label("W2").unwrap(), "W2");
    for bad in [
        "H",
        "TOOLONGLABEL",
        "2HOME",
        "HO-ME",
        "a@b",
        "a.b",
        "",
        "CAFE",
        "BEEF12",
    ] {
        assert!(validate_label(bad).is_err(), "{bad} should be refused");
    }
    for prefix in ["TASK", "BUG", "STORY", "EPIC", "FR", "ADR", "SPEC", "PR"] {
        assert!(validate_label(prefix).is_err(), "{prefix} is a type prefix");
    }
}

#[test]
fn node_alias_rebind_to_another_node_refuses_naming_both() {
    let mut map = home();
    assert!(!map.set("home", NODE_A).unwrap(), "re-set is a no-op");
    let err = map.set("HOME", NODE_B).unwrap_err();
    assert!(err.contains(NODE_A) && err.contains(NODE_B), "{err}");
    // One current label per node: a second set is a rename, not a set.
    let err = map.set("LAPTOP", NODE_A).unwrap_err();
    assert!(err.contains("rename"), "{err}");
}

#[test]
fn node_alias_rename_retires_old_label_forever() {
    let mut map = home();
    assert!(map.rename("HOME", "DESK").unwrap());
    assert_eq!(map.current_label(NODE_A), Some("DESK"));
    // The retired label still resolves for input...
    assert_eq!(map.node_for_label("HOME"), Some(NODE_A));
    assert_eq!(
        map.expand("task-home-095").as_deref(),
        Some("TASK-B6B1278890774EF993D2EBC327862470-095")
    );
    // ...is never displayed...
    assert_eq!(
        map.display("TASK-B6B1278890774EF993D2EBC327862470-095", false),
        "TASK-DESK-095"
    );
    // ...and can never be bound to another node.
    let err = map.set("HOME", NODE_B).unwrap_err();
    assert!(err.contains("retired") && err.contains(NODE_A), "{err}");
    let mut other = map.clone();
    other.set("WORKBOX", NODE_B).unwrap();
    assert!(other.rename("WORKBOX", "HOME").is_err());
    // The same node may take its retired label back.
    assert!(map.rename("DESK", "HOME").unwrap());
    assert_eq!(map.current_label(NODE_A), Some("HOME"));
    assert_eq!(map.node_for_label("DESK"), Some(NODE_A));
}

#[test]
fn node_alias_round_trip_and_case() {
    let map = home();
    let canonical = "TASK-B6B1278890774EF993D2EBC327862470-095";
    let lower = "task-b6b1278890774ef993d2ebc327862470-095";
    let shown = map.display(lower, false);
    assert_eq!(shown, "TASK-HOME-095");
    assert_eq!(map.expand(&shown).as_deref(), Some(canonical));
    // Canonical display is upper-case on every surface.
    assert_eq!(map.display(lower, true), canonical);
    // A node with no alias displays canonically (upper-cased).
    let other = format!("BUG-{NODE_B}-001");
    assert_eq!(map.display(&other, false), other.to_ascii_uppercase());
    // Short / agreed / feature IDs pass through untouched.
    for id in ["TASK-095", "FR-1-042", "BUG-JM-001", "PR-12"] {
        assert_eq!(map.display(id, false), id);
        assert_eq!(map.expand(id), None);
    }
    // Unknown label never guesses.
    assert_eq!(map.expand("TASK-NOPE-001"), None);
}

#[test]
fn node_alias_malformed_file_reads_empty_and_refuses_write() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("registry")).unwrap();
    std::fs::write(dir.path().join(NODE_ALIASES_TOML_REL), "not = [valid").unwrap();
    assert_eq!(NodeAliasMap::load(dir.path()), NodeAliasMap::default());
    assert!(NodeAliasMap::load_strict(dir.path()).is_err());

    home().save(dir.path()).unwrap();
    assert_eq!(NodeAliasMap::load(dir.path()), home());
}

#[test]
fn node_alias_registered_node_id_guard_covers_set_and_rename() {
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("registry")).unwrap();
    std::fs::write(
        dir.path().join("registry/nodes.toml"),
        "[[nodes]]\nid = \"w2\"\nuser_id = 1\nhostname = \"h\"\nregistered = \"2026-01-01T00:00:00Z\"\n",
    )
    .unwrap();
    let before = home();
    // Rename onto a registered node id refuses.
    let mut renamed = before.clone();
    renamed.rename("HOME", "W2").unwrap();
    let err = refuse_colliding_labels(dir.path(), &before, &renamed).unwrap_err();
    assert!(err.to_string().contains("W2"), "{err}");
    // Set onto a registered node id refuses.
    let mut set = NodeAliasMap::default();
    set.set("W2", NODE_B).unwrap();
    assert!(refuse_colliding_labels(dir.path(), &NodeAliasMap::default(), &set).is_err());
    // An unrelated new label passes; bindings that already existed are not re-judged.
    let mut ok = before.clone();
    ok.rename("HOME", "DESK").unwrap();
    refuse_colliding_labels(dir.path(), &before, &ok).unwrap();
    refuse_colliding_labels(dir.path(), &set, &set).unwrap();
}

#[test]
fn node_alias_stored_id_guard_covers_spec_id_and_agreed_id() {
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    let dir = tempfile::tempdir().unwrap();
    let objects = dir.path().join("objects");
    let mut literal = crate::Requirement::new("t".into(), "b".into());
    literal.spec_id = Some("BUG-JM-001".into());
    crate::object_store::write_object(&objects, &literal).unwrap();
    let mut agreed = crate::Requirement::new("t".into(), "b".into());
    agreed.spec_id = Some(format!("TASK-{NODE_B}-001").to_ascii_uppercase());
    agreed.agreed_id = Some("task-ak-007".into());
    crate::object_store::write_object(&objects, &agreed).unwrap();

    let before = home();
    for (label, example) in [("JM", "BUG-JM-001"), ("ak", "TASK-AK-007")] {
        let mut set = NodeAliasMap::default();
        set.set(label, NODE_B).unwrap();
        let err = refuse_colliding_labels(dir.path(), &NodeAliasMap::default(), &set)
            .unwrap_err()
            .to_string();
        assert!(err.contains(example), "{err}");
        let mut renamed = before.clone();
        renamed.rename("HOME", label).unwrap();
        let err = refuse_colliding_labels(dir.path(), &before, &renamed)
            .unwrap_err()
            .to_string();
        assert!(err.contains(example), "{err}");
    }
    // A label that is no stored id's middle segment passes.
    let mut ok = before.clone();
    ok.rename("HOME", "DESK").unwrap();
    refuse_colliding_labels(dir.path(), &before, &ok).unwrap();
}
