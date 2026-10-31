//! Synthetic store coverage and a real handler regression for worker listing.
// trace:BUG-B6B1278890774EF993D2EBC327862470-026 | ai:codex

use super::*;
use std::os::unix::fs::PermissionsExt;

const LOCAL: &str = "11111111111111111111111111111111";
const REMOTE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SPEC: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const POOL: &str = "cccccccccccccccccccccccccccccccc";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn private(path: &Path, text: &str) {
    write(path, text);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

struct Fixture {
    root: tempfile::TempDir,
    store: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        // common_dir deliberately uses filesystem identity, not process Git
        // overrides; no real repository or HOME enrollment is needed.
        std::fs::create_dir(root.path().join(".git")).unwrap();
        let store = root.path().join(".aida-store");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        Self { root, store }
    }

    fn binding(&self) {
        private(
            &self.root.path().join(".git/aida-work-node.toml"),
            &format!("version = 1\nnode_id = \"{LOCAL}\"\n"),
        );
    }

    fn object(&self, filename_id: &str, spec: Option<&str>, agreed: Option<&str>, archived: bool) {
        let mut req =
            aida_core::Requirement::new("fixture".into(), "synthetic node evidence".into());
        req.spec_id = spec.map(str::to_owned);
        req.agreed_id = agreed.map(str::to_owned);
        req.archived = archived;
        let path =
            aida_core::object_store::object_path(&self.store.join("objects"), filename_id).unwrap();
        write(&path, &serde_yaml::to_string(&req).unwrap());
    }

    fn populated() -> Self {
        let fixture = Self::new();
        fixture.binding();
        let mut registry = aida_core::node::NodeRegistry::default();
        registry.register_specific_full_named(
            "1".into(),
            1,
            "legacy-host".into(),
            None,
            Some("/fixtures/legacy".into()),
            None,
            None,
        );
        registry.register_specific_full_named(
            REMOTE.to_ascii_uppercase(),
            2,
            "uuid-host".into(),
            None,
            Some("/fixtures/uuid".into()),
            None,
            None,
        );
        let encoded = toml::to_string(&registry).unwrap();
        let legacy_numeric = encoded.replacen("id = \"1\"", "id = 1", 1);
        assert_ne!(
            encoded, legacy_numeric,
            "fixture must contain a pre-string numeric id"
        );
        write(&fixture.store.join("registry/nodes.toml"), &legacy_numeric);
        let aliases = aida_core::node_alias::NodeAliasMap {
            bindings: vec![
                aida_core::node_alias::NodeAliasBinding {
                    label: "HOME".into(),
                    node: LOCAL.into(),
                    retired: false,
                },
                aida_core::node_alias::NodeAliasBinding {
                    label: "REMOTE".into(),
                    node: REMOTE.into(),
                    retired: false,
                },
                aida_core::node_alias::NodeAliasBinding {
                    label: "OLDNODE".into(),
                    node: REMOTE.into(),
                    retired: true,
                },
            ],
        };
        write(
            &fixture.store.join("registry/node_aliases.toml"),
            &toml::to_string(&aliases).unwrap(),
        );
        let blocks = aida_core::node::BlockRegistry {
            blocks: vec![aida_core::node::AgreedIdBlock {
                node_id: "2".into(),
                owner: "fixture".into(),
                hostname: "block-host".into(),
                type_prefix: "TASK".into(),
                range_start: 100,
                range_end: 199,
                next: 101,
                allocated_at: chrono::Utc::now(),
                pool_id: None,
            }],
        };
        write(
            &fixture.store.join("registry/blocks.yaml"),
            &serde_yaml::to_string(&blocks).unwrap(),
        );
        let pools = aida_core::allocation_lineage::PoolRegistry {
            pools: vec![aida_core::allocation_lineage::PoolRecord {
                pool_id: "fixture-root-pool".into(),
                type_prefix: "BUG".into(),
                range_start: 1,
                range_end: 99,
                parent_pool_id: None,
                holder_node: POOL.into(),
                home_ref: "fixture-home".into(),
            }],
        };
        write(
            &fixture.store.join("registry/pools.yaml"),
            &serde_yaml::to_string(&pools).unwrap(),
        );
        fixture.object(
            &format!("TASK-{SPEC}-001"),
            Some(&format!("TASK-{SPEC}-001")),
            Some("TASK-3-001"),
            true,
        );
        fixture.object("TASK-JM-BOX-002", None, None, false);
        // Two-level (FEAT-TYPE-NNN) and label-scoped IDs share the
        // PREFIX-MIDDLE-SEQ shape; neither middle is a node.
        fixture.object("AUTH-FR-001", Some("AUTH-FR-001"), None, false);
        fixture.object("BUG-HOME-026", Some("BUG-HOME-026"), None, false);
        fixture.object(
            &format!("BUG-{REMOTE}-002"),
            Some(&format!("BUG-{REMOTE}-002")),
            None,
            false,
        );
        // A UUID-shaped filename with a decimal suffix must not manufacture
        // a fake node namespace from its middle UUID components.
        let req = aida_core::Requirement::new("opaque object".into(), "fixture".into());
        write(
            &fixture
                .store
                .join("objects/META/000/aaaaaaaa-bbbb-4ccc-8ddd-000000000123.yaml"),
            &serde_yaml::to_string(&req).unwrap(),
        );
        fixture
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().into(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn node_inventory_unions_every_source_and_keeps_roles_separate() {
    let fixture = Fixture::populated();
    let before = snapshot(fixture.root.path());
    let current = local_node(&fixture.store).unwrap();
    let rows = read_nodes(&fixture.store, current.as_ref(), false).unwrap();
    assert_eq!(rows.len(), 7, "{rows:?}");
    let row = |id: &str| rows.iter().find(|r| r.id.eq_ignore_ascii_case(id)).unwrap();
    assert_eq!(rows.iter().filter(|r| r.current).count(), 1);
    assert!(row(LOCAL).current);
    assert_eq!(
        row(LOCAL).role,
        Some(AllocationRole::Node),
        "HOME is a label, not root authority"
    );
    assert!(row(LOCAL).labels.contains("HOME"));
    assert!(row(REMOTE).labels.contains("REMOTE"));
    assert!(row(REMOTE).labels.contains("OLDNODE (retired)"));
    assert_eq!(row(REMOTE).hostname.as_deref(), Some("uuid-host"));
    assert_eq!(
        row("1").clone_path.as_deref(),
        Some(Path::new("/fixtures/legacy"))
    );
    assert_eq!(row("2").hostname.as_deref(), Some("block-host"));
    assert_eq!(
        row(POOL).role,
        None,
        "shared grants are not clone-local configuration"
    );
    assert!(!row(SPEC).current);
    assert!(!row("3").current);
    for fake in ["JM-BOX", "FR", "HOME"] {
        assert!(
            !rows.iter().any(|r| r.id.eq_ignore_ascii_case(fake)),
            "an unknown non-UUID namespace is not a node: {fake} in {rows:?}"
        );
    }
    assert_eq!(
        before,
        snapshot(fixture.root.path()),
        "listing must not mutate identity or records"
    );
}

#[test]
fn node_inventory_worker_dispatch_lists_other_nodes_without_allocating() {
    let fixture = Fixture::populated();
    let before = snapshot(fixture.root.path());
    let runner = crate::resolve_aida_exe();
    assert_eq!(
        runner
            .parent()
            .and_then(|deps| deps.file_name())
            .and_then(|name| name.to_str()),
        Some("deps"),
        "expected this build's runner under target/<profile>/deps; an ambient AIDA_BIN redirects the resolver"
    );
    let out = std::process::Command::new(runner)
        .args([
            "--ignored",
            "--exact",
            "node_cmd::node_inventory::tests::worker_list_dispatch_child",
            "--nocapture",
        ])
        .env("AIDA_TEST_NODE_LIST_FIXTURE_ROOT", &fixture.store)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains(REMOTE), "{text}");
    assert!(text.contains(POOL), "{text}");
    assert!(text.contains(SPEC), "{text}");
    assert!(text.contains("OLDNODE (retired)"), "{text}");
    let marked: Vec<_> = text.lines().filter(|s| s.starts_with("* ")).collect();
    assert_eq!(marked.len(), 1, "{text}");
    assert!(
        marked[0].contains(LOCAL) && marked[0].contains("work node"),
        "{text}"
    );
    assert!(!text.contains("WORK node"), "{text}");
    assert_eq!(before, snapshot(fixture.root.path()));
}

#[test]
#[ignore = "subprocess helper for the worker-list dispatch regression"]
fn worker_list_dispatch_child() {
    let store = std::env::var_os("AIDA_TEST_NODE_LIST_FIXTURE_ROOT")
        .expect("regression parent must provide its private fixture");
    crate::node_cmd::handle_node_command(&crate::node_cmd::NodeCommand::List, Path::new(&store))
        .unwrap();
}

#[test]
fn node_inventory_legacy_current_root_and_unbound_reads() {
    let fixture = Fixture::new();
    let config = aida_core::NodeConfig {
        node_id: "1".into(),
        user_id: 1,
        hostname: "legacy-local".into(),
        email: None,
        name: None,
        user: None,
        registered_at: chrono::Utc::now(),
    };
    write(
        &fixture.store.join(".aida/node.toml"),
        &toml::to_string(&config).unwrap(),
    );
    let current = local_node(&fixture.store).unwrap();
    let rows = read_nodes(&fixture.store, current.as_ref(), true).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "1");
    assert!(rows[0].current);
    assert_eq!(rows[0].role, Some(AllocationRole::Root));
    assert_eq!(rows[0].hostname.as_deref(), Some("legacy-local"));
    fixture.binding();
    let current = local_node(&fixture.store).unwrap().unwrap();
    assert_eq!(
        current.id, LOCAL,
        "the opaque binding supersedes stale legacy config"
    );
    assert!(
        current.hostname.is_none(),
        "do not borrow another identity's metadata"
    );
    let empty = Fixture::new();
    let before = snapshot(empty.root.path());
    assert!(list(&empty.store, false)
        .unwrap()
        .contains("No known nodes found"));
    assert_eq!(
        before,
        snapshot(empty.root.path()),
        "a read cannot bootstrap a node"
    );
}

#[test]
fn node_inventory_keeps_a_root_record_without_pools_or_registry() {
    let fixture = Fixture::new();
    let record = aida_core::allocation_lineage::RootRecord {
        holder_node: REMOTE.into(),
        epoch: 1,
    };
    write(
        &fixture.store.join("registry/root.yaml"),
        &serde_yaml::to_string(&record).unwrap(),
    );
    let before = snapshot(fixture.root.path());
    let rows = read_nodes(&fixture.store, None, false).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, REMOTE);
    assert!(!rows[0].current);
    assert_eq!(
        rows[0].role, None,
        "a record is not this clone's private role setting"
    );
    assert!(rows[0].recorded_root);
    assert_eq!(rows[0].role_label(), "recorded root");
    assert_eq!(before, snapshot(fixture.root.path()));

    fixture.binding();
    let record = aida_core::allocation_lineage::RootRecord {
        holder_node: LOCAL.into(),
        epoch: 2,
    };
    write(
        &fixture.store.join("registry/root.yaml"),
        &serde_yaml::to_string(&record).unwrap(),
    );
    let current = local_node(&fixture.store).unwrap();
    let rows = read_nodes(&fixture.store, current.as_ref(), false).unwrap();
    assert_eq!(rows[0].role, Some(AllocationRole::Node));
    assert_eq!(
        rows[0].role_label(),
        "work node (recorded root)",
        "report both facts without inferring local authority"
    );
}

#[test]
fn node_inventory_refuses_broken_sources_instead_of_reporting_a_complete_view() {
    for path in [
        "registry/nodes.toml",
        "registry/node_aliases.toml",
        "registry/blocks.yaml",
        "registry/pools.yaml",
        "registry/root.yaml",
        "objects/BUG/000/BUG-1-001.yaml",
    ] {
        let fixture = Fixture::new();
        write(&fixture.store.join(path), "invalid: [");
        assert!(read_nodes(&fixture.store, None, false).is_err(), "{path}");
    }
}

#[test]
fn node_inventory_namespace_and_display_boundaries() {
    let mut nodes = BTreeMap::new();
    assert_eq!(namespace("SEC-2-007", &nodes), Some("2"));
    let spec = format!("BUG-{SPEC}-002");
    assert_eq!(namespace(&spec, &nodes), Some(SPEC));
    for id in [
        "TASK-JM-BOX-002",
        "AUTH-FR-001",
        "GEN-REQ-001",
        "TASK-123",
        "TASK--001",
        "TASK-node-next",
        "aaaaaaaa-bbbb-4ccc-8ddd-000000000123",
    ] {
        assert_eq!(namespace(id, &nodes), None, "{id}");
    }
    node(&mut nodes, "JM-BOX").unwrap();
    assert_eq!(
        namespace("TASK-JM-BOX-002", &nodes),
        Some("JM-BOX"),
        "a registry-known legacy node keeps its namespace"
    );
    let mut nodes = BTreeMap::new();
    let row = node(&mut nodes, LOCAL).unwrap();
    row.labels.insert("label\nnext".into());
    row.hostname = Some("hôte\u{1b}[31m".into());
    row.clone_path = Some("/fixture/line\nbreak".into());
    let text = render(&nodes.into_values().collect::<Vec<_>>(), false);
    assert!(!text.contains('\u{1b}'));
    assert!(text.contains("hôte"));
    assert!(text.contains("label\\nnext"));
    assert!(text.contains("/fixture/line\\nbreak"));
}
