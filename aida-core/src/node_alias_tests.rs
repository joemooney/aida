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

#[test]
fn node_alias_node_id_registration_refuses_current_and_retired_labels() {
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    let dir = tempfile::tempdir().unwrap();
    let mut map = home();
    map.rename("HOME", "DESK").unwrap();
    map.save(dir.path()).unwrap();
    for (id, kind) in [("DESK", "the node label"), ("home", "a retired node label")] {
        let err = refuse_node_id_colliding_with_label(dir.path(), id)
            .unwrap_err()
            .to_string();
        assert!(err.contains(kind) && err.contains(NODE_A), "{err}");
    }
    refuse_node_id_colliding_with_label(dir.path(), "JM2").unwrap();
    // No map file at all: nothing to collide with.
    let empty = tempfile::tempdir().unwrap();
    refuse_node_id_colliding_with_label(empty.path(), "HOME").unwrap();
}

#[test]
fn node_alias_text_parsers_keep_a_stored_literal_id() {
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    let canonical = format!("TASK-{NODE_A}-001").to_ascii_uppercase();
    {
        let _guard = install_for_thread(home());
        assert_eq!(expand_or_keep("TASK-HOME-001"), canonical);
    }
    // A stored `TASK-HOME-001` (e.g. from a node registered as HOME) wins.
    let _guard =
        install_for_thread_with_literals(home(), |id| id.eq_ignore_ascii_case("TASK-HOME-001"));
    assert_eq!(expand_or_keep("TASK-HOME-001"), "TASK-HOME-001");
    assert_eq!(
        expand_or_keep("TASK-HOME-002"),
        canonical.replace("-001", "-002")
    );
    assert_eq!(expand_or_keep("TASK-095"), "TASK-095");
}

#[test]
fn node_alias_stored_spec_ids_cover_spec_id_and_agreed_id() {
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    let dir = tempfile::tempdir().unwrap();
    let objects = dir.path().join("objects");
    let mut req = crate::Requirement::new("t".into(), "b".into());
    req.spec_id = Some("TASK-HOME-001".into());
    req.agreed_id = Some("task-ak-007".into());
    crate::object_store::write_object(&objects, &req).unwrap();
    let ids = stored_spec_ids(&objects);
    assert!(
        // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — numeric keys.
        ids.contains("TASK-HOME-1") && ids.contains("TASK-AK-7"),
        "{ids:?}"
    );
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-036.acc7fcab | ai:claude
#[test]
fn bare_id_local_twins_ignore_leading_zeros_and_agreed_ids() {
    let local = format!("BUG-{}-030", NODE_A.to_ascii_uppercase());
    for bare in ["BUG-030", "BUG-30", "bug-030"] {
        assert!(is_local_twin(bare, &local), "{bare}");
    }
    // The agreed id itself, another type, another number and an alias form
    // are never twins.
    for other in [
        "BUG-30",
        "TASK-B6B1278890774EF993D2EBC327862470-030",
        "BUG-HOME-030",
    ] {
        assert!(!is_local_twin("BUG-30", other), "{other}");
    }
    assert!(!is_local_twin("BUG-31", &local));
    assert!(!is_local_twin("BUG-HOME-030", &local), "not a bare id");
    let known = [
        "STORY-100",
        "STORY-B6B1278890774EF993D2EBC327862470-100",
        "TASK-123",
    ];
    assert_eq!(
        local_twins("STORY-100", known),
        vec!["STORY-B6B1278890774EF993D2EBC327862470-100".to_string()]
    );
    assert!(local_twins("TASK-123", known).is_empty());
    assert_eq!(local_id_key(&local), split_bare_id("BUG-30"));
    for not_local in ["BUG-30", "BUG-HOME-030", "TASK-1-096", "$TASK-1-1"] {
        assert_eq!(local_id_key(not_local), None, "{not_local}");
    }
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-036.Rendering | ai:claude
#[test]
fn bare_id_local_twin_candidates_use_the_allocator_padding() {
    let map = home();
    assert_eq!(
        map.local_twin_candidates("TASK-123"),
        vec!["TASK-B6B1278890774EF993D2EBC327862470-123".to_string()]
    );
    assert_eq!(
        map.local_twin_candidates("bug-30"),
        vec!["BUG-B6B1278890774EF993D2EBC327862470-030".to_string()]
    );
    // Display of a twin is the alias form, never the bare number.
    assert_eq!(
        map.display(&map.local_twin_candidates("STORY-100")[0], false),
        "STORY-HOME-100"
    );
    assert!(map.local_twin_candidates("STORY-HOME-100").is_empty());
    assert!(NodeAliasMap::default()
        .local_twin_candidates("TASK-1")
        .is_empty());
    // A twin is named by its agreed id when it has one, else its alias form.
    let twin = map.local_twin_candidates("STORY-100").remove(0);
    let _guard = install_for_thread(map);
    assert_eq!(local_twin_label(&twin, None), "STORY-HOME-100");
    assert_eq!(local_twin_label(&twin, Some(" ")), "STORY-HOME-100");
    assert_eq!(local_twin_label(&twin, Some("story-4120")), "STORY-4120");
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
#[test]
fn alias_expansion_pads_the_sequence_like_the_allocator() {
    let map = home();
    for alias in ["BUG-HOME-36", "bug-home-036", "BUG-HOME-0036"] {
        assert_eq!(
            map.expand(alias).as_deref(),
            Some("BUG-B6B1278890774EF993D2EBC327862470-036"),
            "{alias}"
        );
    }
    assert_eq!(
        map.expand("TASK-HOME-1234").as_deref(),
        Some("TASK-B6B1278890774EF993D2EBC327862470-1234")
    );
}

#[test]
fn bare_id_tokens_skip_local_alias_and_legacy_node_forms() {
    let text = "See STORY-100, BUG-030.\nnot TASK-1-096 or BUG-HOME-030 or \
                TASK-B6B1278890774EF993D2EBC327862470-123 or X-1; (task-123)";
    assert_eq!(
        bare_id_tokens(text),
        vec![
            (1, "STORY-100".to_string()),
            (1, "BUG-030".to_string()),
            (2, "TASK-123".to_string()),
        ]
    );
}

#[cfg(feature = "native")]
fn store_repo(origin_url: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let ok = crate::test_git::git_command()
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "aida-store"]);
    git(&["config", "user.email", "t@t.t"]);
    git(&["config", "user.name", "t"]);
    std::fs::write(dir.path().join("README.md"), "seed").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "seed"]);
    git(&["remote", "add", "origin", origin_url]);
    dir
}

/// An origin that cannot be reached keeps the committed label and reports
/// it, instead of failing a write that already landed locally.
// trace:BUG-B6B1278890774EF993D2EBC327862470-025 | ai:claude
#[cfg(feature = "native")]
#[test]
fn node_alias_unreachable_origin_keeps_the_commit_and_reports_it() {
    let missing = tempfile::tempdir().unwrap();
    let origin = missing.path().join("no-such-hub.git");
    let dir = store_repo(origin.to_str().unwrap());
    let outcome = mutate_cas(dir.path(), "label node test", |map| map.set("HOME", NODE_A)).unwrap();
    let MutateCasOutcome::Committed { origin, branch, .. } = &outcome else {
        panic!("expected a commit, got {outcome:?}");
    };
    assert_eq!(branch, "aida-store");
    assert!(matches!(origin, OriginPublish::Unreached(_)), "{origin:?}");
    assert_eq!(NodeAliasMap::load(dir.path()), home());
    // A re-run sees the committed label rather than writing it again.
    let again = mutate_cas(dir.path(), "label node test", |map| map.set("HOME", NODE_A)).unwrap();
    assert_eq!(again, MutateCasOutcome::Unchanged);
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-025 | ai:claude
#[cfg(feature = "native")]
#[test]
fn node_alias_reachable_origin_reports_pushed() {
    let hub = tempfile::tempdir().unwrap();
    let ok = crate::test_git::git_command()
        .args(["init", "-q", "--bare"])
        .arg(hub.path())
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let dir = store_repo(hub.path().to_str().unwrap());
    let outcome = mutate_cas(dir.path(), "label node test", |map| map.set("HOME", NODE_A)).unwrap();
    assert!(
        matches!(
            outcome,
            MutateCasOutcome::Committed {
                origin: OriginPublish::Pushed,
                ..
            }
        ),
        "{outcome:?}"
    );
}
