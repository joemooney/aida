//! `aida doctor --fix agreed-ids`: give every node-local spec that has no
//! agreed ID one, once.
//!
//! A spec filed on a node without an agreed-ID block (`STORY-<node>-105`)
//! carries only its local ID. This repair assigns each such spec the next
//! agreed number of its type, continuing that type's global sequence: one
//! past the highest number any spec (native or agreed), any block of that
//! type, any spent part of a shared `*` block, or the merge-gate counter
//! already holds. The local `spec_id` is never changed, so existing local
//! IDs, commit trailers and `trace:` comments still resolve; the agreed ID
//! is an additional name for the same spec.
//!
//! Flat specs (an agreed-form `spec_id` and no local ID) are left alone.
//!
//! `--dry-run` prints (and with `--mapping FILE` saves) the plan and writes
//! nothing. Applying needs `--yes` and the allocation root: in one store
//! write, each spec gets its agreed ID plus a history entry, the merge-gate
//! counter is raised to the highest assigned number, and a fully spent block
//! per type is recorded in `registry/blocks.yaml` so no later claim or
//! dispense reissues a backfilled number. A re-run finds nothing to do.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

use aida_core::models::{FieldChange, HistoryEntry, Requirement};

/// Author recorded on backfill history entries.
pub(crate) const BACKFILL_AUTHOR: &str = "aida-backfill-agreed-ids";

/// Options for one `aida doctor --fix agreed-ids` run.
#[derive(Debug, Clone, Default)]
pub(crate) struct BackfillOptions {
    pub dry_run: bool,
    pub yes: bool,
    pub mapping: Option<PathBuf>,
}

/// One planned assignment: the spec's local ID and its new agreed ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackfillRow {
    pub local: String,
    pub agreed: String,
    pub uuid: Uuid,
}

/// The type prefix and sequence of a node-local ID (`TYPE-<node>-NNN`),
/// where `<node>` is a 32-hex node id or a node in `registry/nodes.toml`.
fn local_parts(id: &str, nodes: &HashSet<String>) -> Option<(String, u64)> {
    let key = aida_core::id_identity::IdKey::parse(id)?;
    let (typ, node) = key.prefix.split_once('-')?;
    if typ.is_empty() || !typ.chars().all(|c| c.is_ascii_uppercase()) || node.contains('-') {
        return None;
    }
    let hex_node = node.len() == 32 && node.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex_node && !nodes.contains(node) {
        return None;
    }
    Some((typ.to_string(), key.seq.parse().ok()?))
}

/// The type prefix and number of an agreed-form ID (`TYPE-NNN`).
fn agreed_parts(id: &str) -> Option<(String, u64)> {
    let key = aida_core::id_identity::IdKey::parse(id)?;
    if key.prefix.is_empty() || !key.prefix.chars().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    Some((key.prefix, key.seq.parse().ok()?))
}

fn registered_nodes(store_root: &Path) -> HashSet<String> {
    aida_core::node::NodeRegistry::load(&store_root.join("registry/nodes.toml"))
        .map(|r| {
            r.nodes
                .into_iter()
                .map(|n| n.id.to_ascii_uppercase())
                .collect()
        })
        .unwrap_or_default()
}

/// The highest number each type already uses anywhere: native and agreed IDs
/// of every spec, every block of the type, the spent part of each shared `*`
/// block, and the merge-gate counter.
pub(crate) fn type_floors(
    reqs: &[Requirement],
    blocks: &aida_core::BlockRegistry,
    counters: &aida_core::node::AgreedCounters,
) -> BTreeMap<String, u64> {
    let mut floor: BTreeMap<String, u64> = BTreeMap::new();
    let mut raise = |typ: &str, n: u64| {
        let e = floor.entry(typ.to_ascii_uppercase()).or_insert(0);
        *e = (*e).max(n);
    };
    for r in reqs {
        for id in [r.spec_id.as_deref(), r.agreed_id.as_deref()]
            .into_iter()
            .flatten()
        {
            if let Some((typ, n)) = agreed_parts(id) {
                raise(&typ, n);
            }
        }
    }
    let mut shared = 0u64;
    for b in &blocks.blocks {
        if b.type_prefix == aida_core::IdCounterScope::GLOBAL_TYPE_PREFIX {
            if b.next > b.range_start {
                shared = shared.max(u64::from((b.next - 1).min(b.range_end)));
            }
        } else {
            raise(&b.type_prefix, u64::from(b.range_end));
        }
    }
    for (typ, n) in &counters.counters {
        raise(typ, u64::from(*n));
    }
    if shared > 0 {
        for v in floor.values_mut() {
            *v = (*v).max(shared);
        }
    }
    floor
}

/// Plan the backfill: every spec with a node-local `spec_id` and no agreed
/// ID, ordered by type then local number, numbered from its type's floor.
pub(crate) fn plan(
    reqs: &[Requirement],
    nodes: &HashSet<String>,
    floors: &BTreeMap<String, u64>,
) -> Vec<BackfillRow> {
    let mut pending: Vec<(String, u64, String, Uuid)> = reqs
        .iter()
        .filter(|r| r.agreed_id.as_deref().is_none_or(|a| a.trim().is_empty()))
        .filter_map(|r| {
            let local = r.spec_id.as_deref()?.trim();
            let (typ, n) = local_parts(local, nodes)?;
            Some((typ, n, local.to_string(), r.id))
        })
        .collect();
    pending.sort();
    let taken: HashSet<String> = reqs
        .iter()
        .flat_map(|r| [r.spec_id.as_deref(), r.agreed_id.as_deref()])
        .flatten()
        .map(aida_core::id_identity::id_key)
        .collect();
    let mut next: BTreeMap<String, u64> = BTreeMap::new();
    let mut rows = Vec::new();
    for (typ, _, local, uuid) in pending {
        let cursor = next
            .entry(typ.clone())
            .or_insert_with(|| floors.get(&typ).copied().unwrap_or(0));
        let agreed = loop {
            *cursor += 1;
            let seq = u32::try_from(*cursor).unwrap_or(u32::MAX);
            let candidate = aida_core::node::AgreedCounters::format_agreed_id(&typ, seq);
            if !taken.contains(&aida_core::id_identity::id_key(&candidate)) {
                break candidate;
            }
        };
        rows.push(BackfillRow {
            local,
            agreed,
            uuid,
        });
    }
    rows
}

pub(crate) fn render_mapping(rows: &[BackfillRow]) -> String {
    let mut out = String::from("# local_id\tagreed_id\tuuid\n");
    for r in rows {
        out.push_str(&format!("{}\t{}\t{}\n", r.local, r.agreed, r.uuid));
    }
    out
}

/// Per type: count, first and last assigned agreed ID.
fn summary(rows: &[BackfillRow]) -> BTreeMap<String, (usize, String, String)> {
    let mut out: BTreeMap<String, (usize, String, String)> = BTreeMap::new();
    for r in rows {
        let typ = agreed_parts(&r.agreed).map(|(t, _)| t).unwrap_or_default();
        let e = out
            .entry(typ)
            .or_insert_with(|| (0, r.agreed.clone(), r.agreed.clone()));
        e.0 += 1;
        e.2 = r.agreed.clone();
    }
    out
}

fn backfill_history(agreed: &str) -> HistoryEntry {
    HistoryEntry::new(
        BACKFILL_AUTHOR.to_string(),
        vec![FieldChange {
            field_name: "agreed_id".to_string(),
            old_value: String::new(),
            new_value: agreed.to_string(),
        }],
    )
}

fn load_counters(store_root: &Path) -> Result<aida_core::node::AgreedCounters> {
    let path = store_root.join("registry/agreed_counters.toml");
    if !path.exists() {
        return Ok(Default::default());
    }
    let text = aida_core::read_atomic(&path)?;
    toml::from_str(&text).with_context(|| format!("invalid {}", path.display()))
}

fn load_plan(store_root: &Path) -> Result<(Vec<Requirement>, Vec<BackfillRow>)> {
    let reqs = aida_core::object_store::load_all_objects(&store_root.join("objects"))?;
    let blocks = aida_core::BlockRegistry::load(&store_root.join("registry/blocks.yaml"))
        .unwrap_or_default();
    let counters = load_counters(store_root)?;
    let floors = type_floors(&reqs, &blocks, &counters);
    let rows = plan(&reqs, &registered_nodes(store_root), &floors);
    Ok((reqs, rows))
}

fn print_plan(rows: &[BackfillRow], reqs: &[Requirement]) {
    println!("Local ID -> agreed ID:");
    for r in rows {
        let title = reqs
            .iter()
            .find(|q| q.id == r.uuid)
            .map_or("", |q| q.title.as_str());
        println!("  {:<44} -> {:<12} {title}", r.local, r.agreed);
    }
    println!("Per type:");
    for (typ, (n, first, last)) in summary(rows) {
        println!("  {typ:<8} {n:>4}  {first} .. {last}");
    }
}

/// Write the plan to the store: agreed IDs, history, counters and the spent
/// ledger blocks, in one commit. Runs under the store write lock and
/// re-plans inside it, so a concurrent writer cannot slip a number in.
fn apply(store_root: &Path) -> Result<Vec<BackfillRow>> {
    aida_core::allocation_authority::require_global(store_root)?;
    aida_core::db::with_store_write_lock(store_root, || {
        aida_core::allocation_authority::require_global(store_root)?;
        let (reqs, rows) = load_plan(store_root)?;
        if rows.is_empty() {
            return Ok(rows);
        }
        let objects = store_root.join("objects");
        let by_uuid: BTreeMap<Uuid, &Requirement> = reqs.iter().map(|r| (r.id, r)).collect();
        let context = aida_core::provenance::WriteContext::capture();
        for row in &rows {
            let prior = *by_uuid.get(&row.uuid).context("planned spec vanished")?;
            let mut req = prior.clone();
            req.agreed_id = Some(row.agreed.clone());
            req.history.push(backfill_history(&row.agreed));
            aida_core::provenance::stamp_write(&mut req, Some(prior), &context)?;
            aida_core::object_store::write_object(&objects, &req)?;
        }
        // Ledger: raise the merge-gate counter and record one spent block per
        // type, so neither path can hand a backfilled number out again.
        let mut ranges: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        for row in &rows {
            let (typ, n) = agreed_parts(&row.agreed).context("unparsable agreed ID")?;
            let n = u32::try_from(n)?;
            let e = ranges.entry(typ).or_insert((n, n));
            e.0 = e.0.min(n);
            e.1 = e.1.max(n);
        }
        let mut counters = load_counters(store_root)?;
        let blocks_path = store_root.join("registry/blocks.yaml");
        std::fs::create_dir_all(store_root.join("registry"))?;
        aida_core::BlockRegistry::with_dispense_lock(&blocks_path, || {
            let mut blocks = aida_core::BlockRegistry::load(&blocks_path).unwrap_or_default();
            let node = aida_core::allocation_authority::node(store_root)
                .unwrap_or_else(|_| "root".to_string());
            for (typ, (start, end)) in &ranges {
                let c = counters.counters.entry(typ.clone()).or_insert(0);
                *c = (*c).max(*end);
                blocks.blocks.push(aida_core::node::AgreedIdBlock {
                    node_id: node.clone(),
                    owner: BACKFILL_AUTHOR.to_string(),
                    hostname: String::new(),
                    type_prefix: typ.clone(),
                    range_start: *start,
                    range_end: *end,
                    next: end.saturating_add(1),
                    allocated_at: chrono::Utc::now(),
                    pool_id: None,
                });
            }
            blocks.save(&blocks_path)
        })?;
        aida_core::write_atomic(
            &store_root.join("registry/agreed_counters.toml"),
            toml::to_string_pretty(&counters)?,
        )?;
        if aida_core::git_ops::is_git_repo(store_root)
            && !aida_core::git_ops::store_commit_deferred_to_host(store_root)
        {
            aida_core::git_ops::add_all(store_root, "objects")?;
            aida_core::git_ops::add(
                store_root,
                &["registry/agreed_counters.toml", "registry/blocks.yaml"],
            )?;
            aida_core::git_ops::commit(
                store_root,
                &format!(
                    "chore(doctor): backfill {} agreed ID(s) for node-local specs",
                    rows.len()
                ),
            )?;
        }
        Ok(rows)
    })
}

pub(crate) fn run(store_root: &Path, opts: &BackfillOptions) -> Result<()> {
    if !opts.dry_run && !opts.yes {
        anyhow::bail!(
            "the agreed-ID backfill writes the store; preview with --dry-run, \
             then re-run with --yes on the allocation root to apply"
        );
    }
    if opts.dry_run {
        let (reqs, rows) = load_plan(store_root)?;
        if rows.is_empty() {
            println!("No node-local spec lacks an agreed ID; nothing to backfill.");
            return Ok(());
        }
        print_plan(&rows, &reqs);
        if let Some(path) = &opts.mapping {
            std::fs::write(path, render_mapping(&rows))
                .with_context(|| format!("write {}", path.display()))?;
            println!("Saved the mapping to {}.", path.display());
        }
        println!(
            "Would assign {} agreed ID(s). (dry run: nothing written)",
            rows.len()
        );
        return Ok(());
    }
    let rows = apply(store_root)?;
    if rows.is_empty() {
        println!("No node-local spec lacks an agreed ID; nothing to backfill.");
        return Ok(());
    }
    let reqs = aida_core::object_store::load_all_objects(&store_root.join("objects"))?;
    print_plan(&rows, &reqs);
    if let Some(path) = &opts.mapping {
        std::fs::write(path, render_mapping(&rows))
            .with_context(|| format!("write {}", path.display()))?;
    }
    println!("Assigned {} agreed ID(s).", rows.len());
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_105_backfill_agreed_tests.rs"]
mod tests;
