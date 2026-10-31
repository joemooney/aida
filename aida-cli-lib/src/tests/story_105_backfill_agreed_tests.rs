//! `aida doctor --fix agreed-ids`: the plan continues each type's global
//! sequence; dry run writes nothing; apply sets agreed IDs, raises the
//! ledger, keeps local IDs resolving, and a re-run is a no-op.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude

use super::*;

const NODE: &str = "B6B1278890774EF993D2EBC327862470";

fn plant(objects: &Path, spec_id: &str, agreed: Option<&str>) -> Requirement {
    let mut r = Requirement::new(format!("spec {spec_id}"), String::new());
    r.spec_id = Some(spec_id.into());
    r.agreed_id = agreed.map(Into::into);
    r.req_type = aida_core::RequirementType::Story;
    aida_core::object_store::write_object(objects, &r).unwrap();
    r
}

#[test]
fn local_parts_accept_hex_and_registered_nodes_only() {
    let nodes: HashSet<String> = ["1".to_string()].into();
    assert_eq!(
        local_parts(&format!("STORY-{NODE}-007"), &nodes),
        Some(("STORY".into(), 7))
    );
    assert_eq!(local_parts("BUG-1-012", &nodes), Some(("BUG".into(), 12)));
    assert_eq!(local_parts("FR-AUTH-001", &nodes), None, "feature prefix");
    assert_eq!(local_parts("STORY-105", &nodes), None, "flat");
    assert_eq!(local_parts("$USER-1-001", &nodes), None, "meta");
}

#[test]
fn floors_take_specs_blocks_spent_shared_blocks_and_counters() {
    let mut a = Requirement::new("a".into(), String::new());
    a.spec_id = Some("STORY-40".into());
    let mut b = Requirement::new("b".into(), String::new());
    b.spec_id = Some(format!("STORY-{NODE}-001"));
    b.agreed_id = Some("STORY-0055".into());
    let mut blocks = aida_core::BlockRegistry::default();
    let block = |typ: &str, start, end, next| aida_core::node::AgreedIdBlock {
        node_id: "n".into(),
        owner: String::new(),
        hostname: String::new(),
        type_prefix: typ.into(),
        range_start: start,
        range_end: end,
        next,
        allocated_at: chrono::Utc::now(),
        pool_id: None,
    };
    blocks.blocks.push(block("BUG", 1, 300, 10));
    // An unspent shared block reserves nothing; its spent part does.
    blocks.blocks.push(block("*", 1001, 2000, 1001));
    let mut counters = aida_core::node::AgreedCounters::default();
    counters.counters.insert("TASK".into(), 9);
    let f = type_floors(&[a.clone(), b.clone()], &blocks, &counters);
    assert_eq!(f["STORY"], 55);
    assert_eq!(f["BUG"], 300);
    assert_eq!(f["TASK"], 9);
    blocks.blocks.push(block("*", 2001, 3000, 2011));
    let f = type_floors(&[a, b], &blocks, &counters);
    assert_eq!(f["STORY"], 2010);
}

// trace:STORY-B6B1278890774EF993D2EBC327862470-105.ace15a08 | ai:claude
#[test]
fn backfill_agreed_ids_end_to_end() {
    use aida_core::DatabaseBackend;
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join(".aida-store");
    let backend = crate::test_env::home_fixture_backend(&store).unwrap();
    backend.save(&aida_core::RequirementsStore::new()).unwrap();
    for (k, v) in [("user.email", "test@example.com"), ("user.name", "Test")] {
        let ok = crate::test_git::git_command()
            .args(["config", k, v])
            .current_dir(&store)
            .status()
            .unwrap();
        assert!(ok.success());
    }
    let objects = store.join("objects");
    plant(&objects, "STORY-0010", None);
    plant(&objects, "STORY-12", None);
    // Already has an agreed ID: untouched, but its number raises the floor.
    plant(&objects, &format!("STORY-{NODE}-001"), Some("STORY-20"));
    let second = plant(&objects, &format!("STORY-{NODE}-003"), None);
    let first = plant(&objects, &format!("STORY-{NODE}-002"), None);
    // A flat STORY-22 raises the floor past the agreed STORY-20.
    plant(&objects, "STORY-22", None);
    let before = aida_core::object_store::load_all_objects(&objects).unwrap();

    // Apply without --yes or --dry-run refuses.
    assert!(run(&store, &BackfillOptions::default()).is_err());

    // Dry run: mapping saved, store unchanged.
    let mapping = tmp.path().join("map.tsv");
    run(
        &store,
        &BackfillOptions {
            dry_run: true,
            yes: false,
            mapping: Some(mapping.clone()),
        },
    )
    .unwrap();
    let text = std::fs::read_to_string(&mapping).unwrap();
    assert!(
        text.contains(&format!("STORY-{NODE}-002\tSTORY-23\t{}", first.id)),
        "{text}"
    );
    assert!(
        text.contains(&format!("STORY-{NODE}-003\tSTORY-24\t{}", second.id)),
        "{text}"
    );
    let unchanged = aida_core::object_store::load_all_objects(&objects).unwrap();
    assert_eq!(
        unchanged.iter().filter(|r| r.agreed_id.is_some()).count(),
        before.iter().filter(|r| r.agreed_id.is_some()).count()
    );
    assert!(!store.join("registry/agreed_counters.toml").exists());

    // Apply.
    run(
        &store,
        &BackfillOptions {
            dry_run: false,
            yes: true,
            mapping: None,
        },
    )
    .unwrap();
    let after = aida_core::object_store::load_all_objects(&objects).unwrap();
    let got = |uuid| after.iter().find(|r| r.id == uuid).unwrap();
    assert_eq!(got(first.id).agreed_id.as_deref(), Some("STORY-23"));
    assert_eq!(got(second.id).agreed_id.as_deref(), Some("STORY-24"));
    assert_eq!(
        got(first.id).spec_id.as_deref(),
        Some(format!("STORY-{NODE}-002").as_str()),
        "local id unchanged"
    );
    assert_eq!(
        got(first.id).history.last().map(|h| h.author.as_str()),
        Some(BACKFILL_AUTHOR)
    );
    let counters = load_counters(&store).unwrap();
    assert_eq!(counters.counters["STORY"], 24);
    let blocks = aida_core::BlockRegistry::load(&store.join("registry/blocks.yaml")).unwrap();
    let ledger = blocks.blocks.last().unwrap();
    assert_eq!(
        (ledger.range_start, ledger.range_end, ledger.next),
        (23, 24, 25)
    );
    assert!(ledger.is_exhausted());

    // Both the local and the agreed id resolve to the spec.
    let fresh = aida_core::GitBackend::new(&store).unwrap();
    for id in [format!("STORY-{NODE}-002"), "STORY-23".to_string()] {
        let hit = fresh.get_requirement_by_spec_id(&id).unwrap().unwrap();
        assert_eq!(hit.id, first.id, "{id}");
    }

    // Re-run: nothing to do; nothing changes.
    let (_, rows) = load_plan(&store).unwrap();
    assert!(rows.is_empty());
    run(
        &store,
        &BackfillOptions {
            dry_run: false,
            yes: true,
            mapping: None,
        },
    )
    .unwrap();
    let blocks2 = aida_core::BlockRegistry::load(&store.join("registry/blocks.yaml")).unwrap();
    assert_eq!(blocks2.blocks.len(), blocks.blocks.len());
}

#[test]
fn doctor_fix_agreed_ids_parses() {
    let cli = crate::command_registry::try_parse_from([
        "aida",
        "doctor",
        "--fix",
        "agreed-ids",
        "--dry-run",
    ])
    .unwrap();
    let crate::doctor_cmd::DoctorArgs { fix, dry_run, .. } =
        cli.command.registered::<crate::doctor_cmd::DoctorArgs>()
    else {
        panic!("expected doctor command");
    };
    assert_eq!(fix.as_deref(), Some("agreed-ids"));
    assert!(dry_run);
}
