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
    let (_, rerun) = load_plan(&store).unwrap();
    assert!(rerun.is_empty(), "{rerun:?}");
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

/// Review item 4: an unparsable `blocks.yaml` refuses both the dry run and
/// the apply, and the ledger file is left byte-for-byte unchanged (never
/// replaced by a registry holding only the new spent blocks).
#[test]
fn story_105_corrupt_blocks_yaml_refuses_and_leaves_the_ledger() {
    use aida_core::DatabaseBackend;
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join(".aida-store");
    let backend = crate::test_env::home_fixture_backend(&store).unwrap();
    backend.save(&aida_core::RequirementsStore::new()).unwrap();
    let objects = store.join("objects");
    let pending = plant(&objects, &format!("STORY-{NODE}-001"), None);
    std::fs::create_dir_all(store.join("registry")).unwrap();
    let blocks = store.join("registry/blocks.yaml");
    let corrupt = "blocks:\n- node_id: [unterminated\n  range_start: nope\n";
    std::fs::write(&blocks, corrupt).unwrap();

    let dry = run(
        &store,
        &BackfillOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(format!("{dry:#}").contains("blocks.yaml"), "{dry:#}");
    let apply = run(
        &store,
        &BackfillOptions {
            yes: true,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(format!("{apply:#}").contains("refusing"), "{apply:#}");

    assert_eq!(std::fs::read_to_string(&blocks).unwrap(), corrupt);
    assert!(!store.join("registry/agreed_counters.toml").exists());
    let after = aida_core::object_store::load_all_objects(&objects).unwrap();
    let row = after.iter().find(|r| r.id == pending.id).unwrap();
    assert_eq!(row.agreed_id, None, "no spec was written");
}

/// Review item 5: the legacy node's specs get the bound node's next free
/// local IDs per type (above the store's highest and the bound counter),
/// in old-number order, skipping a number already taken.
#[test]
fn story_105_retirement_renames_legacy_node_specs_into_the_bound_node() {
    let bound = NODE.to_ascii_lowercase();
    let mut reqs = Vec::new();
    let mut mk = |sid: &str, agreed: Option<&str>| {
        let mut r = Requirement::new(sid.into(), String::new());
        r.spec_id = Some(sid.into());
        r.agreed_id = agreed.map(Into::into);
        reqs.push(r.clone());
        r
    };
    let t2 = mk("TASK-1-0002", Some("TASK-900"));
    let t1 = mk("TASK-1-0001", Some("TASK-899"));
    let faq = mk("FAQ-1-0007", None);
    mk(&format!("TASK-{bound}-0003"), Some("TASK-12"));
    // The bound node's TASK-…-0005 is taken (a padded twin counts too).
    mk(&format!("TASK-{bound}-5"), None);
    mk("STORY-3-0001", None); // another live node: untouched
    let retire = Retirement {
        old_node: "1".into(),
        new_node: bound.clone(),
    };
    let counter: BTreeMap<String, u64> = [("TASK".to_string(), 4)].into();
    let rows = plan_retirement(&reqs, &retire, &counter, 3);
    let got: Vec<(&str, &str, Uuid)> = rows
        .iter()
        .map(|r| (r.old.as_str(), r.new.as_str(), r.uuid))
        .collect();
    assert_eq!(
        got,
        vec![
            ("FAQ-1-0007", format!("FAQ-{bound}-001").as_str(), faq.id),
            ("TASK-1-0001", format!("TASK-{bound}-006").as_str(), t1.id),
            ("TASK-1-0002", format!("TASK-{bound}-007").as_str(), t2.id),
        ]
    );
    let mut plan = Plan::default();
    plan.renames = rows;
    let text = render_mapping(&plan);
    assert!(
        text.contains("# old_local_id\tnew_local_id\tuuid"),
        "{text}"
    );
    assert!(
        text.contains(&format!("TASK-1-0001\tTASK-{bound}-006")),
        "{text}"
    );
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
