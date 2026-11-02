//! `aida node hub add`: register another writable hub under the enrolled
//! root and grant it a child pool per ID type in use.
//!
//! The root carves each hub pool from its root pool's free space above
//! every number already issued for the type, records it in
//! `registry/pools.yaml`, and pushes that by CAS to the root's own hub, so
//! the grant is ordered against that hub's claims. The pool reaches the new
//! hub by an ordinary store sync or bundle; clones of the new hub then
//! declare it as their upstream and claim blocks inside the pool.
// trace:STORY-B6B1278890774EF993D2EBC327862470-170 | ai:claude

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::Path;

use aida_core::allocation_lineage::{self as lineage, PoolRecord};
use aida_core::store_hubs::{self, HubSet};

/// Refusal when the store has no enrolled root.
const NEEDS_ROOT: &str = "adding a hub needs an enrolled allocation root: with more than one hub, only the root can split the ID space between them. Run `aida node enroll-root` on the primary clone first";

/// `aida node hub add <name> --url <url> [--size N]`.
pub(crate) fn handle_hub_add(store_path: &Path, name: &str, url: &str, size: u32) -> Result<()> {
    use aida_core::allocation_authority as authority;
    anyhow::ensure!(
        store_hubs::valid_hub_name(name),
        "`{name}` is not a valid hub name (letters, digits, `-`, `_`)"
    );
    let normalized = store_hubs::normalize_url(url);
    anyhow::ensure!(!normalized.is_empty(), "`{url}` is not a git URL");
    anyhow::ensure!(size > 0, "--size must be at least 1");

    let root = lineage::RootRecord::load(store_path)?.context(NEEDS_ROOT)?;
    let node = authority::node(store_path)?;
    anyhow::ensure!(
        authority::resolve(store_path)?.authoritative && root.holder_node == node,
        "only the allocation root (node {}) grants ID pools to a hub; run `aida node hub add` on that clone (this clone is node {node})",
        root.holder_node
    );
    authority::refuse_foreign_root_record(store_path)?;

    let declared = store_hubs::working(store_path)?;
    let already = match declared.get(name) {
        Some(hub) if hub.urls.contains(&normalized) => true,
        Some(hub) => anyhow::bail!(
            "hub `{name}` is already declared under [store.hubs] with {}; a hub keeps its URLs",
            hub.urls.join(", ")
        ),
        None => false,
    };
    if let Some(other) = declared.hub_for_url(url).filter(|h| h.name != name) {
        anyhow::bail!(
            "{url} is already declared as hub `{}` under [store.hubs]",
            other.name
        );
    }
    let mut hubs = declared.clone();
    if !already {
        hubs.hubs.push(store_hubs::Hub {
            name: name.to_string(),
            urls: vec![normalized.clone()],
        });
    }

    // The root claims on `origin` (its claim remote): a hub that remote
    // reaches is the root's own hub, which needs no grant.
    let new_hub = hubs.get(name).context("hub just declared")?.clone();
    let root_hub = root_hub(store_path, &hubs, name);
    let own_hub = store_hubs::reach_problem(store_path, &new_hub, "origin").is_none();
    let granted = if own_hub {
        println!("Hub `{name}` is the hub this root claims on (`origin`); it needs no grant.");
        Vec::new()
    } else {
        grant(store_path, &hubs, &root, name, root_hub.as_deref(), size)?
    };
    if !already {
        declare(store_path, name, url)?;
    }

    for p in &granted {
        println!(
            "{} Granted hub `{name}` {} {}-{} (pool {}, from {})",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            p.type_prefix,
            p.range_start,
            p.range_end,
            p.pool_id,
            p.parent_pool_id.as_deref().unwrap_or("-"),
        );
    }
    if granted.is_empty() && !own_hub {
        println!("Hub `{name}` already holds a pool for every ID type in use.");
    }
    if already {
        println!("Hub `{name}` was already declared under [store.hubs].");
    } else {
        println!("Declared hub `{name}` ({normalized}) under [store.hubs] in .aida/config.toml.");
    }
    println!("Next:");
    if !already {
        println!(
            "  1. Commit .aida/config.toml and push it to the default branch on every hub (a hub counts only once it is there)."
        );
    }
    println!(
        "  2. Bring the store to hub `{name}`: push the `aida-store` branch to it, or carry a bundle."
    );
    println!(
        "  3. On each clone of `{name}`: `aida node upstream <remote that reaches {name}>`, then `aida db block claim --type <TYPE>`."
    );
    Ok(())
}

/// The declared hub (other than `new_hub`) this root reaches as `origin`,
/// its claim remote.
fn root_hub(store_path: &Path, hubs: &HubSet, new_hub: &str) -> Option<String> {
    hubs.hubs
        .iter()
        .filter(|h| h.name != new_hub)
        .find(|h| store_hubs::reach_problem(store_path, h, "origin").is_none())
        .map(|h| h.name.clone())
}

/// Every ID type a block or pool of the store uses (the shared `*` scope
/// has no per-type lineage and is never granted).
fn types_in_use(pools: &[PoolRecord], blocks: &[aida_core::AgreedIdBlock]) -> Vec<String> {
    let mut types: Vec<String> = pools
        .iter()
        .map(|p| p.type_prefix.to_ascii_uppercase())
        .chain(blocks.iter().map(|b| b.type_prefix.to_ascii_uppercase()))
        .filter(|t| !t.is_empty() && t != aida_core::IdCounterScope::GLOBAL_TYPE_PREFIX)
        .collect();
    types.sort();
    types.dedup();
    types
}

/// The root pool a `typ` grant is carved from: the root's own pool for the
/// type, preferring the one homed on the root's hub. `None` when the root
/// has none yet (a store rooted before single-hub mode).
fn root_pool<'a>(
    pools: &'a [PoolRecord],
    typ: &str,
    root_node: &str,
    root_hub: Option<&str>,
) -> Option<&'a PoolRecord> {
    let mut candidates = pools.iter().filter(|p| {
        p.parent_pool_id.is_none()
            && p.holder_node == root_node
            && p.type_prefix.eq_ignore_ascii_case(typ)
    });
    let first = candidates.next()?;
    Some(
        std::iter::once(first)
            .chain(candidates)
            .find(|p| root_hub.is_some() && lineage::promoted_hub(p) == root_hub)
            .unwrap_or(first),
    )
}

/// Plan and record the grants hub `hub` still lacks, one per type in use,
/// by CAS push to the root's hub. A type with no root pool yet gets one
/// (`hub:<root hub>:<TYPE>`, the whole number space) in the same commit.
fn grant(
    store_path: &Path,
    hubs: &HubSet,
    root: &lineage::RootRecord,
    hub: &str,
    root_hub: Option<&str>,
    size: u32,
) -> Result<Vec<PoolRecord>> {
    let remote = root_hub
        .and_then(|h| hubs.get(h))
        .and_then(|h| store_hubs::remote_reaching(store_path, hubs, h))
        .unwrap_or_else(|| "origin".to_string());
    let recorded = lineage::record_pools(
        store_path,
        &remote,
        &format!("chore(registry): grant hub {hub} its ID pools"),
        |store| {
            let pools = lineage::PoolRegistry::load(store)?.pools;
            let blocks =
                aida_core::node::BlockRegistry::load(&store.join("registry/blocks.yaml"))?.blocks;
            let mut all = pools.clone();
            let mut out = Vec::new();
            for typ in types_in_use(&pools, &blocks) {
                if lineage::hub_grant(&all, hub, &typ).is_some() {
                    continue;
                }
                let parent = match root_pool(&all, &typ, &root.holder_node, root_hub) {
                    Some(p) => p.clone(),
                    None => {
                        let Some(rh) = root_hub else {
                            anyhow::bail!(
                                "the root holds no {typ} pool and no declared hub is reached by its `origin`, so the grant has no parent. Declare the root's own hub first: `aida node hub add <name> --url <origin's URL>`"
                            );
                        };
                        let id = store_hubs::hub_pool_id(rh, &typ);
                        let made = lineage::materialized_hub_pool(&id, &root.holder_node)
                            .context("root pool id")?;
                        out.push(made.clone());
                        all.push(made.clone());
                        made
                    }
                };
                let floor = crate::doctor_backfill_agreed::hub_claim_floor(store, &typ)?;
                let child = lineage::hub_child_pool(hub, &parent, &all, &blocks, floor, size)?;
                all.push(child.clone());
                out.push(child);
            }
            Ok(out)
        },
    )?;
    Ok(recorded
        .into_iter()
        .filter(|p| p.parent_pool_id.is_some())
        .collect())
}

/// Add `name = "<url>"` under [store.hubs] in the project's
/// `.aida/config.toml`, keeping the rest of the file as written.
fn declare(store_path: &Path, name: &str, url: &str) -> Result<()> {
    let project_root = crate::find_project_root_from(store_path.parent().unwrap_or(store_path))?;
    let path = crate::config_init::config_path_for_project(&project_root);
    let mut doc = crate::config_edit::load_doc(&path)?;
    if !doc.contains_key("store") {
        doc.insert("store", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let store = doc["store"]
        .as_table_mut()
        .context("[store] in .aida/config.toml is not a table")?;
    if !store.contains_key("hubs") {
        store.insert("hubs", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let hubs = store["hubs"]
        .as_table_like_mut()
        .context("[store.hubs] in .aida/config.toml is not a table")?;
    hubs.insert(name, toml_edit::value(url));
    // Re-read it the way every clone will, so a malformed result fails here.
    HubSet::parse(&doc.to_string())?;
    crate::config_edit::save_doc(&path, &doc)
}

#[cfg(test)]
#[path = "tests/node_hub_tests.rs"]
mod tests;
