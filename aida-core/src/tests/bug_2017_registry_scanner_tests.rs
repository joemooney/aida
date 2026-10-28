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
