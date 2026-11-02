//! `aida doctor --fix agreed-ids`: give every node-local spec that has no
//! agreed ID one, once; retire the store's legacy node; and claim the
//! root's own per-type blocks above every number the backfill used.
//!
//! A spec filed on a node without an agreed-ID block (`STORY-<node>-105`)
//! carries only its local ID. This repair assigns each such spec the next
//! agreed number of its type, continuing that type's global sequence: one
//! past the highest number any spec (native or agreed), any block of that
//! type, any spent part of a shared `*` block, or the merge-gate counter
//! already holds. The local `spec_id` is never changed by the backfill, so
//! existing local IDs, commit trailers and `trace:` comments still resolve;
//! the agreed ID is an additional name for the same spec.
//!
//! Legacy node: when this clone is bound to a node but the store's
//! `.aida/node.toml` still names another (node `1` on the home store), the
//! same run renames that node's specs into the bound node's local IDs
//! (`TASK-1-418` -> `TASK-<bound>-178`), keeping every agreed ID, records the
//! old ID in each spec's history and in `registry/retired_ids.toml` (which
//! every store resolver honours, so `aida show TASK-1-418`, old commit
//! trailers and `trace:` comments still find the spec), raises the bound
//! node's counter past every renamed number so the next `aida add` mints
//! above them, and marks the old node retired in `registry/nodes.toml` so it
//! never allocates again.
//!
//! Blocks: on a bound root the run then claims one block per
//! auto-allocating type for the bound node (unless it already holds an
//! active one), starting above the highest agreed or backfilled number, and
//! raises the merge-gate counter past each claimed block. A claim made
//! through any other path before this backfill has run is refused
//! ([`refuse_claim_before_backfill`]), so the safe order is the only order.
//!
//! Flat specs (an agreed-form `spec_id` and no local ID) are left alone.
//!
//! `--dry-run` prints (and with `--mapping FILE` saves) the plan and writes
//! nothing. Applying needs `--yes` and the allocation root. Objects are
//! written first; any failure part-way restores every file already written.
//! In one store commit, each spec gets its agreed ID (and, on the legacy node, its new
//! local ID) plus a history entry, the merge-gate counter is raised, a fully
//! spent block per type is recorded in `registry/blocks.yaml` so no later
//! claim or dispense reissues a backfilled number, the root's blocks are
//! claimed, and the legacy node is marked retired. A re-run finds nothing to
//! do. An unreadable `registry/blocks.yaml` refuses the run; the ledger is
//! never rewritten from a partial view.
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

use aida_core::models::{FieldChange, HistoryEntry, Requirement};

/// Author recorded on backfill history entries and ledger blocks.
pub(crate) const BACKFILL_AUTHOR: &str = "aida-backfill-agreed-ids";

/// Size of each block the backfill claims for the root's own node.
pub(crate) const ROOT_BLOCK_SIZE: u32 = 100;

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

/// One planned rename of a legacy-node spec into the bound node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenameRow {
    pub old: String,
    pub new: String,
    pub uuid: Uuid,
}

/// The legacy node this run retires, and the bound node taking its specs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Retirement {
    pub old_node: String,
    pub new_node: String,
}

/// One block the run claims for the root's own node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedClaim {
    pub type_prefix: String,
    pub range_start: u32,
    pub range_end: u32,
}

/// Everything one run would do.
#[derive(Debug, Clone, Default)]
pub(crate) struct Plan {
    pub agreed: Vec<BackfillRow>,
    pub renames: Vec<RenameRow>,
    pub retire: Option<Retirement>,
    /// Whether the retirement still needs its `registry/nodes.toml` mark.
    pub mark_retired: bool,
    /// Whether this clone's `.aida/node.toml` still names the retired node
    /// and must be re-pointed at the bound node. trace:BUG-2378 | ai:claude
    pub repoint_identity: bool,
    pub claim_node: Option<String>,
    pub claims: Vec<PlannedClaim>,
}

impl Plan {
    fn is_empty(&self) -> bool {
        self.agreed.is_empty()
            && self.renames.is_empty()
            && !self.mark_retired
            && !self.repoint_identity
            && self.claims.is_empty()
    }
}

/// The type prefix and sequence of a node-local ID (`TYPE-<node>-NNN`),
/// where `<node>` is a 32-hex node id or a node in `registry/nodes.toml`.
fn local_parts(id: &str, nodes: &HashSet<String>) -> Option<(String, u64)> {
    let (typ, node, n) = local_triple(id)?;
    let hex_node = node.len() == 32 && node.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex_node && !nodes.contains(&node) {
        return None;
    }
    Some((typ, n))
}

/// `(TYPE, NODE, seq)` of any `TYPE-NODE-NNN` ID, both parts upper-cased.
fn local_triple(id: &str) -> Option<(String, String, u64)> {
    let key = aida_core::id_identity::IdKey::parse(id)?;
    let (typ, node) = key.prefix.split_once('-')?;
    if typ.is_empty() || !typ.chars().all(|c| c.is_ascii_uppercase()) || node.contains('-') {
        return None;
    }
    if node.is_empty() {
        return None;
    }
    Some((typ.to_string(), node.to_string(), key.seq.parse().ok()?))
}

/// The type prefix and number of an agreed-form ID (`TYPE-NNN`).
fn agreed_parts(id: &str) -> Option<(String, u64)> {
    let key = aida_core::id_identity::IdKey::parse(id)?;
    if key.prefix.is_empty() || !key.prefix.chars().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    Some((key.prefix, key.seq.parse().ok()?))
}

fn nodes_path(store_root: &Path) -> PathBuf {
    store_root.join("registry/nodes.toml")
}

fn load_nodes(store_root: &Path) -> Result<aida_core::node::NodeRegistry> {
    let path = nodes_path(store_root);
    if !path.try_exists()? {
        return Ok(Default::default());
    }
    aida_core::node::NodeRegistry::load(&path).with_context(|| {
        format!(
            "cannot read {}; refusing the agreed-ID backfill",
            path.display()
        )
    })
}

fn registered_nodes(registry: &aida_core::node::NodeRegistry) -> HashSet<String> {
    registry
        .nodes
        .iter()
        .map(|n| n.id.to_ascii_uppercase())
        .collect()
}

/// The block ledger, failing closed: a `blocks.yaml` that does not parse
/// refuses the run rather than planning (or saving) over an empty view.
fn load_blocks(store_root: &Path) -> Result<aida_core::BlockRegistry> {
    let path = store_root.join("registry/blocks.yaml");
    aida_core::BlockRegistry::load(&path).with_context(|| {
        format!(
            "cannot read {}; refusing the agreed-ID backfill (nothing was written). Repair the file, then re-run",
            path.display()
        )
    })
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
    let taken = taken_keys(reqs);
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

fn taken_keys(reqs: &[Requirement]) -> HashSet<String> {
    reqs.iter()
        .flat_map(|r| [r.spec_id.as_deref(), r.agreed_id.as_deref()])
        .flatten()
        .map(aida_core::id_identity::id_key)
        .collect()
}

/// Plan the legacy-node rename: every spec whose `spec_id` is local to
/// `old_node` gets the next free local ID of the bound `new_node`, per type,
/// above both the highest such ID already in the store and the bound
/// counter's last dispensed number (`counter_floor`), in old-number order.
pub(crate) fn plan_retirement(
    reqs: &[Requirement],
    retire: &Retirement,
    counter_floor: &BTreeMap<String, u64>,
    width: usize,
) -> Vec<RenameRow> {
    let old = retire.old_node.to_ascii_uppercase();
    let new = retire.new_node.to_ascii_uppercase();
    let mut next: BTreeMap<String, u64> = counter_floor
        .iter()
        .map(|(t, n)| (t.to_ascii_uppercase(), *n))
        .collect();
    let mut moving: Vec<(String, u64, String, Uuid)> = Vec::new();
    for r in reqs {
        let Some(sid) = r.spec_id.as_deref().map(str::trim) else {
            continue;
        };
        let Some((typ, node, n)) = local_triple(sid) else {
            continue;
        };
        if node == new {
            let e = next.entry(typ).or_insert(0);
            *e = (*e).max(n);
        } else if node == old {
            moving.push((typ, n, sid.to_string(), r.id));
        }
    }
    moving.sort();
    let taken = taken_keys(reqs);
    let mut rows = Vec::new();
    for (typ, _, old_id, uuid) in moving {
        let cursor = next.entry(typ.clone()).or_insert(0);
        let new_id = loop {
            *cursor += 1;
            let candidate = format!(
                "{typ}-{}-{:0width$}",
                retire.new_node,
                *cursor,
                width = width
            );
            if !taken.contains(&aida_core::id_identity::id_key(&candidate)) {
                break candidate;
            }
        };
        rows.push(RenameRow {
            old: old_id,
            new: new_id,
            uuid,
        });
    }
    rows
}

/// The legacy node to retire: the store's `.aida/node.toml` node when this
/// clone is bound to a different node and the legacy node is registered or
/// still names specs. `None` on an unbound clone.
fn retiring_node(store_root: &Path, reqs: &[Requirement]) -> Result<Option<Retirement>> {
    let Some(binding) = aida_core::work_binding::load(store_root)? else {
        return Ok(None);
    };
    let legacy = store_root.join(".aida/node.toml");
    if !legacy.try_exists()? {
        return Ok(None);
    }
    let old_node = aida_core::NodeConfig::load(&legacy)?.node_id;
    if old_node.trim().is_empty() || old_node.eq_ignore_ascii_case(&binding.node_id) {
        return Ok(None);
    }
    let upper = old_node.to_ascii_uppercase();
    let names_specs = reqs.iter().any(|r| {
        r.spec_id
            .as_deref()
            .and_then(local_triple)
            .is_some_and(|(_, node, _)| node == upper)
    });
    let registered = load_nodes(store_root)?
        .nodes
        .iter()
        .any(|n| n.id.eq_ignore_ascii_case(&old_node));
    if !names_specs && !registered {
        return Ok(None);
    }
    Ok(Some(Retirement {
        old_node,
        new_node: binding.node_id,
    }))
}

/// Whether the store's git tracks `.aida/node.toml`. A current store
/// ignores it (it is per-clone); an older one committed it to the shared
/// branch. Re-pointing a tracked copy would leave the store worktree dirty
/// (`aida remote reconcile --execute` then refuses) and, once committed,
/// hand the root's node to every unbound clone that pulls it. The clone's
/// WORK binding already outranks it for `aida node show` and lease stamps,
/// so the backfill leaves a tracked copy alone. Fails closed on a git error.
// trace:BUG-2378 | ai:claude
fn legacy_identity_tracked(store_root: &Path) -> Result<bool> {
    if !aida_core::git_ops::is_git_repo(store_root) {
        return Ok(false);
    }
    let out = aida_core::git_ops::host_git_command(store_root)
        .args(["ls-files", "--", ".aida/node.toml"])
        .output()
        .context("run git ls-files .aida/node.toml")?;
    anyhow::ensure!(
        out.status.success(),
        "git ls-files failed in {}: {}",
        store_root.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(!out.stdout.is_empty())
}

/// The bound counter's last dispensed number per type (empty when the
/// clone has no counter file). An unreadable counter refuses the run.
fn bound_counter_floor(store_root: &Path) -> Result<BTreeMap<String, u64>> {
    let Ok(path) = aida_core::work_binding::counter_path(store_root) else {
        return Ok(BTreeMap::new());
    };
    if !path.try_exists()? {
        return Ok(BTreeMap::new());
    }
    let text = aida_core::read_atomic(&path)?;
    let state: aida_core::dispenser::DispenserState =
        toml::from_str(&text).with_context(|| format!("invalid {}", path.display()))?;
    Ok(state
        .sequences
        .into_iter()
        .map(|(t, n)| (t.to_ascii_uppercase(), u64::from(n)))
        .collect())
}

/// The blocks to claim for `node`: one per auto-allocating type it holds
/// no active block for, starting above `floors` (taken after the backfill).
fn plan_claims(
    node: &str,
    blocks: &aida_core::BlockRegistry,
    floors: &BTreeMap<String, u64>,
) -> Vec<PlannedClaim> {
    let mut probe = blocks.clone();
    let mut out = Vec::new();
    for typ in crate::db_block::phase3_auto_alloc_types() {
        if probe.find_active_block(node, typ).is_some() {
            continue;
        }
        let floor = u32::try_from(floors.get(typ).copied().unwrap_or(0)).unwrap_or(u32::MAX);
        let b = probe.claim_block_with_floor(
            node.to_string(),
            BACKFILL_AUTHOR.to_string(),
            String::new(),
            typ.to_string(),
            ROOT_BLOCK_SIZE,
            floor,
        );
        out.push(PlannedClaim {
            type_prefix: b.type_prefix,
            range_start: b.range_start,
            range_end: b.range_end,
        });
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

fn rename_history(old: &str, new: &str) -> HistoryEntry {
    HistoryEntry::new(
        BACKFILL_AUTHOR.to_string(),
        vec![FieldChange {
            field_name: "spec_id".to_string(),
            old_value: old.to_string(),
            new_value: new.to_string(),
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

/// The whole plan from the store as it is now. Every registry read fails
/// closed.
fn load_plan(store_root: &Path) -> Result<(Vec<Requirement>, Plan)> {
    let reqs = aida_core::object_store::load_all_objects(&store_root.join("objects"))?;
    let blocks = load_blocks(store_root)?;
    let counters = load_counters(store_root)?;
    let nodes = load_nodes(store_root)?;
    let mut floors = type_floors(&reqs, &blocks, &counters);
    raise_for_granted_pools(store_root, &mut floors)?;
    let agreed = plan(&reqs, &registered_nodes(&nodes), &floors);
    for row in &agreed {
        if let Some((typ, n)) = agreed_parts(&row.agreed) {
            let e = floors.entry(typ).or_insert(0);
            *e = (*e).max(n);
        }
    }
    let retire = retiring_node(store_root, &reqs)?;
    let (renames, mark_retired) = match &retire {
        Some(r) => (
            plan_retirement(
                &reqs,
                r,
                &bound_counter_floor(store_root)?,
                aida_core::id_identity::installed_widths().node_mint_width(),
            ),
            !nodes.is_retired(&r.old_node),
        ),
        None => (Vec::new(), false),
    };
    // Claims: only a bound root claims for its own node; an unbound
    // (legacy single-node) store keeps its existing claim paths.
    let claim_node = if aida_core::allocation_authority::resolve(store_root)?.authoritative {
        aida_core::work_binding::load(store_root)?.map(|b| b.node_id)
    } else {
        None
    };
    let claims = claim_node
        .as_deref()
        .map(|node| plan_claims(node, &blocks, &floors))
        .unwrap_or_default();
    Ok((
        reqs,
        Plan {
            agreed,
            renames,
            // `retiring_node` only names a node `.aida/node.toml` still holds.
            repoint_identity: retire.is_some() && !legacy_identity_tracked(store_root)?,
            retire,
            mark_retired,
            claim_node,
            claims,
        },
    ))
}

/// Refuse a block claim on a bound clone before the agreed-ID backfill has
/// run: while node-local specs still lack agreed IDs (or a legacy node still
/// holds specs) and no backfill ledger block exists, a claim would start
/// below numbers the backfill is about to assign. The backfill itself claims
/// the root's blocks, above every backfilled number. An unbound store, or
/// one with nothing to backfill, is never refused.
///
/// Single-hub mode is never refused either: it has no root to run the
/// backfill, and every clone binding itself files a spec without an agreed
/// ID, so the gate would never open. A hub claim starts instead above
/// [`hub_claim_floor`], the highest number any spec, block or counter of its
/// type holds, so it can never land below a number the backfill would
/// assign (the backfill numbers above every block, including hub claims).
pub(crate) fn refuse_claim_before_backfill(store_root: &Path) -> Result<()> {
    if aida_core::work_binding::load(store_root)?.is_none() {
        return Ok(());
    }
    // trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
    // A delegated claim (STORY-170) is carved inside the pool the root
    // granted its hub, which the backfill numbers above (see
    // `raise_for_granted_pools`), so it cannot land on a backfilled number.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-170 | ai:claude
    if matches!(
        aida_core::allocation_authority::claim_authority(store_root),
        Ok(
            aida_core::allocation_authority::ClaimAuthority::HubCas { .. }
                | aida_core::allocation_authority::ClaimAuthority::Delegated { .. }
        )
    ) {
        return Ok(());
    }
    let blocks = load_blocks(store_root)?;
    if blocks.blocks.iter().any(|b| b.owner == BACKFILL_AUTHOR) {
        return Ok(());
    }
    let (_, plan) = load_plan(store_root)?;
    if plan.agreed.is_empty() && plan.renames.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to claim a block before the agreed-ID backfill: {} spec(s) still lack an agreed ID and {} still carry a retired node's local ID, and a block claimed now could start below the numbers the backfill assigns. Run `aida doctor --fix agreed-ids --dry-run`, then `aida doctor --fix agreed-ids --yes` on the allocation root; it claims this node's blocks above every backfilled number",
        plan.agreed.len(),
        plan.renames.len()
    )
}

/// The floor a single-hub claim of `type_prefix` starts above: the highest
/// number any spec (native or agreed ID), any block, the spent part of a
/// shared `*` block, or the merge-gate counter holds for the type, so a hub
/// claim never reissues a number and never starts below one the agreed-ID
/// backfill could assign later. Every read fails closed.
// trace:STORY-B6B1278890774EF993D2EBC327862470-164 | ai:claude
pub(crate) fn hub_claim_floor(store_root: &Path, type_prefix: &str) -> Result<u32> {
    let reqs = aida_core::object_store::load_all_objects(&store_root.join("objects"))?;
    let blocks = load_blocks(store_root)?;
    let counters = load_counters(store_root)?;
    let mut floors = type_floors(&reqs, &blocks, &counters);
    raise_for_granted_pools(store_root, &mut floors)?;
    let floor = floors
        .get(&type_prefix.to_ascii_uppercase())
        .copied()
        .unwrap_or(0);
    Ok(u32::try_from(floor).unwrap_or(u32::MAX))
}

/// Raise each type's floor above every range the root granted a hub (`aida
/// node hub add`): such a range is spent by that hub's clones, so neither
/// the backfill nor a claim placed above every block may number into it.
/// Fails closed on an unreadable pool registry.
// trace:STORY-B6B1278890774EF993D2EBC327862470-170 | ai:claude
fn raise_for_granted_pools(store_root: &Path, floors: &mut BTreeMap<String, u64>) -> Result<()> {
    let pools = aida_core::allocation_lineage::PoolRegistry::load(store_root)?;
    for p in pools.pools.iter().filter(|p| p.parent_pool_id.is_some()) {
        let e = floors
            .entry(p.type_prefix.to_ascii_uppercase())
            .or_insert(0);
        *e = (*e).max(u64::from(p.range_end));
    }
    Ok(())
}

fn print_plan(plan: &Plan, reqs: &[Requirement]) {
    let title = |uuid: Uuid| {
        reqs.iter()
            .find(|q| q.id == uuid)
            .map_or("", |q| q.title.as_str())
    };
    if !plan.agreed.is_empty() {
        println!("Local ID -> agreed ID:");
        for r in &plan.agreed {
            println!("  {:<44} -> {:<12} {}", r.local, r.agreed, title(r.uuid));
        }
        println!("Per type:");
        for (typ, (n, first, last)) in summary(&plan.agreed) {
            println!("  {typ:<8} {n:>4}  {first} .. {last}");
        }
    }
    if let Some(r) = &plan.retire {
        if !plan.renames.is_empty() {
            println!(
                "Node {} local ID -> node {} local ID (agreed IDs kept):",
                r.old_node, r.new_node
            );
            for row in &plan.renames {
                println!("  {:<20} -> {:<44} {}", row.old, row.new, title(row.uuid));
            }
        }
        if plan.mark_retired {
            println!(
                "Node {} is marked retired in registry/nodes.toml.",
                r.old_node
            );
        }
        if plan.repoint_identity {
            println!(
                "This clone's .aida/node.toml identity: node {} -> node {}.",
                r.old_node, r.new_node
            );
        }
    }
    if let Some(node) = &plan.claim_node {
        if !plan.claims.is_empty() {
            println!("Blocks claimed for node {node}:");
            for c in &plan.claims {
                println!("  {}-{}..{}", c.type_prefix, c.range_start, c.range_end);
            }
        }
    }
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

pub(crate) fn render_mapping(plan: &Plan) -> String {
    let mut out = String::from("# local_id\tagreed_id\tuuid\n");
    for r in &plan.agreed {
        out.push_str(&format!("{}\t{}\t{}\n", r.local, r.agreed, r.uuid));
    }
    if !plan.renames.is_empty() {
        out.push_str("# old_local_id\tnew_local_id\tuuid\n");
        for r in &plan.renames {
            out.push_str(&format!("{}\t{}\t{}\n", r.old, r.new, r.uuid));
        }
    }
    out
}

/// `FILE` -> `FILE.tree.tsv`: where the tree-rewrite mapping goes.
pub(crate) fn tree_mapping_path(mapping: &Path) -> PathBuf {
    let mut name = mapping.as_os_str().to_owned();
    name.push(".tree.tsv");
    PathBuf::from(name)
}

/// The retired node's old local IDs mapped to each spec's agreed ID (the
/// everyday name), in the `old new uuid` format `aida doctor --fix
/// id-collisions --tree-only --mapping` reads, so `trace:` comments and docs
/// citing an old ID can be rewritten. Commit trailers are immutable; each
/// spec's history keeps its old ID.
pub(crate) fn render_tree_mapping(plan: &Plan, reqs: &[Requirement]) -> String {
    let mut out = String::from("# old_id\tagreed_id\tuuid\n");
    for r in &plan.renames {
        let agreed = plan
            .agreed
            .iter()
            .find(|a| a.uuid == r.uuid)
            .map(|a| a.agreed.clone())
            .or_else(|| {
                reqs.iter()
                    .find(|q| q.id == r.uuid)
                    .and_then(|q| q.agreed_id.clone())
            })
            .unwrap_or_else(|| r.new.clone());
        out.push_str(&format!("{}\t{}\t{}\n", r.old, agreed, r.uuid));
    }
    out
}

/// Every file the apply wrote, with its prior bytes (`None`: it did not
/// exist), so a failure part-way restores the store to where it started.
#[derive(Default)]
struct Undo {
    files: Vec<(PathBuf, Option<Vec<u8>>)>,
    /// The bound counter's prior text, restored under its dispense lock.
    counter: Option<String>,
}

impl Undo {
    /// Record `path`'s current bytes, once, before the first write to it.
    fn snapshot(&mut self, path: &Path) -> Result<()> {
        if self.files.iter().any(|(p, _)| p == path) {
            return Ok(());
        }
        let prior = match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
        };
        self.files.push((path.to_path_buf(), prior));
        Ok(())
    }

    /// Restore every snapshot, newest first; returns what it could not
    /// restore.
    fn restore(self, store_root: &Path) -> Vec<String> {
        let mut failed = Vec::new();
        if let Some(prior) = &self.counter {
            if let Err(e) = aida_core::work_binding::restore_counter(store_root, prior) {
                failed.push(format!("the node counter ({e:#})"));
            }
        }
        let blocks_path = store_root.join("registry/blocks.yaml");
        for (path, prior) in self.files.into_iter().rev() {
            let put_back = || -> Result<()> {
                match &prior {
                    Some(bytes) => aida_core::write_atomic(&path, bytes)?,
                    None => {
                        if let Err(e) = std::fs::remove_file(&path) {
                            if e.kind() != std::io::ErrorKind::NotFound {
                                return Err(e.into());
                            }
                        }
                    }
                }
                Ok(())
            };
            let restored = if path == blocks_path {
                aida_core::BlockRegistry::with_dispense_lock(&blocks_path, put_back)
            } else {
                put_back()
            };
            if let Err(e) = restored {
                failed.push(format!("{} ({e:#})", path.display()));
            }
        }
        failed
    }
}

/// Write the plan to the store in one commit. Runs under the store write
/// lock and re-plans inside it, so a concurrent writer cannot slip a number
/// in; every registry file is read up front and a parse failure refuses
/// before anything is written. Objects are written first and the ledger
/// blocks that lift [`refuse_claim_before_backfill`] near last; a failure
/// part-way restores every file already written, so a half-applied run
/// never leaves the claim gate open over half-assigned specs.
fn apply(store_root: &Path) -> Result<(Vec<Requirement>, Plan)> {
    aida_core::allocation_authority::require_global(store_root)?;
    aida_core::db::with_store_write_lock(store_root, || {
        aida_core::allocation_authority::require_global(store_root)?;
        let (reqs, plan) = load_plan(store_root)?;
        if plan.is_empty() {
            return Ok((reqs, plan));
        }
        // Read every registry file up front (fail closed) before writing.
        load_blocks(store_root)?;
        let counters = load_counters(store_root)?;
        let nodes = load_nodes(store_root)?;
        let retired_ids = aida_core::node_alias::RetiredIds::load_strict(store_root)
            .context("refusing the agreed-ID backfill (nothing was written)")?;
        let mut undo = Undo::default();
        let written = write_plan(
            store_root,
            &reqs,
            &plan,
            counters,
            nodes,
            retired_ids,
            &mut undo,
        );
        if let Err(e) = written {
            let failed = undo.restore(store_root);
            if failed.is_empty() {
                return Err(e.context(
                    "the agreed-ID backfill failed; every file it wrote was restored. Re-run once the cause is fixed",
                ));
            }
            return Err(e.context(format!(
                "the agreed-ID backfill failed and could not restore: {}. Repair those by hand (git -C {} status) before re-running",
                failed.join(", "),
                store_root.display()
            )));
        }
        if aida_core::git_ops::is_git_repo(store_root)
            && !aida_core::git_ops::store_commit_deferred_to_host(store_root)
        {
            aida_core::git_ops::add_all(store_root, "objects")?;
            let mut paths = vec!["registry/agreed_counters.toml", "registry/blocks.yaml"];
            if plan.mark_retired {
                paths.push("registry/nodes.toml");
            }
            if !plan.renames.is_empty() {
                paths.push(aida_core::node_alias::RETIRED_IDS_TOML_REL);
            }
            aida_core::git_ops::add(store_root, &paths)?;
            aida_core::git_ops::commit(
                store_root,
                &format!(
                    "chore(doctor): backfill {} agreed ID(s), rename {} legacy-node spec(s), claim {} block(s)",
                    plan.agreed.len(),
                    plan.renames.len(),
                    plan.claims.len()
                ),
            )?;
        }
        Ok((reqs, plan))
    })
}

/// The file writes of [`apply`], each snapshotted into `undo` first.
fn write_plan(
    store_root: &Path,
    reqs: &[Requirement],
    plan: &Plan,
    mut counters: aida_core::node::AgreedCounters,
    mut nodes: aida_core::node::NodeRegistry,
    mut retired_ids: aida_core::node_alias::RetiredIds,
    undo: &mut Undo,
) -> Result<()> {
    // 1. Objects: agreed IDs and legacy-node renames, merged per spec.
    let objects = store_root.join("objects");
    let by_uuid: BTreeMap<Uuid, &Requirement> = reqs.iter().map(|r| (r.id, r)).collect();
    let agreed: BTreeMap<Uuid, &BackfillRow> = plan.agreed.iter().map(|r| (r.uuid, r)).collect();
    let renames: BTreeMap<Uuid, &RenameRow> = plan.renames.iter().map(|r| (r.uuid, r)).collect();
    let touched: std::collections::BTreeSet<Uuid> =
        agreed.keys().chain(renames.keys()).copied().collect();
    let context = aida_core::provenance::WriteContext::capture();
    for uuid in touched {
        let prior = *by_uuid.get(&uuid).context("planned spec vanished")?;
        let mut req = prior.clone();
        if let Some(row) = agreed.get(&uuid) {
            req.agreed_id = Some(row.agreed.clone());
            req.history.push(backfill_history(&row.agreed));
        }
        let old_path = match renames.get(&uuid) {
            Some(row) => {
                req.spec_id = Some(row.new.clone());
                req.history.push(rename_history(&row.old, &row.new));
                Some(aida_core::object_store::object_path(&objects, &row.old)?)
            }
            None => None,
        };
        let new_path = aida_core::object_store::object_path(
            &objects,
            req.spec_id
                .as_deref()
                .context("planned spec has no spec_id")?,
        )?;
        undo.snapshot(&new_path)?;
        if let Some(old) = &old_path {
            undo.snapshot(old)?;
        }
        aida_core::provenance::stamp_write(&mut req, Some(prior), &context)?;
        aida_core::object_store::write_object(&objects, &req)?;
        if let Some(old) = old_path.filter(|old| *old != new_path) {
            std::fs::remove_file(&old).with_context(|| format!("remove {}", old.display()))?;
        }
        fail_point("object")?;
    }

    std::fs::create_dir_all(store_root.join("registry"))?;
    if !plan.renames.is_empty() {
        // 2. Old local IDs stay resolvable: every store resolver honours the
        // retired-ID record (`node_alias::RetiredIds`).
        for r in &plan.renames {
            retired_ids.add(&r.old, &r.new, &r.uuid.to_string());
        }
        let path = store_root.join(aida_core::node_alias::RETIRED_IDS_TOML_REL);
        undo.snapshot(&path)?;
        aida_core::write_atomic(&path, toml::to_string_pretty(&retired_ids)?)?;

        // 3. The bound node's counter: its next dispense must land above
        // every renamed number, or `aida add` would mint a renamed spec's ID
        // and run out of collision skips.
        let mut high: BTreeMap<String, u32> = BTreeMap::new();
        for r in &plan.renames {
            let (typ, _, n) = local_triple(&r.new).context("unparsable renamed local ID")?;
            let e = high.entry(typ).or_insert(0);
            *e = (*e).max(u32::try_from(n)?);
        }
        undo.counter = Some(aida_core::work_binding::raise_counter(store_root, &high)?);
    }

    // 4. The block ledger: a fully spent block per backfilled type, then the
    // root's own blocks. These lift the claim gate, so only the counters and
    // the node mark come after them.
    let mut ranges: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for row in &plan.agreed {
        let (typ, n) = agreed_parts(&row.agreed).context("unparsable agreed ID")?;
        let n = u32::try_from(n)?;
        let e = ranges.entry(typ).or_insert((n, n));
        e.0 = e.0.min(n);
        e.1 = e.1.max(n);
    }
    let ledger_node = plan.claim_node.clone().unwrap_or_else(|| {
        aida_core::allocation_authority::node(store_root).unwrap_or_else(|_| "root".into())
    });
    let blocks_path = store_root.join("registry/blocks.yaml");
    aida_core::BlockRegistry::with_dispense_lock(&blocks_path, || {
        let mut blocks = load_blocks(store_root)?;
        for (typ, (start, end)) in &ranges {
            let c = counters.counters.entry(typ.clone()).or_insert(0);
            *c = (*c).max(*end);
            blocks.blocks.push(aida_core::node::AgreedIdBlock {
                node_id: ledger_node.clone(),
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
        for c in &plan.claims {
            blocks.blocks.push(aida_core::node::AgreedIdBlock {
                node_id: plan.claim_node.clone().unwrap_or_default(),
                owner: BACKFILL_AUTHOR.to_string(),
                hostname: String::new(),
                type_prefix: c.type_prefix.clone(),
                range_start: c.range_start,
                range_end: c.range_end,
                next: c.range_start,
                allocated_at: chrono::Utc::now(),
                pool_id: None,
            });
            // The merge gate numbers pending specs from the counter, not
            // from blocks: keep it above the root's own block.
            let e = counters.counters.entry(c.type_prefix.clone()).or_insert(0);
            *e = (*e).max(c.range_end);
        }
        undo.snapshot(&blocks_path)?;
        blocks.save(&blocks_path)
    })?;
    let counters_path = store_root.join("registry/agreed_counters.toml");
    undo.snapshot(&counters_path)?;
    aida_core::write_atomic(&counters_path, toml::to_string_pretty(&counters)?)?;

    // 5. Retire the legacy node so it never allocates again.
    if let (Some(r), true) = (&plan.retire, plan.mark_retired) {
        let now = chrono::Utc::now();
        match nodes
            .nodes
            .iter_mut()
            .find(|n| n.id.eq_ignore_ascii_case(&r.old_node))
        {
            Some(n) => n.retired_at = Some(now),
            None => nodes.nodes.push(aida_core::node::NodeRegistryEntry {
                id: r.old_node.clone(),
                user_id: 0,
                hostname: String::new(),
                email: None,
                clone_path: None,
                name: None,
                user: None,
                registered: now,
                retired_at: Some(now),
            }),
        }
        let path = nodes_path(store_root);
        undo.snapshot(&path)?;
        nodes.save(&path)?;
    }

    // 6. Re-point this clone's legacy identity at the bound node, so
    // `aida node show` and every lease or commit stamp stop naming the
    // retired node. The file is clone-local (never committed).
    // trace:BUG-2378 | ai:claude
    if let (Some(r), true) = (&plan.retire, plan.repoint_identity) {
        let path = store_root.join(".aida/node.toml");
        let mut config = aida_core::NodeConfig::load(&path)?;
        config.node_id = r.new_node.clone();
        undo.snapshot(&path)?;
        config.save(&path)?;
    }
    fail_point("end")
}

#[cfg(test)]
thread_local! {
    /// Test hook: the step name at which [`write_plan`] fails.
    pub(crate) static FAIL_AT: std::cell::Cell<Option<&'static str>> =
        const { std::cell::Cell::new(None) };
}

fn fail_point(_step: &'static str) -> Result<()> {
    #[cfg(test)]
    if FAIL_AT.with(|f| f.get()) == Some(_step) {
        anyhow::bail!("injected backfill failure at {_step}");
    }
    Ok(())
}

pub(crate) fn run(store_root: &Path, opts: &BackfillOptions) -> Result<()> {
    if !opts.dry_run && !opts.yes {
        anyhow::bail!(
            "the agreed-ID backfill writes the store; preview with --dry-run, \
             then re-run with --yes on the allocation root to apply"
        );
    }
    let (reqs, plan) = if opts.dry_run {
        load_plan(store_root)?
    } else {
        apply(store_root)?
    };
    if plan.is_empty() {
        println!("No node-local spec lacks an agreed ID; nothing to backfill.");
        return Ok(());
    }
    print_plan(&plan, &reqs);
    if let Some(path) = &opts.mapping {
        std::fs::write(path, render_mapping(&plan))
            .with_context(|| format!("write {}", path.display()))?;
        println!("Saved the mapping to {}.", path.display());
        if !plan.renames.is_empty() {
            let tree = tree_mapping_path(path);
            std::fs::write(&tree, render_tree_mapping(&plan, &reqs))
                .with_context(|| format!("write {}", tree.display()))?;
            println!(
                "Saved the retired node's old ID -> agreed ID mapping to {}; rewrite code and doc mentions with `aida doctor --fix id-collisions --tree-only --mapping {}` (add --dry-run to preview).",
                tree.display(),
                tree.display()
            );
        }
    }
    let verb = if opts.dry_run {
        "Would assign"
    } else {
        "Assigned"
    };
    println!(
        "{verb} {} agreed ID(s), {} legacy-node rename(s), {} block claim(s).{}",
        plan.agreed.len(),
        plan.renames.len(),
        plan.claims.len(),
        if opts.dry_run {
            " (dry run: nothing written)"
        } else {
            ""
        }
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_105_backfill_agreed_tests.rs"]
mod tests;
