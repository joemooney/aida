//! Hierarchical block delegation: the lineage records that tie every ID
//! block back to the one root that owns the unallocated space.
//!
//! Records are append-only grants. A pool is an immutable
//! `{pool_id, type_prefix, range, parent_pool_id, holder_node, home_ref}`;
//! a claim is an immutable child pool or block whose range lies inside its
//! parent's. Free space is derived (parent range minus children), never
//! stored, so two stores that each took a claim union-merge cleanly and any
//! overlap shows up as a listed conflict rather than a silent double grant.
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
use crate::node::AgreedIdBlock;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

pub const POOLS_YAML_REL: &str = "registry/pools.yaml";
pub const ROOT_YAML_REL: &str = "registry/root.yaml";

/// IDs per type a leaf node claims at a time.
pub const DEFAULT_LEAF_BLOCK: u32 = 100;
/// IDs per type an intermediate (hub) pool holds.
pub const DEFAULT_HUB_POOL: u32 = 1000;
/// Replenish once a block has this percentage or less remaining.
pub const DEFAULT_LOW_WATER_PCT: u32 = 20;

/// A clone's place in the delegation tree (clone-local, never shared).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AllocationRole {
    Root,
    Node,
}

/// An immutable grant of one type's range to a holder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolRecord {
    pub pool_id: String,
    pub type_prefix: String,
    pub range_start: u32,
    pub range_end: u32,
    /// `None` only for a root pool, the one place new ranges come from.
    #[serde(default)]
    pub parent_pool_id: Option<String>,
    /// Opaque node id holding the pool.
    pub holder_node: String,
    /// The single remote ref where claims against this pool are pushed.
    pub home_ref: String,
}

impl PoolRecord {
    fn label(&self) -> String {
        format!(
            "pool {} ({} {}-{})",
            self.pool_id, self.type_prefix, self.range_start, self.range_end
        )
    }
}

/// `registry/pools.yaml`: the append-only pool grants.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PoolRegistry {
    #[serde(default)]
    pub pools: Vec<PoolRecord>,
}

impl PoolRegistry {
    pub fn load(store_root: &Path) -> Result<Self> {
        let path = store_root.join(POOLS_YAML_REL);
        if !path.try_exists()? {
            return Ok(Self::default());
        }
        let text = crate::read_atomic(&path)?;
        serde_yaml::from_str(&text).with_context(|| format!("invalid {POOLS_YAML_REL}"))
    }

    /// Union two registries the way a store reconcile does: identical
    /// records collapse, everything else is kept for the invariant check.
    pub fn union(&self, other: &Self) -> Self {
        let mut pools = self.pools.clone();
        for p in &other.pools {
            if !pools.contains(p) {
                pools.push(p.clone());
            }
        }
        Self { pools }
    }
}

/// `registry/root.yaml`: which node is root. `epoch` bumps on transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootRecord {
    pub holder_node: String,
    pub epoch: u64,
}

impl RootRecord {
    pub fn load(store_root: &Path) -> Result<Option<Self>> {
        let path = store_root.join(ROOT_YAML_REL);
        if !path.try_exists()? {
            return Ok(None);
        }
        let text = crate::read_atomic(&path)?;
        serde_yaml::from_str(&text)
            .map(Some)
            .with_context(|| format!("invalid {ROOT_YAML_REL}"))
    }
}

/// What `enroll_root` did (or, for a dry run, would do).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollOutcome {
    Enrolled,
    AlreadyRoot,
}

/// Refuse when another node already holds root. The holder itself re-runs
/// idempotently; transferring the role is a separate, root-run command.
pub fn enroll_preflight(store_root: &Path, node: &str) -> Result<EnrollOutcome> {
    match RootRecord::load(store_root)? {
        Some(r) if r.holder_node == node => Ok(EnrollOutcome::AlreadyRoot),
        Some(r) => bail!(
            "node {} is already the allocation root of this store; a store has exactly one root, and only the root can hand the role over",
            r.holder_node
        ),
        None => Ok(EnrollOutcome::Enrolled),
    }
}

/// Record `node` as the store's root in `registry/root.yaml`, by CAS push
/// to origin when the store has one, so two clones racing to enroll never
/// both win.
pub fn enroll_root(store_root: &Path, node: &str) -> Result<EnrollOutcome> {
    use crate::git_ops;
    const MAX_RETRIES: u32 = 10;
    let branch = git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    let local_only = !git_ops::has_remote(store_root, "origin");
    for attempt in 0..MAX_RETRIES {
        let committed = crate::db::with_store_write_lock(store_root, || {
            if attempt > 0 && !local_only {
                git_ops::pull_rebase(store_root, "origin", &branch)?;
            }
            if enroll_preflight(store_root, node)? == EnrollOutcome::AlreadyRoot {
                return Ok(None);
            }
            let record = RootRecord {
                holder_node: node.to_string(),
                epoch: 1,
            };
            let path = store_root.join(ROOT_YAML_REL);
            std::fs::create_dir_all(path.parent().context("invalid root record path")?)?;
            crate::write_atomic(&path, serde_yaml::to_string(&record)?)?;
            git_ops::add(store_root, &[ROOT_YAML_REL])?;
            git_ops::commit(
                store_root,
                &format!("chore(registry): enroll node {node} as allocation root"),
            )?;
            Ok(Some(git_ops::head_sha(store_root)?))
        })?;
        let Some(sha) = committed else {
            return Ok(EnrollOutcome::AlreadyRoot);
        };
        if local_only || git_ops::push_exact(store_root, "origin", &sha, &branch)? {
            return Ok(EnrollOutcome::Enrolled);
        }
        let _ = git_ops::rollback_commit_if_head_matches(store_root, &sha);
    }
    bail!("could not record the allocation root after {MAX_RETRIES} attempts (store push kept being rejected); run `aida db sync --pull` and retry")
}

/// A lineage invariant violation. Never auto-resolved: every one is listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineageConflict {
    DuplicatePoolId(String),
    InvalidRange(String),
    WildcardType(String),
    UnknownParent {
        record: String,
        parent: String,
    },
    TypeMismatch {
        record: String,
        parent: String,
    },
    OutsideParent {
        record: String,
        parent: String,
    },
    Overlap {
        a: String,
        b: String,
    },
    /// `pool`'s lineage enters a cycle at `at`; `at == pool` when `pool`
    /// itself lies on the cycle.
    Cycle {
        pool: String,
        at: String,
    },
    RootPoolNotHeldByRoot {
        pool: String,
        holder: String,
    },
}

impl std::fmt::Display for LineageConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicatePoolId(id) => {
                write!(f, "pool id {id} is recorded twice with different grants")
            }
            Self::InvalidRange(r) => write!(f, "{r} has an empty or inverted range"),
            Self::WildcardType(r) => {
                write!(f, "{r} names no single type; lineage ranges are per type")
            }
            Self::UnknownParent { record, parent } => {
                write!(f, "{record} names unknown parent pool {parent}")
            }
            Self::TypeMismatch { record, parent } => {
                write!(f, "{record} has a different type than its parent {parent}")
            }
            Self::OutsideParent { record, parent } => {
                write!(f, "{record} lies outside its parent {parent}")
            }
            Self::Overlap { a, b } => write!(f, "{a} overlaps {b}"),
            Self::Cycle { pool, at } if pool == at => write!(f, "pool {pool} is its own ancestor"),
            Self::Cycle { pool, at } => {
                write!(f, "the lineage of pool {pool} reaches a cycle at pool {at}")
            }
            Self::RootPoolNotHeldByRoot { pool, holder } => {
                write!(
                    f,
                    "root {pool} is held by node {holder}, which is not the store's root"
                )
            }
        }
    }
}

/// One range in the tree: a child pool or a lineage-bearing block.
struct Span<'a> {
    label: String,
    type_prefix: &'a str,
    start: u32,
    end: u32,
    parent: Option<&'a str>,
}

fn spans<'a>(pools: &'a [PoolRecord], blocks: &'a [AgreedIdBlock]) -> Vec<Span<'a>> {
    let mut out: Vec<Span<'a>> = pools
        .iter()
        .map(|p| Span {
            label: p.label(),
            type_prefix: &p.type_prefix,
            start: p.range_start,
            end: p.range_end,
            parent: p.parent_pool_id.as_deref(),
        })
        .collect();
    out.extend(blocks.iter().filter_map(|b| {
        b.pool_id.as_deref().map(|pool| Span {
            label: format!(
                "block of node {} ({} {}-{})",
                b.node_id, b.type_prefix, b.range_start, b.range_end
            ),
            type_prefix: &b.type_prefix,
            start: b.range_start,
            end: b.range_end,
            parent: Some(pool),
        })
    }));
    out
}

/// Check the tree: children are disjoint and inside a same-typed parent,
/// parents exist without cycles, and root pools belong to the root.
pub fn check_invariants(
    pools: &[PoolRecord],
    blocks: &[AgreedIdBlock],
    root: Option<&RootRecord>,
) -> Vec<LineageConflict> {
    let mut conflicts = Vec::new();
    let mut by_id: HashMap<&str, &PoolRecord> = HashMap::new();
    for p in pools {
        if by_id.insert(p.pool_id.as_str(), p).is_some() {
            conflicts.push(LineageConflict::DuplicatePoolId(p.pool_id.clone()));
        }
    }
    for p in pools {
        if p.parent_pool_id.is_none() && root.is_none_or(|r| r.holder_node != p.holder_node) {
            conflicts.push(LineageConflict::RootPoolNotHeldByRoot {
                pool: p.pool_id.clone(),
                holder: p.holder_node.clone(),
            });
        }
        let mut seen = HashSet::new();
        let mut current = p;
        while let Some(parent) = current.parent_pool_id.as_deref() {
            if !seen.insert(current.pool_id.as_str()) {
                conflicts.push(LineageConflict::Cycle {
                    pool: p.pool_id.clone(),
                    at: current.pool_id.clone(),
                });
                break;
            }
            match by_id.get(parent) {
                Some(next) => current = next,
                None => break,
            }
        }
    }
    // Siblings grouped by (parent, type); root pools are siblings per type.
    let mut siblings: BTreeMap<(Option<&str>, String), Vec<Span>> = BTreeMap::new();
    for span in spans(pools, blocks) {
        let typ = span.type_prefix.to_ascii_uppercase();
        if span.start == 0 || span.start > span.end {
            conflicts.push(LineageConflict::InvalidRange(span.label));
            continue;
        }
        if typ == crate::node::IdCounterScope::GLOBAL_TYPE_PREFIX || typ.is_empty() {
            conflicts.push(LineageConflict::WildcardType(span.label));
            continue;
        }
        if let Some(parent_id) = span.parent {
            let Some(parent) = by_id.get(parent_id) else {
                conflicts.push(LineageConflict::UnknownParent {
                    record: span.label,
                    parent: parent_id.to_string(),
                });
                continue;
            };
            if !parent.type_prefix.eq_ignore_ascii_case(&typ) {
                conflicts.push(LineageConflict::TypeMismatch {
                    record: span.label,
                    parent: parent.label(),
                });
                continue;
            }
            if span.start < parent.range_start || span.end > parent.range_end {
                conflicts.push(LineageConflict::OutsideParent {
                    record: span.label,
                    parent: parent.label(),
                });
                continue;
            }
        }
        siblings.entry((span.parent, typ)).or_default().push(span);
    }
    for group in siblings.values_mut() {
        group.sort_by_key(|s| (s.start, s.end));
        for (i, a) in group.iter().enumerate() {
            for b in &group[i + 1..] {
                if b.start > a.end {
                    break;
                }
                conflicts.push(LineageConflict::Overlap {
                    a: a.label.clone(),
                    b: b.label.clone(),
                });
            }
        }
    }
    conflicts
}

/// The chain from `pool_id` up to its root pool (inclusive, child first).
pub fn lineage<'a>(pools: &'a [PoolRecord], pool_id: &str) -> Result<Vec<&'a PoolRecord>> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = pool_id;
    loop {
        if !seen.insert(current) {
            bail!("pool {pool_id} has a cyclic lineage")
        }
        let pool = pools
            .iter()
            .find(|p| p.pool_id == current)
            .with_context(|| format!("pool {current} is not recorded"))?;
        chain.push(pool);
        match pool.parent_pool_id.as_deref() {
            Some(parent) => current = parent,
            None => return Ok(chain),
        }
    }
}

/// Free space of `parent`: its range minus every child pool and block.
pub fn free_ranges(
    parent: &PoolRecord,
    pools: &[PoolRecord],
    blocks: &[AgreedIdBlock],
) -> Vec<(u32, u32)> {
    let mut taken: Vec<(u32, u32)> = spans(pools, blocks)
        .into_iter()
        .filter(|s| s.parent == Some(parent.pool_id.as_str()))
        .map(|s| (s.start.max(parent.range_start), s.end.min(parent.range_end)))
        .filter(|(start, end)| start <= end)
        .collect();
    taken.sort_unstable();
    let mut free = Vec::new();
    let mut cursor = parent.range_start;
    for (start, end) in taken {
        if start > cursor {
            free.push((cursor, start - 1));
        }
        if end >= cursor {
            match end.checked_add(1) {
                Some(next) => cursor = next,
                None => return free,
            }
        }
    }
    if cursor <= parent.range_end {
        free.push((cursor, parent.range_end));
    }
    free
}

/// Why a clone cannot dispense from a block of one type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unusable {
    NoUpstream,
    NoBlock(String),
    NoLineage(String),
    WrongUpstream {
        type_prefix: String,
        home_ref: String,
        upstream: String,
    },
    Exhausted(String),
    Conflict(String),
}

impl std::fmt::Display for Unusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoUpstream => write!(f, "no upstream is declared for this clone (run `aida node upstream <remote>`)"),
            Self::NoBlock(t) => write!(f, "this clone holds no {t} block from its upstream yet"),
            Self::NoLineage(t) => write!(f, "this clone's {t} blocks have no lineage to the root"),
            Self::WrongUpstream { type_prefix, home_ref, upstream } => write!(
                f,
                "this clone's {type_prefix} block was granted through `{home_ref}`, not its declared upstream `{upstream}`"
            ),
            Self::Exhausted(t) => write!(f, "this clone's {t} block is exhausted and no new block has been claimed"),
            Self::Conflict(detail) => write!(f, "the block lineage records conflict ({detail}); run `aida node show`"),
        }
    }
}

/// The remote a `home_ref` such as `gitlab/aida-store` lives on.
pub fn home_remote(home_ref: &str) -> &str {
    home_ref
        .split_once('/')
        .map_or(home_ref, |(remote, _)| remote)
}

/// Inputs to the spendability check, as read from one store and one clone.
pub struct LineageView<'a> {
    pub pools: &'a [PoolRecord],
    pub blocks: &'a [AgreedIdBlock],
    pub root: Option<&'a RootRecord>,
    pub node: &'a str,
    pub role: AllocationRole,
    pub upstream: Option<&'a str>,
}

impl LineageView<'_> {
    /// Index of the block this clone may dispense `type_prefix` from: bound
    /// to this node, carved from a pool whose chain reaches a root pool held
    /// by the root, and (off the root) granted through the declared
    /// upstream. A block merely present in an imported store never passes.
    pub fn spendable_block(&self, type_prefix: &str) -> Result<usize, Unusable> {
        let typ = type_prefix.to_ascii_uppercase();
        let mut reason = Unusable::NoBlock(typ.clone());
        for (i, block) in self.blocks.iter().enumerate() {
            if block.node_id != self.node || !block.type_prefix.eq_ignore_ascii_case(&typ) {
                continue;
            }
            match self.block_status(block) {
                Ok(()) => return Ok(i),
                Err(e @ (Unusable::NoUpstream | Unusable::Conflict(_))) => return Err(e),
                // Prefer the most specific reason among this node's blocks.
                Err(e) => {
                    if !matches!(
                        reason,
                        Unusable::Exhausted(_) | Unusable::WrongUpstream { .. }
                    ) {
                        reason = e;
                    }
                }
            }
        }
        if self.role == AllocationRole::Node && self.upstream.is_none() {
            return Err(Unusable::NoUpstream);
        }
        Err(reason)
    }

    /// Whether this clone may dispense from `block` now, and if not, why.
    pub fn block_status(&self, block: &AgreedIdBlock) -> Result<(), Unusable> {
        let typ = block.type_prefix.to_ascii_uppercase();
        if self.role == AllocationRole::Node && self.upstream.is_none() {
            return Err(Unusable::NoUpstream);
        }
        if block.node_id != self.node {
            return Err(Unusable::NoBlock(typ));
        }
        self.block_lineage(block, &typ)?;
        let conflicts: Vec<String> = check_invariants(self.pools, self.blocks, self.root)
            .iter()
            .map(ToString::to_string)
            .collect();
        if !conflicts.is_empty() {
            return Err(Unusable::Conflict(conflicts.join("; ")));
        }
        if block.is_exhausted() {
            return Err(Unusable::Exhausted(typ));
        }
        Ok(())
    }

    /// The block's pool chain must reach a root pool held by the store's
    /// root, and on a non-root node the block's immediate parent pool must
    /// live on the declared upstream. Slice 1 checks only that immediate
    /// parent's `home_ref`; the ancestors' `home_ref`s are not yet walked
    /// against the upstream's own upstream chain.
    fn block_lineage(&self, block: &AgreedIdBlock, typ: &str) -> Result<(), Unusable> {
        let no_lineage = || Unusable::NoLineage(typ.to_string());
        let pool_id = block.pool_id.as_deref().ok_or_else(no_lineage)?;
        let chain = lineage(self.pools, pool_id).map_err(|_| no_lineage())?;
        let root_pool = chain.last().ok_or_else(no_lineage)?;
        if self
            .root
            .is_none_or(|r| r.holder_node != root_pool.holder_node)
        {
            return Err(no_lineage());
        }
        if self.role == AllocationRole::Node {
            let upstream = self.upstream.unwrap_or_default();
            let parent = chain[0];
            if home_remote(&parent.home_ref) != upstream {
                return Err(Unusable::WrongUpstream {
                    type_prefix: typ.to_string(),
                    home_ref: parent.home_ref.clone(),
                    upstream: upstream.to_string(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/allocation_lineage_tests.rs"]
mod tests;
