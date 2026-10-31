//! STORY-1742 scanner negatives over node and registry serialization: every
//! field the registry writers serialize is scanned, with no exemption for
//! the fixed registry paths. A pseudonymous row leaves `hostname` empty: the
//! scanner refuses any non-empty host field.
// trace:BUG-2017 | ai:claude
use super::*;
use crate::node::{AgreedCounters, AgreedIdBlock, BlockRegistry, NodeRegistry, NodeRegistryEntry};

const TOKEN: &str = "private-marker";

fn policy() -> Policy {
    Policy {
        version: 1,
        reviewed: true,
        deny_tokens: vec![TOKEN.into()],
        git_name: "AIDA Generic".into(),
        git_identity: "aida".into(),
    }
}

fn at() -> chrono::DateTime<chrono::Utc> {
    "2026-01-01T00:00:00Z".parse().unwrap()
}

fn node() -> NodeRegistryEntry {
    NodeRegistryEntry {
        id: "0123456789abcdef0123456789abcdef".into(),
        user_id: 1,
        hostname: String::new(),
        email: None,
        clone_path: None,
        name: None,
        user: None,
        registered: at(),
        retired_at: None,
    }
}

fn block() -> AgreedIdBlock {
    AgreedIdBlock {
        node_id: "0123456789abcdef0123456789abcdef".into(),
        owner: "generic".into(),
        hostname: String::new(),
        type_prefix: "FR".into(),
        range_start: 1,
        range_end: 100,
        next: 1,
        allocated_at: at(),
        pool_id: None,
    }
}

fn nodes_toml(entry: NodeRegistryEntry) -> String {
    toml::to_string_pretty(&NodeRegistry { nodes: vec![entry] }).unwrap()
}

fn blocks_yaml(row: AgreedIdBlock) -> String {
    serde_yaml::to_string(&BlockRegistry { blocks: vec![row] }).unwrap()
}

fn counters_toml(prefix: &str, n: u32) -> String {
    let mut c = AgreedCounters::default();
    c.counters.insert(prefix.into(), n);
    toml::to_string_pretty(&c).unwrap()
}

#[test]
fn bug_2017_pseudonymous_registry_serialization_passes_the_scanner() {
    let p = policy();
    p.artifact("registry/nodes.toml", nodes_toml(node()).as_bytes())
        .unwrap();
    p.artifact("registry/blocks.yaml", blocks_yaml(block()).as_bytes())
        .unwrap();
    p.artifact(
        "registry/agreed_counters.toml",
        counters_toml("FR", 42).as_bytes(),
    )
    .unwrap();
}

#[test]
fn bug_2017_every_node_registry_field_is_scanned() {
    let p = policy();
    let cases: Vec<(&str, NodeRegistryEntry)> = vec![
        (
            "id",
            NodeRegistryEntry {
                id: TOKEN.into(),
                ..node()
            },
        ),
        (
            "hostname",
            NodeRegistryEntry {
                hostname: "laptop.example.invalid".into(),
                ..node()
            },
        ),
        (
            "short hostname",
            NodeRegistryEntry {
                hostname: "laptop".into(),
                ..node()
            },
        ),
        (
            "hostname token",
            NodeRegistryEntry {
                hostname: TOKEN.into(),
                ..node()
            },
        ),
        (
            "email",
            NodeRegistryEntry {
                email: Some("person@example.invalid".into()),
                ..node()
            },
        ),
        (
            "clone_path",
            NodeRegistryEntry {
                clone_path: Some(format!("/home/{TOKEN}/clone").into()),
                ..node()
            },
        ),
        (
            "name",
            NodeRegistryEntry {
                name: Some(format!("{TOKEN}-1")),
                ..node()
            },
        ),
        (
            "user",
            NodeRegistryEntry {
                user: Some(TOKEN.into()),
                ..node()
            },
        ),
    ];
    for (field, entry) in cases {
        assert!(
            p.artifact("registry/nodes.toml", nodes_toml(entry).as_bytes())
                .is_err(),
            "nodes.toml {field} was not scanned"
        );
    }
}

#[test]
fn bug_2017_every_block_registry_field_is_scanned() {
    let p = policy();
    let cases: Vec<(&str, AgreedIdBlock)> = vec![
        (
            "node_id",
            AgreedIdBlock {
                node_id: TOKEN.into(),
                ..block()
            },
        ),
        (
            "owner",
            AgreedIdBlock {
                owner: "person@example.invalid".into(),
                ..block()
            },
        ),
        (
            "owner token",
            AgreedIdBlock {
                owner: TOKEN.into(),
                ..block()
            },
        ),
        (
            "hostname",
            AgreedIdBlock {
                hostname: "laptop.example.invalid".into(),
                ..block()
            },
        ),
        (
            "type_prefix",
            AgreedIdBlock {
                type_prefix: TOKEN.to_uppercase(),
                ..block()
            },
        ),
    ];
    for (field, row) in cases {
        assert!(
            p.artifact("registry/blocks.yaml", blocks_yaml(row).as_bytes())
                .is_err(),
            "blocks.yaml {field} was not scanned"
        );
    }
}

#[test]
fn bug_2017_agreed_counter_keys_are_scanned() {
    let p = policy();
    assert!(p
        .artifact(
            "registry/agreed_counters.toml",
            counters_toml(&TOKEN.to_uppercase(), 1).as_bytes()
        )
        .is_err());
}

// Lineage records ride the same scanner: only opaque ids and ranges pass.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
#[test]
fn lineage_records_are_scanned_field_by_field() {
    use crate::allocation_lineage::{PoolRecord, PoolRegistry, RootRecord};
    let p = policy();
    let pool = PoolRecord {
        pool_id: "p-0123456789abcdef".into(),
        type_prefix: "BUG".into(),
        range_start: 1,
        range_end: 1000,
        parent_pool_id: None,
        holder_node: "0123456789abcdef0123456789abcdef".into(),
        home_ref: "origin/aida-store".into(),
    };
    let pools_yaml =
        |row: PoolRecord| serde_yaml::to_string(&PoolRegistry { pools: vec![row] }).unwrap();
    p.artifact("registry/pools.yaml", pools_yaml(pool.clone()).as_bytes())
        .unwrap();
    let root = RootRecord {
        holder_node: "0123456789abcdef0123456789abcdef".into(),
        epoch: 1,
    };
    p.artifact(
        "registry/root.yaml",
        serde_yaml::to_string(&root).unwrap().as_bytes(),
    )
    .unwrap();
    for row in [
        PoolRecord {
            pool_id: TOKEN.into(),
            ..pool.clone()
        },
        PoolRecord {
            holder_node: TOKEN.into(),
            ..pool.clone()
        },
        PoolRecord {
            home_ref: format!("{TOKEN}/aida-store"),
            ..pool.clone()
        },
        PoolRecord {
            parent_pool_id: Some(TOKEN.into()),
            ..pool.clone()
        },
    ] {
        assert!(p
            .artifact("registry/pools.yaml", pools_yaml(row).as_bytes())
            .is_err());
    }
    let tagged = AgreedIdBlock {
        pool_id: Some(TOKEN.into()),
        ..block()
    };
    assert!(p
        .artifact("registry/blocks.yaml", blocks_yaml(tagged).as_bytes())
        .is_err());
    let root = RootRecord {
        holder_node: TOKEN.into(),
        epoch: 1,
    };
    assert!(p
        .artifact(
            "registry/root.yaml",
            serde_yaml::to_string(&root).unwrap().as_bytes()
        )
        .is_err());
}
