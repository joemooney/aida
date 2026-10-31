//! Read-only union of node identity evidence for `aida node list`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-026 | ai:codex

use aida_core::allocation_lineage::AllocationRole;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct Node {
    id: String,
    labels: BTreeSet<String>,
    hostname: Option<String>,
    clone_path: Option<PathBuf>,
    role: Option<AllocationRole>,
    recorded_root: bool,
    current: bool,
}

struct LocalNode {
    id: String,
    hostname: Option<String>,
}

// UUID spellings normalize; legacy IDs retain their display spelling. Store
// spec IDs are case-insensitive, so evidence joins on a case-insensitive key.
fn node<'a>(nodes: &'a mut BTreeMap<String, Node>, raw: &str) -> Result<&'a mut Node> {
    let id = aida_core::node_alias::normalize_node(raw).unwrap_or_else(|_| raw.to_owned());
    aida_core::node::validate_node_id(&id).map_err(anyhow::Error::msg)?;
    Ok(nodes
        .entry(id.to_ascii_lowercase())
        .or_insert_with(|| Node {
            id,
            labels: BTreeSet::new(),
            hostname: None,
            clone_path: None,
            role: None,
            recorded_root: false,
            current: false,
        }))
}

// A spec ID's middle segment is only node evidence when it is shaped like a
// node ID (UUID or numeric) or already names a node another source recorded.
// Two-level IDs (FEAT-TYPE-NNN, e.g. AUTH-FR-001) and label-scoped IDs share
// the PREFIX-MIDDLE-SEQ shape, so a merely valid middle is not a node.
fn namespace<'a>(id: &'a str, known: &BTreeMap<String, Node>) -> Option<&'a str> {
    // A bare object UUID is not a spec namespace, even if its final UUID
    // component happens to be decimal and fits a sequence number.
    if uuid::Uuid::parse_str(id).is_ok() || !aida_core::object_store::valid_spec_id_format(id) {
        return None;
    }
    let (_, rest) = id.split_once('-')?;
    let (middle, _) = rest.rsplit_once('-')?;
    let normalized = aida_core::node_alias::normalize_node(middle);
    let numeric = middle.bytes().all(|b| b.is_ascii_digit());
    let id = normalized.as_deref().unwrap_or(middle);
    aida_core::node::validate_node_id(id).ok()?;
    (normalized.is_ok() || numeric || known.contains_key(&id.to_ascii_lowercase()))
        .then_some(middle)
}

fn local_node(store: &Path) -> Result<Option<LocalNode>> {
    if let Some(binding) = aida_core::work_binding::load(store)? {
        return Ok(Some(LocalNode {
            id: binding.node_id,
            hostname: None,
        }));
    }
    let path = store.join(".aida/node.toml");
    if !path.try_exists()? {
        return Ok(None);
    }
    let config = aida_core::NodeConfig::load(&path)?;
    Ok(Some(LocalNode {
        id: config.node_id,
        hostname: (!config.hostname.is_empty()).then_some(config.hostname),
    }))
}

fn read_nodes(store: &Path, local: Option<&LocalNode>, authoritative: bool) -> Result<Vec<Node>> {
    let mut nodes = BTreeMap::new();
    // The registry loaders treat a failed exists() probe as an absent file;
    // try_exists() surfaces an unreadable path instead of an incomplete view.
    let registry = store.join("registry/nodes.toml");
    registry.try_exists()?;
    for entry in aida_core::node::NodeRegistry::load(&registry)?.nodes {
        let row = node(&mut nodes, &entry.id).context("invalid registered node id")?;
        row.hostname = (!entry.hostname.is_empty()).then_some(entry.hostname);
        row.clone_path = entry.clone_path.filter(|p| !p.as_os_str().is_empty());
    }
    for binding in aida_core::node_alias::NodeAliasMap::load_strict(store)?.bindings {
        let id =
            aida_core::node_alias::normalize_node(&binding.node).map_err(anyhow::Error::msg)?;
        let label = if binding.retired {
            format!("{} (retired)", binding.label)
        } else {
            binding.label
        };
        node(&mut nodes, &id)?.labels.insert(label);
    }
    let blocks = store.join("registry/blocks.yaml");
    blocks.try_exists()?; // see registry above
    for block in aida_core::node::BlockRegistry::load(&blocks)?.blocks {
        let row = node(&mut nodes, &block.node_id).context("invalid block node id")?;
        if row.hostname.is_none() && !block.hostname.is_empty() {
            row.hostname = Some(block.hostname);
        }
    }
    for pool in aida_core::allocation_lineage::PoolRegistry::load(store)?.pools {
        node(&mut nodes, &pool.holder_node).context("invalid pool holder node id")?;
    }
    if let Some(root) = aida_core::allocation_lineage::RootRecord::load(store)? {
        node(&mut nodes, &root.holder_node)
            .context("invalid recorded root node id")?
            .recorded_root = true;
    }

    let objects = store.join("objects");
    if objects.try_exists()? {
        for (stem, path) in aida_core::object_store::list_objects_strict(&objects)? {
            let requirement = aida_core::object_store::read_object_from_path(&path)?;
            for id in std::iter::once(stem.as_str())
                .chain(requirement.spec_id.as_deref())
                .chain(requirement.agreed_id.as_deref())
            {
                if let Some(id) = namespace(id, &nodes) {
                    node(&mut nodes, id)?;
                }
            }
        }
    }

    if let Some(local) = local {
        let row = node(&mut nodes, &local.id)?;
        row.current = true;
        row.role = Some(if authoritative {
            AllocationRole::Root
        } else {
            AllocationRole::Node
        });
        if row.hostname.is_none() {
            row.hostname = local.hostname.clone();
        }
    }
    // Roles are clone-local configuration. Labels and shared allocation
    // records identify nodes but do not establish another clone's current role.
    // Keep the shared root designation distinct from private configuration.
    Ok(nodes.into_values().collect())
}

impl Node {
    fn role_label(&self) -> &'static str {
        match (self.role, self.recorded_root) {
            (Some(AllocationRole::Root), _) => "root",
            (Some(AllocationRole::Node), true) => "work node (recorded root)",
            (Some(AllocationRole::Node), false) => "work node",
            (None, true) => "recorded root",
            (None, false) => "unknown",
        }
    }
}

fn cell(value: &str) -> String {
    let mut text = String::new();
    for c in value.chars() {
        if c.is_control() {
            text.extend(c.escape_default());
        } else {
            text.push(c);
        }
    }
    text
}

fn render(nodes: &[Node], authoritative: bool) -> String {
    let mut out = format!(
        "Allocation: {}\n\n",
        if authoritative {
            "authoritative"
        } else {
            "node-local only"
        }
    );
    if nodes.is_empty() {
        out.push_str("No known nodes found.\n");
        return out;
    }
    let _ = writeln!(
        out,
        "{:<2}  {:<32}  {:<24}  {:<22}  {:<22}  Clone path",
        "", "Node", "Labels", "Role", "Hostname"
    );
    let _ = writeln!(out, "{}", "-".repeat(110));
    for row in nodes {
        let labels = if row.labels.is_empty() {
            "-".into()
        } else {
            row.labels.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        let clone_path = row
            .clone_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "-".into());
        let _ = writeln!(
            out,
            "{:<2}  {:<32}  {:<24}  {:<22}  {:<22}  {}",
            if row.current { "*" } else { "" },
            row.id,
            cell(&labels),
            row.role_label(),
            cell(row.hostname.as_deref().unwrap_or("-")),
            cell(&clone_path)
        );
    }
    out.push_str(
        "\nConfiguration roles are clone-local; 'recorded root' is the shared root designation.\n",
    );
    out
}

pub(super) fn list(store: &Path, authoritative: bool) -> Result<String> {
    let local = local_node(store)?;
    Ok(render(
        &read_nodes(store, local.as_ref(), authoritative)?,
        authoritative,
    ))
}

#[cfg(all(test, unix))]
#[path = "tests/bug026_node_inventory_tests.rs"]
mod tests;
