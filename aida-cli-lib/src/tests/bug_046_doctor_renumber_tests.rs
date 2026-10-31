//! `aida doctor --fix id-collisions`: unit tests for the rewrite rules and an
//! end-to-end fixture run of the whole repair (plan + mapping, tree-only
//! rewrite, store apply, zero groups after).
// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude

use super::*;

#[test]
fn tokens_respect_id_boundaries_and_case() {
    let t = "see TASK-42, not TASK-420 or BUG-TASK-42 or TASK-42-2; (TASK-42) [l](task-42.md)";
    assert_eq!(find_id_tokens(t, "TASK-42").len(), 2);
    let (out, n) = replace_id_tokens(t, "TASK-42", "TASK-HOME-007");
    assert_eq!(n, 2);
    assert!(out.starts_with("see TASK-HOME-007, not TASK-420"));
    // A lowercase file name in a link is a path, not an ID mention: kept.
    assert!(out.ends_with("(TASK-HOME-007) [l](task-42.md)"), "{out}");
    // The padded and unpadded literals are distinct text.
    assert!(find_id_tokens("TASK-0042", "TASK-42").is_empty());
}

fn member(literal: &str, commits: usize, refs: usize) -> Member {
    Member {
        uuid: Uuid::new_v4(),
        literal: literal.into(),
        title: String::new(),
        commit_shas: (0..commits).map(|i| format!("sha{i}")).collect(),
        other_refs: refs,
    }
}

#[test]
fn survivor_has_most_commit_refs_then_refs_then_unpadded() {
    let m = [member("TASK-0042", 3, 0), member("TASK-42", 1, 50)];
    assert_eq!(choose_survivor(&m), 0);
    let m = [member("TASK-0042", 1, 9), member("TASK-42", 1, 2)];
    assert_eq!(choose_survivor(&m), 0);
    let m = [member("TASK-0042", 0, 0), member("TASK-42", 0, 0)];
    assert_eq!(choose_survivor(&m), 1, "tie keeps the unpadded side");
}

#[test]
fn tracked_rewrite_touches_docs_and_comments_only() {
    let map = vec![("TASK-0042".to_string(), "TASK-HOME-001".to_string())];
    let rs = "//! TASK-0042 smallest-first slice\n\
              let fixture = \"TASK-0042\"; // was TASK-0042\n\
              // trace:TASK-0042 | ai:claude\n\
              # TASK-0042 in a shell comment\n";
    let (out, n) = rewrite_tracked_text("src/x.rs", rs, &map);
    assert_eq!(n, 4, "{out}");
    assert!(out.contains("let fixture = \"TASK-0042\"; // was TASK-HOME-001"));
    assert!(out.starts_with("//! TASK-HOME-001 smallest-first slice"));
    assert!(out.contains("trace:TASK-HOME-001"));
    let md = "- fixed TASK-0042, see [it](task-0042.md)\n";
    let (out, n) = rewrite_tracked_text("CHANGELOG.md", md, &map);
    assert_eq!(n, 1);
    assert_eq!(out, "- fixed TASK-HOME-001, see [it](task-0042.md)\n");
}

#[test]
fn mapping_round_trips_and_rejects_garbage() {
    let rows = vec![MappingRow {
        old: "TASK-0042".into(),
        new: "TASK-HOME-101".into(),
        uuid: Uuid::new_v4(),
    }];
    assert_eq!(parse_mapping(&render_mapping(&rows)).unwrap(), rows);
    assert!(parse_mapping("TASK-1 TASK-2\n").is_err());
    assert!(parse_mapping("TASK-1 TASK-2 not-a-uuid\n").is_err());
}

#[test]
fn mapping_problems_name_a_taken_target_and_a_moved_source() {
    let mut a = Requirement::new("a".into(), String::new());
    a.spec_id = Some("TASK-0042".into());
    let reqs = vec![a.clone()];
    let held = Uuid::new_v4();
    let holder = |id: &str| Ok(aida_core::id_identity::same_id(id, "TASK-77").then_some(held));
    let ok = MappingRow {
        old: "TASK-0042".into(),
        new: "TASK-78".into(),
        uuid: a.id,
    };
    assert!(mapping_problems(&reqs, &[ok.clone()], &holder)
        .unwrap()
        .is_empty());
    let taken = MappingRow {
        new: "TASK-077".into(),
        ..ok.clone()
    };
    let p = mapping_problems(&reqs, &[taken], &holder).unwrap();
    assert!(p[0].contains("already used"), "{p:?}");
    let moved = MappingRow {
        old: "TASK-0043".into(),
        ..ok
    };
    let p = mapping_problems(&reqs, &[moved], &holder).unwrap();
    assert!(p[0].contains("now filed as TASK-0042"), "{p:?}");
}

#[test]
fn build_rewrites_renames_records_history_and_rewrites_referrers() {
    let mut a = Requirement::new("padded".into(), "dup of TASK-42".into());
    a.spec_id = Some("TASK-0042".into());
    let mut b = Requirement::new("plain".into(), String::new());
    b.spec_id = Some("TASK-42".into());
    let mut c = Requirement::new("referrer".into(), "see TASK-0042".into());
    c.spec_id = Some("TASK-9".into());
    let reqs = vec![a.clone(), b, c];
    let mut mapping = BTreeMap::new();
    mapping.insert(
        a.id,
        (
            "TASK-0042".to_string(),
            "TASK-HOME-001".to_string(),
            vec!["abc".to_string()],
        ),
    );
    let (renames, referrers) = build_rewrites(&reqs, &mapping);
    assert_eq!(renames.len(), 1);
    let (old, row) = &renames[0];
    assert_eq!(old, "TASK-0042");
    assert_eq!(row.spec_id.as_deref(), Some("TASK-HOME-001"));
    let h = row.history.last().unwrap();
    assert_eq!(h.author, RENUMBER_AUTHOR);
    assert_eq!(h.changes[0].old_value, "TASK-0042");
    assert!(h.changes[1].new_value.contains("abc"));
    // The unpadded literal in a's own text is a different literal: kept.
    assert_eq!(row.description, "dup of TASK-42");
    assert_eq!(referrers.len(), 1);
    assert_eq!(referrers[0].description, "see TASK-HOME-001");
}

fn git(repo: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn plant(objects: &Path, spec_id: &str, title: &str, description: &str) -> Requirement {
    let mut r = Requirement::new(title.into(), description.into());
    r.spec_id = Some(spec_id.into());
    r.req_type = aida_core::RequirementType::Task;
    aida_core::object_store::write_object(objects, &r).unwrap();
    r
}

/// AC3/AC4 end to end: a project whose store holds the padded twin
/// `TASK-0042` beside `TASK-42`. The dry run saves a mapping and writes
/// nothing; the tree-only half rewrites comments, the changelog and docs
/// (but not code literals or lowercase links); the store apply renumbers the
/// padded twin (same UUID, history recorded, referrer rewritten) and leaves
/// zero collision groups.
#[test]
fn fix_id_collisions_end_to_end() {
    use aida_core::DatabaseBackend;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    git(&root, &["init", "-q", "--initial-branch=main"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "// trace:TASK-0042 | ai:claude\nconst FIXTURE: &str = \"TASK-0042\";\n",
    )
    .unwrap();
    std::fs::write(
        root.join("CHANGELOG.md"),
        // The plain twin has more references (4 here vs 3 for the padded
        // one: trace comment, changelog, store referrer), so the selection
        // rule renumbers the padded twin.
        "- TASK-0042 padded fix\n- TASK-42 plain fix\n- TASK-42 again, see [n](task-0042.md)\n\
         - TASK-42 follow-up\n- TASK-42 cleanup\n",
    )
    .unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "init"]);

    let store = root.join(".aida-store");
    let backend = crate::test_env::home_fixture_backend(&store).unwrap();
    backend.save(&aida_core::RequirementsStore::new()).unwrap();
    let objects = store.join("objects");
    let padded = plant(&objects, "TASK-0042", "padded twin", "");
    let plain = plant(&objects, "TASK-42", "plain", "");
    let referrer = plant(&objects, "TASK-9", "referrer", "blocked on TASK-0042");
    let before = aida_core::object_store::load_all_objects(&objects).unwrap();
    assert_eq!(collision_groups(&before).len(), 1);

    let mapping = root.join("mapping.tsv");
    let opts = |dry_run, yes, tree_only| FixOptions {
        dry_run,
        yes,
        mapping: Some(mapping.clone()),
        tree_only,
    };
    // Applying without --yes or --dry-run refuses.
    assert!(run(
        &root,
        &store,
        &FixOptions {
            mapping: None,
            ..opts(false, false, false)
        }
    )
    .is_err());

    // 1. Dry run: the mapping is saved; nothing else changes.
    run(&root, &store, &opts(true, false, false)).unwrap();
    let rows = parse_mapping(&std::fs::read_to_string(&mapping).unwrap()).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        (rows[0].old.as_str(), rows[0].uuid),
        ("TASK-0042", padded.id)
    );
    let new = rows[0].new.clone();
    assert!(!aida_core::id_identity::same_id(&new, "TASK-42"));
    assert_eq!(
        aida_core::object_store::load_all_objects(&objects)
            .unwrap()
            .len(),
        before.len()
    );
    assert!(aida_core::object_store::object_exists(&objects, "TASK-0042").unwrap());
    assert!(std::fs::read_to_string(root.join("src/lib.rs"))
        .unwrap()
        .contains("trace:TASK-0042"));

    // 2. Tree-only: comments, changelog and docs follow the mapping; code
    //    literals and lowercase links stay; the store is untouched.
    run(&root, &store, &opts(false, false, true)).unwrap();
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(lib.contains(&format!("trace:{new} |")), "{lib}");
    assert!(lib.contains("\"TASK-0042\""), "{lib}");
    let log = std::fs::read_to_string(root.join("CHANGELOG.md")).unwrap();
    assert!(log.starts_with(&format!("- {new} padded fix\n- TASK-42 plain fix")));
    assert!(log.contains("[n](task-0042.md)"), "{log}");
    assert!(aida_core::object_store::object_exists(&objects, "TASK-0042").unwrap());

    // 3. Store apply with the same mapping.
    run(&root, &store, &opts(false, true, false)).unwrap();
    let after = aida_core::object_store::load_all_objects(&objects).unwrap();
    assert!(
        collision_groups(&after).is_empty(),
        "doctor must report zero groups"
    );
    assert!(!aida_core::object_store::object_exists(&objects, "TASK-0042").unwrap());
    let moved = after.iter().find(|r| r.id == padded.id).unwrap();
    assert_eq!(moved.spec_id.as_deref(), Some(new.as_str()));
    let h = moved.history.last().unwrap();
    assert_eq!(h.author, RENUMBER_AUTHOR);
    assert_eq!(
        (
            h.changes[0].old_value.as_str(),
            h.changes[0].new_value.as_str()
        ),
        ("TASK-0042", new.as_str())
    );
    let kept = after.iter().find(|r| r.id == plain.id).unwrap();
    assert_eq!(kept.spec_id.as_deref(), Some("TASK-42"));
    let rewritten = after.iter().find(|r| r.id == referrer.id).unwrap();
    assert_eq!(rewritten.description, format!("blocked on {new}"));
    // Every padding of the kept number resolves to the survivor now.
    for id in ["TASK-42", "TASK-0042", "TASK-042"] {
        assert_eq!(
            backend.get_requirement_by_spec_id(id).unwrap().unwrap().id,
            plain.id,
            "{id}"
        );
    }
    // A re-run finds nothing to do.
    run(
        &root,
        &store,
        &FixOptions {
            mapping: None,
            ..opts(true, false, false)
        },
    )
    .unwrap();
}
