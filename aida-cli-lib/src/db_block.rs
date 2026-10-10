//! handle_db_check_collisions, handle_block_command, block auto-allocation (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Audit the store for pre-existing agreed-id collisions — two requirements
/// claiming the same short id across their (spec_id, agreed_id) pair.
///
/// Background: BUG-82's gate-time guard prevents NEW collisions, but the
/// 5 pre-existing collisions surfaced by PR-12 (TASK-31, TASK-32, TASK-33,
/// TASK-34, BUG-34) persist in the store with no audit surface. This
/// reports each collision with all claimants' (spec_id, agreed_id, title,
/// status) so the operator can decide which to keep and re-gate the rest.
///
/// `--repair` is currently a not-yet-implemented placeholder; resolving
/// a collision typically means picking a winner manually because
/// "automatically re-gate the later claimant" interacts with the
/// pre-allocated block registry (FR-2-005) in ways that need policy
/// decisions, not just code. trace:TASK-80 | ai:claude
pub(crate) fn handle_db_check_collisions(
    backend: &aida_core::CachedGitBackend,
    _store_path: &std::path::Path,
    repair: bool,
) -> Result<()> {
    use aida_core::DatabaseBackend;

    let store = backend.load()?;

    // For each requirement, collect the set of short-id strings it claims:
    // its spec_id and its agreed_id (if any, and not equal to spec_id).
    // Then invert: short_id → set of claimant UUIDs. A short_id claimed by
    // more than one distinct requirement is a collision.
    let mut claims: std::collections::BTreeMap<String, Vec<uuid::Uuid>> =
        std::collections::BTreeMap::new();
    for req in &store.requirements {
        let mut seen_for_this_req: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for id in [req.spec_id.as_deref(), req.agreed_id.as_deref()]
            .into_iter()
            .flatten()
        {
            if seen_for_this_req.insert(id.to_string()) {
                claims.entry(id.to_string()).or_default().push(req.id);
            }
        }
    }

    // A "collision" is a short_id claimed by 2+ distinct requirements.
    let collisions: Vec<(&String, &Vec<uuid::Uuid>)> = claims
        .iter()
        .filter(|(_, claimants)| claimants.len() > 1)
        .collect();

    if collisions.is_empty() {
        println!(
            "{} No agreed-id collisions found ({} requirements scanned).",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            store.requirements.len()
        );
        return Ok(());
    }

    println!(
        "{} {} collision{} found across {} requirements:",
        crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
        collisions.len(),
        if collisions.len() == 1 { "" } else { "s" },
        store.requirements.len()
    );
    println!("{}", "─".repeat(72));

    for (short_id, claimants) in &collisions {
        println!();
        println!("  {} {}", "Short ID:".bold(), short_id.yellow().bold());
        for uuid in claimants.iter() {
            let req = store.requirements.iter().find(|r| r.id == *uuid);
            let Some(req) = req else {
                continue;
            };
            let spec = req.spec_id.as_deref().unwrap_or("?");
            let agreed = req
                .agreed_id
                .as_deref()
                .map(|a| format!(", agreed_id={}", a))
                .unwrap_or_default();
            println!(
                "    {} {}{}  —  {}  ({})",
                "Claim:".dimmed(),
                spec.cyan(),
                agreed.dimmed(),
                req.title,
                format!("{}", req.status).dimmed()
            );
        }
    }

    println!();
    println!("{}", "─".repeat(72));
    println!(
        "  {} pick one claimant to keep the contested short id; for the others,",
        "Action:".bold()
    );
    println!("  edit the YAML to clear or rewrite `agreed_id`, then re-run `aida db merge-gate`.");

    if repair {
        println!();
        println!(
            "  {} `--repair` is not yet implemented — automatic re-gating",
            "Note:".yellow().bold()
        );
        println!("  interacts with the pre-allocated block registry (FR-2-005) in ways");
        println!("  that need policy decisions (which claimant wins, which block to draw");
        println!("  from for the loser). For now, resolve manually using the action above.");
    }

    // Non-zero exit so CI / hooks can wire this in. trace:TASK-80 | ai:claude
    std::process::exit(1);
}

/// One row of `aida db block list` after contiguous same-node/type/owner
/// sub-blocks have been merged. The `Next`/`Remaining` of the row come from
/// the live frontier sub-block (the first non-exhausted one); when every
/// merged sub-block is exhausted the row is `full` (`remaining == 0`).
// trace:TASK-950 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergedBlockRow {
    pub(crate) node_id: String,
    pub(crate) owner: String,
    pub(crate) type_prefix: String,
    pub(crate) range_start: u32,
    pub(crate) range_end: u32,
    /// Next id to dispense, from the live frontier sub-block.
    pub(crate) next: u32,
    /// Remaining ids in the live frontier sub-block; 0 means the whole
    /// merged span is exhausted (rendered as `full`).
    pub(crate) remaining: u32,
}

/// Merge contiguous agreed-id blocks for `aida db block list`.
///
/// Groups by `(node_id, type_prefix, owner)`, sorts each group by
/// `range_start`, and folds runs of blocks where `prev.range_end + 1 ==
/// next.range_start` into a single [`MergedBlockRow`] spanning
/// `first.range_start..=last.range_end`. A gap, or a different
/// node/type/owner, starts a new row.
///
/// The merged row's `next`/`remaining` are taken from the LIVE frontier:
/// the first sub-block in the run that still has ids to dispense
/// (`remaining() > 0`). If every sub-block in the run is exhausted the row
/// reports `remaining == 0` (the caller renders this as `full`) and `next`
/// is carried from the last sub-block.
///
/// Pure: input order within a group does not matter (it is sorted); the
/// returned rows preserve first-seen group order so the listing stays
/// stable across runs.
// trace:TASK-950 | ai:claude
pub(crate) fn merge_contiguous_blocks(blocks: &[aida_core::AgreedIdBlock]) -> Vec<MergedBlockRow> {
    use std::collections::BTreeMap;

    // Preserve first-seen order of groups for a stable listing.
    let mut group_order: Vec<(String, String, String)> = Vec::new();
    let mut groups: BTreeMap<(String, String, String), Vec<&aida_core::AgreedIdBlock>> =
        BTreeMap::new();
    for b in blocks {
        let key = (b.node_id.clone(), b.type_prefix.clone(), b.owner.clone());
        if !groups.contains_key(&key) {
            group_order.push(key.clone());
        }
        groups.entry(key).or_default().push(b);
    }

    let mut rows: Vec<MergedBlockRow> = Vec::new();
    for key in &group_order {
        let mut members = groups.remove(key).unwrap_or_default();
        members.sort_by_key(|b| b.range_start);

        let mut run: Vec<&aida_core::AgreedIdBlock> = Vec::new();
        for b in members {
            match run.last() {
                Some(prev) if prev.range_end + 1 == b.range_start => run.push(b),
                Some(_) => {
                    rows.push(fold_run(&run));
                    run = vec![b];
                }
                None => run.push(b),
            }
        }
        if !run.is_empty() {
            rows.push(fold_run(&run));
        }
    }
    rows
}

/// Fold a contiguous run of sub-blocks into a single [`MergedBlockRow`].
/// `run` must be non-empty and sorted ascending by `range_start`.
// trace:TASK-950 | ai:claude
pub(crate) fn fold_run(run: &[&aida_core::AgreedIdBlock]) -> MergedBlockRow {
    let first = run[0];
    let last = run[run.len() - 1];
    // Live frontier: first sub-block with ids left; else the last (exhausted).
    let frontier = run
        .iter()
        .find(|b| !b.is_exhausted())
        .copied()
        .unwrap_or(last);
    MergedBlockRow {
        node_id: first.node_id.clone(),
        owner: first.owner.clone(),
        type_prefix: first.type_prefix.clone(),
        range_start: first.range_start,
        range_end: last.range_end,
        next: frontier.next,
        remaining: frontier.remaining(),
    }
}

pub(crate) fn handle_block_command(cmd: &BlockCommand, store_path: &std::path::Path) -> Result<()> {
    use aida_core::BlockRegistry;

    let blocks_path = store_path.join("registry").join("blocks.yaml");
    let node_id = load_node_id(store_path);

    match cmd {
        BlockCommand::Claim { r#type, size } => {
            let type_prefix = r#type.to_uppercase();

            // Load or create registry, push-wins loop
            let max_retries = 3;
            for attempt in 0..max_retries {
                // Pull before modifying to reduce races
                let branch = aida_core::git_ops::current_branch(store_path)
                    .unwrap_or_else(|_| "aida-store".to_string());
                if attempt > 0 {
                    let _ = aida_core::git_ops::pull_rebase(store_path, "origin", &branch);
                }

                let mut registry = BlockRegistry::load(&blocks_path)?;
                // Honor the agreed-id counter floor so a fresh block can't
                // start at a number already issued in past merge-gate runs
                // or retire-legacy-ids migrations.
                // trace:FR-1-073 | ai:claude
                let counter_floor = read_agreed_counter(store_path, &type_prefix);
                // BUG-715: redact the hostname before it lands in blocks.yaml.
                let (block_host, _) = aida_core::git_ops::redacted_identity(&hostname(), None);
                let block = registry.claim_block_with_floor(
                    node_id.clone(),
                    std::env::var("USER").unwrap_or_else(|_| "unknown".into()),
                    block_host,
                    type_prefix.clone(),
                    *size,
                    counter_floor,
                );
                registry.save(&blocks_path)?;

                // Commit the new block
                aida_core::git_ops::add(store_path, &["registry/blocks.yaml"])?;
                aida_core::git_ops::commit(
                    store_path,
                    &format!(
                        "chore: claim {}-{}..{} for node {}",
                        type_prefix, block.range_start, block.range_end, node_id
                    ),
                )?;

                // Push — retry on rejection
                match aida_core::git_ops::push(store_path, "origin", &branch) {
                    Ok(true) => {
                        println!(
                            "{} Claimed {}-{}..{} for node {} ({})",
                            "".green().bold(),
                            type_prefix,
                            block.range_start,
                            block.range_end,
                            node_id,
                            block.owner
                        );
                        return Ok(());
                    }
                    Ok(false) => {
                        // Push rejected — undo commit, pull, retry
                        let _ = std::process::Command::new("git")
                            .args(["reset", "--soft", "HEAD~1"])
                            .current_dir(store_path)
                            .output();
                        if attempt + 1 == max_retries {
                            anyhow::bail!(
                                "Could not push block claim after {} attempts. Run `aida db sync --pull` and retry.",
                                max_retries
                            );
                        }
                        eprintln!(
                            "{} Push rejected (concurrent claim), retrying ({}/{})...",
                            "!".yellow(),
                            attempt + 1,
                            max_retries
                        );
                    }
                    Err(e) => anyhow::bail!("Push failed: {}", e),
                }
            }
            anyhow::bail!("Failed to claim block after {} attempts", max_retries);
        }

        BlockCommand::List => {
            let registry = BlockRegistry::load(&blocks_path)?;
            if registry.blocks.is_empty() {
                println!("No blocks claimed yet. Run `aida db block claim` to allocate one.");
                return Ok(());
            }

            // TASK-845: resolve each block's owner to its CANONICAL person on an
            // in-memory copy before merging, so a person's blocks claimed under
            // several owner strings across hosts (case-variants via TASK-951
            // plus the operator-curated alias map) group/show under ONE owner.
            // The stored `blocks.yaml` owner strings are left untouched — this is
            // a display-only normalization (like the queue/team resolution).
            let aliases = aida_core::alias::AliasRegistry::load(store_path);
            let display_blocks: Vec<aida_core::AgreedIdBlock> = registry
                .blocks
                .iter()
                .cloned()
                .map(|mut b| {
                    b.owner = aliases.resolve(&b.owner);
                    b
                })
                .collect();

            // Merge contiguous same-node/type/owner sub-blocks into one row.
            // trace:TASK-950 trace:TASK-845 | ai:claude
            let rows = merge_contiguous_blocks(&display_blocks);

            // Build the plain cell text first so per-column max widths are
            // computed without ANSI color codes throwing off the math; the
            // Remaining color is applied after padding. trace:TASK-950 | ai:claude
            struct Cells {
                node: String,
                ty: String,
                range: String,
                next: String,
                // Plain (uncolored) remaining text used for width; "full"
                // when the merged span is exhausted. trace:TASK-950 | ai:claude
                remaining: String,
                is_full: bool,
                is_low: bool,
                owner: String,
            }
            let cells: Vec<Cells> = rows
                .iter()
                .map(|r| {
                    let is_full = r.remaining == 0;
                    Cells {
                        node: r.node_id.clone(),
                        ty: r.type_prefix.clone(),
                        range: format!("{}-{}..{}", r.type_prefix, r.range_start, r.range_end),
                        next: format!("{}-{}", r.type_prefix, r.next),
                        remaining: if is_full {
                            "full".to_string()
                        } else {
                            r.remaining.to_string()
                        },
                        is_full,
                        is_low: !is_full && r.remaining <= aida_core::AgreedIdBlock::LOW_THRESHOLD,
                        owner: r.owner.clone(),
                    }
                })
                .collect();

            // Per-column max width, including the header label.
            let w_node = cells
                .iter()
                .map(|c| c.node.len())
                .chain(["Node".len()])
                .max()
                .unwrap_or(0);
            let w_type = cells
                .iter()
                .map(|c| c.ty.len())
                .chain(["Type".len()])
                .max()
                .unwrap_or(0);
            let w_range = cells
                .iter()
                .map(|c| c.range.len())
                .chain(["Range".len()])
                .max()
                .unwrap_or(0);
            let w_next = cells
                .iter()
                .map(|c| c.next.len())
                .chain(["Next".len()])
                .max()
                .unwrap_or(0);
            let w_rem = cells
                .iter()
                .map(|c| c.remaining.len())
                .chain(["Remaining".len()])
                .max()
                .unwrap_or(0);

            println!(
                "{:<w_node$}  {:<w_type$}  {:<w_range$}  {:<w_next$}  {:>w_rem$}  Owner",
                "Node", "Type", "Range", "Next", "Remaining",
            );
            let rule_width = w_node + w_type + w_range + w_next + w_rem + "Owner".len() + 2 * 5;
            println!("{}", "─".repeat(rule_width));
            for c in &cells {
                // Pad the plain remaining text to width, THEN color, so the
                // ANSI codes never affect alignment. trace:TASK-950 | ai:claude
                let remaining_padded = format!("{:>w_rem$}", c.remaining);
                let remaining_str = if c.is_full {
                    remaining_padded.red().to_string()
                } else if c.is_low {
                    remaining_padded.yellow().to_string()
                } else {
                    remaining_padded
                };
                println!(
                    "{:<w_node$}  {:<w_type$}  {:<w_range$}  {:<w_next$}  {}  {}",
                    c.node, c.ty, c.range, c.next, remaining_str, c.owner,
                );
            }
        }

        BlockCommand::Status => {
            let registry = BlockRegistry::load(&blocks_path)?;
            let my_blocks: Vec<&aida_core::AgreedIdBlock> = registry
                .blocks
                .iter()
                .filter(|b| b.node_id == node_id)
                .collect();
            // trace:TASK-444 | ai:claude — surface the effective auto-claim
            // knobs (`[block_allocation.<type>] auto_claim_threshold /
            // auto_claim_size`) on each per-type line so users see what
            // their threshold/size actually resolve to, not just the
            // remaining count.
            let project_dir = store_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let alloc_cfg = read_block_allocation_config(&project_dir)?;
            if my_blocks.is_empty() {
                // trace:TASK-449 | ai:claude — empty-blocks is the case where
                // a user benefits most from knowing what their first claim
                // will look like (and whether `[block_allocation]` is wired
                // up at all). Print the global summary as a continuation.
                println!(
                    "No blocks for node {}. Run `aida db block claim` to allocate one.",
                    node_id
                );
                println!("  {}", global_auto_claim_summary(&alloc_cfg).dimmed());
                return Ok(());
            }
            println!("Blocks for node {}:", node_id);
            println!("{}", "─".repeat(50));

            // BUG-115: group by type prefix and report the aggregate
            // remaining across all the user's non-exhausted blocks for
            // that type. Pre-fix, the warning fired on the lowest-
            // numbered block's remaining even when a higher block with
            // full capacity had already been claimed — nagging the user
            // on every `aida add` despite plenty of headroom.
            // trace:BUG-115 | ai:claude
            use std::collections::BTreeMap;
            let mut by_prefix: BTreeMap<String, Vec<&aida_core::AgreedIdBlock>> = BTreeMap::new();
            for b in &my_blocks {
                by_prefix
                    .entry(b.type_prefix.to_uppercase())
                    .or_default()
                    .push(*b);
            }
            for (prefix, blocks) in &by_prefix {
                let mut active: Vec<&aida_core::AgreedIdBlock> = blocks
                    .iter()
                    .copied()
                    .filter(|b| !b.is_exhausted())
                    .collect();
                active.sort_by_key(|b| b.range_start);

                if active.is_empty() {
                    let last = blocks
                        .iter()
                        .copied()
                        .max_by_key(|b| b.range_end)
                        .expect("non-empty by construction");
                    let range = format!("{}-{}..{}", prefix, last.range_start, last.range_end);
                    println!(
                        "  {} {}: {} (exhausted — run `aida db block claim --type {}`)",
                        "".red(),
                        prefix,
                        range,
                        prefix
                    );
                    println!("       {}", auto_claim_summary(&alloc_cfg, prefix).dimmed());
                    continue;
                }

                let aggregate: u32 = active.iter().map(|b| b.remaining()).sum();
                let ranges = active
                    .iter()
                    .map(|b| format!("{}-{}..{}", prefix, b.range_start, b.range_end))
                    .collect::<Vec<_>>()
                    .join(", ");
                let span = if active.len() == 1 {
                    format!("({})", ranges)
                } else {
                    format!("across {} blocks ({})", active.len(), ranges)
                };

                if aggregate <= aida_core::BlockRegistry::AGGREGATE_LOW_THRESHOLD {
                    println!(
                        "  {} {}: {} remaining {} — {} Low, claim soon",
                        "".yellow(),
                        prefix,
                        aggregate,
                        span,
                        "WARNING:".yellow().bold()
                    );
                } else {
                    println!(
                        "  {} {}: {} remaining {}",
                        "".green(),
                        prefix,
                        aggregate,
                        span
                    );
                }
                println!("       {}", auto_claim_summary(&alloc_cfg, prefix).dimmed());
            }
        }
        // FR-281: cross-check nodes.toml against blocks.yaml.
        // trace:FR-281 | ai:claude
        BlockCommand::Verify => {
            use aida_core::NodeRegistry;
            let nodes_path = store_path.join("registry").join("nodes.toml");
            let blocks_registry = BlockRegistry::load(&blocks_path).unwrap_or_default();
            let nodes_registry = NodeRegistry::load(&nodes_path).unwrap_or_default();

            // Only active (non-exhausted) blocks count as "owning" a range.
            // Tombstoned blocks (post `aida doctor repair-stale-blocks`)
            // intentionally have an unregistered owner — that's the repair
            // outcome, not a problem to flag.
            let block_owners: std::collections::HashSet<String> = blocks_registry
                .blocks
                .iter()
                .filter(|b| !b.is_exhausted())
                .map(|b| b.node_id.clone())
                .collect();
            let registered: std::collections::HashSet<String> =
                nodes_registry.nodes.iter().map(|n| n.id.clone()).collect();

            let blocks_without_node: Vec<&str> = block_owners
                .iter()
                .filter(|id| !registered.contains(*id))
                .map(|s| s.as_str())
                .collect();
            let nodes_without_block: Vec<&str> = registered
                .iter()
                .filter(|id| !block_owners.contains(*id))
                .map(|s| s.as_str())
                .collect();

            // Whether to flag nodes_without_block as a problem depends on
            // policy. Under blocks-only it's a hard error; under blocks-
            // then-fallback it's just informational.
            let project_dir = std::env::current_dir().unwrap_or_default();
            let policy = read_id_format_policy(&project_dir);
            let nodes_without_block_is_error = policy.requires_block();

            let mut had_error = false;
            println!("{}", "Block registry consistency check".bold());
            println!("  policy: {}", policy.as_str());
            println!("  registered nodes: {}", registered.len());
            println!("  block-owning nodes: {}", block_owners.len());
            println!();

            if blocks_without_node.is_empty() && nodes_without_block.is_empty() {
                println!(
                    "{} consistent — every block has a registered node, every node has a block.",
                    crate::glyph(crate::glyphs::Glyph::Check).green().bold()
                );
                return Ok(());
            }

            if !blocks_without_node.is_empty() {
                had_error = true;
                println!(
                    "{} {} block-owning node{} not in registry/nodes.toml:",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    blocks_without_node.len(),
                    if blocks_without_node.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                );
                let mut sorted = blocks_without_node.clone();
                sorted.sort();
                for id in &sorted {
                    let blocks_for_id: Vec<String> = blocks_registry
                        .blocks
                        .iter()
                        .filter(|b| &b.node_id.as_str() == id)
                        .map(|b| format!("{}-{}..{}", b.type_prefix, b.range_start, b.range_end))
                        .collect();
                    println!(
                        "    - node `{}` owns {} block(s): {}",
                        id,
                        blocks_for_id.len(),
                        blocks_for_id.join(", ")
                    );
                }
                println!(
                    "  {} run `aida node acquire --id {}` from that clone, OR \
                     `aida node release {}` to free the orphaned blocks.",
                    "Fix:".yellow().bold(),
                    sorted[0],
                    sorted[0]
                );
                println!();
            }

            if !nodes_without_block.is_empty() {
                if nodes_without_block_is_error {
                    had_error = true;
                    println!(
                        "{} {} registered node{} with no claimed block (blocks-only policy):",
                        crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                        nodes_without_block.len(),
                        if nodes_without_block.len() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    );
                } else {
                    println!(
                        "{} {} registered node{} with no claimed block (allowed under `{}`):",
                        "·".dimmed(),
                        nodes_without_block.len(),
                        if nodes_without_block.len() == 1 {
                            ""
                        } else {
                            "s"
                        },
                        policy.as_str()
                    );
                }
                let mut sorted = nodes_without_block.clone();
                sorted.sort();
                for id in &sorted {
                    println!("    - node `{}`", id);
                }
                if nodes_without_block_is_error {
                    println!(
                        "  {} run `aida db block claim --type FR --size 100` from each \
                         affected clone (per type) to allocate.",
                        "Fix:".yellow().bold()
                    );
                }
                println!();
            }

            if had_error {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

/// Common requirement types that get auto-allocated blocks on `aida node
/// acquire` (Phase 3 of EPIC-1-052). Without these, new reqs of these
/// types fall through to the node-aware form (`TASK-1-019`) and require
/// `aida db merge-gate` to promote them — friction the user shouldn't
/// have to think about. Includes the five docs-layer types from FR-1-074
/// so new clones get short ADR-1, PRIN-1, VIS-1, etc., out of the box.
/// trace:FR-1-073 | ai:claude
/// trace:FR-1-074 | ai:claude
pub(crate) const PHASE3_AUTO_ALLOC_TYPES: &[&str] = &[
    "FR", "BUG", "TASK", "EPIC", "STORY", "SPIKE", "PRIN", "VIS", "CON", "ADR", "TERM",
];

/// Auto-allocate initial blocks for a freshly-acquired node. Claims one
/// block per common type that doesn't already have one for this node.
/// Returns a vector of allocated range labels (e.g., `["FR-101..200",
/// "BUG-1..100"]`), or an empty vec if every type already had a block
/// (idempotent — safe to re-run).
///
/// Default block size is 100. Each block claim goes through its own CAS
/// push loop so a stray contention on one type doesn't block the others.
/// trace:EPIC-1-052 Phase 3 | ai:claude
/// trace:FR-1-073 | ai:claude
pub(crate) fn auto_allocate_initial_blocks(
    store_path: &std::path::Path,
    node_id: &str,
    hn: &str,
    email: Option<&str>,
) -> Result<Vec<String>> {
    // Read counter_scope from config.toml. When called from `aida init`,
    // the config might not exist yet (init writes it later in the flow);
    // the explicit-scope variant below is the right call there.
    let project_dir = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let scope = read_id_counter_scope(&project_dir);
    auto_allocate_initial_blocks_with_scope(store_path, node_id, hn, email, scope)
}

/// Same as `auto_allocate_initial_blocks` but with an explicit scope —
/// for use by `aida init` which decides scope before writing config.toml.
/// trace:FR-271 | ai:claude
pub(crate) fn auto_allocate_initial_blocks_with_scope(
    store_path: &std::path::Path,
    node_id: &str,
    hn: &str,
    email: Option<&str>,
    scope: aida_core::IdCounterScope,
) -> Result<Vec<String>> {
    let mut allocated = Vec::new();
    if scope == aida_core::IdCounterScope::Global {
        // One shared block per node, size 1000. Dispense formats with
        // the caller-requested type prefix at dispense time.
        if let Some(label) = auto_allocate_block_with_size(
            store_path,
            node_id,
            hn,
            email,
            aida_core::IdCounterScope::GLOBAL_TYPE_PREFIX,
            1000,
        )? {
            allocated.push(label);
        }
        return Ok(allocated);
    }

    for type_prefix in PHASE3_AUTO_ALLOC_TYPES {
        if let Some(label) =
            auto_allocate_block_for_type(store_path, node_id, hn, email, type_prefix)?
        {
            allocated.push(label);
        }
    }
    Ok(allocated)
}

/// Why a block is being claimed — controls the idempotency guard and the
/// commit message in `auto_allocate_block_inner`.
///
/// trace:TASK-281 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockClaimReason {
    /// Initial allocation on `aida node acquire` — idempotent. Returns
    /// None if an active block already exists for this (node, type).
    OnAcquire,
    /// Refill on auto-claim threshold cross — explicitly claims another
    /// block alongside any existing active block.
    OnThresholdCross,
}

/// Like `auto_allocate_block_for_type` but with an explicit size — used
/// by the Global counter-scope path which wants a larger shared block.
/// trace:FR-271 | ai:claude
pub(crate) fn auto_allocate_block_with_size(
    store_path: &std::path::Path,
    node_id: &str,
    hn: &str,
    email: Option<&str>,
    type_prefix: &str,
    size: u32,
) -> Result<Option<String>> {
    auto_allocate_block_inner(
        store_path,
        node_id,
        hn,
        email,
        type_prefix,
        size,
        BlockClaimReason::OnAcquire,
    )
}

/// Allocate a single block for the given (node_id, type_prefix) if one
/// doesn't already exist. Returns Some("<TYPE>-<start>..<end>") on a fresh
/// claim, None if the node already had a block for that type.
/// trace:FR-1-073 | ai:claude
pub(crate) fn auto_allocate_block_for_type(
    store_path: &std::path::Path,
    node_id: &str,
    hn: &str,
    email: Option<&str>,
    type_prefix: &str,
) -> Result<Option<String>> {
    auto_allocate_block_inner(
        store_path,
        node_id,
        hn,
        email,
        type_prefix,
        100,
        BlockClaimReason::OnAcquire,
    )
}

/// Shared CAS-loop allocator. Size differs by caller: per-type defaults
/// to 100; global scope uses 1000. The label format `<TYPE>-<start>..<end>`
/// is preserved verbatim so existing user-facing output looks unchanged
/// under per-type, and the global label reads as `*-1..1000` (a clear
/// visual signal that this is the shared block).
///
/// `reason = OnAcquire` skips when an active block already exists (the
/// init-time idempotency contract). `reason = OnThresholdCross` always
/// claims a fresh block alongside any existing ones (the TASK-281 auto-
/// claim refill).
///
/// trace:FR-271 | ai:claude
/// trace:TASK-281 | ai:claude
pub(crate) fn auto_allocate_block_inner(
    store_path: &std::path::Path,
    node_id: &str,
    hn: &str,
    email: Option<&str>,
    type_prefix: &str,
    size: u32,
    reason: BlockClaimReason,
) -> Result<Option<String>> {
    use aida_core::BlockRegistry;

    let blocks_path = store_path.join("registry").join("blocks.yaml");
    let owner = email
        .map(|e| e.to_string())
        .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "unknown".into()));

    // BUG-40: same local-only short-circuit as register_node_full —
    // claim the block on the local orphan branch and let the next
    // `aida push` upload it.
    let local_only = !aida_core::git_ops::has_remote(store_path, "origin");

    let max_retries = 3;
    for attempt in 0..max_retries {
        if attempt > 0 && !local_only {
            let branch = aida_core::git_ops::current_branch(store_path)
                .unwrap_or_else(|_| "aida-store".to_string());
            let _ = aida_core::git_ops::pull_rebase(store_path, "origin", &branch);
        }

        let mut registry = BlockRegistry::load(&blocks_path)?;
        if reason == BlockClaimReason::OnAcquire
            && registry.find_active_block(node_id, type_prefix).is_some()
        {
            return Ok(None);
        }

        let counter_floor = read_agreed_counter(store_path, type_prefix);
        // BUG-715: redact owner (email) + hostname before they land in blocks.yaml.
        let raw_owner = owner.to_string();
        let raw_hn = hn.to_string();
        let (block_host, block_owner) =
            aida_core::git_ops::redacted_identity(&raw_hn, Some(raw_owner.as_str()));
        let block = registry.claim_block_with_floor(
            node_id.to_string(),
            block_owner.unwrap_or(raw_owner),
            block_host,
            type_prefix.to_string(),
            size,
            counter_floor,
        );
        registry.save(&blocks_path)?;

        let commit_subject = match reason {
            BlockClaimReason::OnAcquire => format!(
                "chore(registry): auto-allocate {}-{}..{} for node {} on acquire",
                type_prefix, block.range_start, block.range_end, node_id
            ),
            BlockClaimReason::OnThresholdCross => format!(
                "chore(registry): auto-claim {}-{}..{} for node {} on threshold cross",
                type_prefix, block.range_start, block.range_end, node_id
            ),
        };
        aida_core::git_ops::add(store_path, &["registry/blocks.yaml"])?;
        aida_core::git_ops::commit(store_path, &commit_subject)?;

        if local_only {
            return Ok(Some(format!(
                "{}-{}..{}",
                type_prefix, block.range_start, block.range_end
            )));
        }

        let branch = aida_core::git_ops::current_branch(store_path)
            .unwrap_or_else(|_| "aida-store".to_string());
        match aida_core::git_ops::push(store_path, "origin", &branch) {
            Ok(true) => {
                return Ok(Some(format!(
                    "{}-{}..{}",
                    type_prefix, block.range_start, block.range_end
                )));
            }
            Ok(false) => {
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(store_path)
                    .output();
                continue;
            }
            Err(e) => anyhow::bail!("Push failed: {}", e),
        }
    }
    anyhow::bail!(
        "could not push block claim for {} after {} attempts",
        type_prefix,
        max_retries
    );
}

/// Read the `[block_allocation]` section (and any `[block_allocation.<type>]`
/// subsections) from `.aida/config.toml`. Returns the project's auto-claim
/// defaults when the file or section is absent.
///
/// trace:TASK-281 | ai:claude
pub(crate) fn read_block_allocation_config(
    project_dir: &std::path::Path,
) -> Result<aida_core::BlockAllocationConfig> {
    let config_path = project_dir.join(".aida").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&config_path) else {
        return Ok(aida_core::BlockAllocationConfig::default());
    };
    let value = content.parse::<toml::Value>().map_err(|err| {
        anyhow::anyhow!(
            "{}",
            config_parse_error_message(&config_path, &content, &err)
        )
    })?;
    let Some(section) = value.get("block_allocation").and_then(|v| v.as_table()) else {
        return Ok(aida_core::BlockAllocationConfig::default());
    };

    let mut cfg = aida_core::BlockAllocationConfig::default();
    if let Some(b) = section.get("auto_claim").and_then(|v| v.as_bool()) {
        cfg.auto_claim = b;
    }
    for (key, subval) in section {
        let Some(sub) = subval.as_table() else {
            continue;
        };
        let mut tcfg = aida_core::BlockAllocationTypeConfig::default();
        if let Some(b) = sub.get("auto_claim").and_then(|v| v.as_bool()) {
            tcfg.auto_claim = Some(b);
        }
        if let Some(n) = sub.get("auto_claim_threshold").and_then(|v| v.as_integer()) {
            if n >= 0 {
                tcfg.auto_claim_threshold = Some(n as u32);
            }
        }
        if let Some(n) = sub.get("auto_claim_size").and_then(|v| v.as_integer()) {
            if n > 0 {
                tcfg.auto_claim_size = Some(n as u32);
            }
        }
        cfg.per_type.insert(key.to_ascii_lowercase(), tcfg);
    }
    Ok(cfg)
}

/// Human-readable one-liner describing the effective auto-claim policy
/// for `type_prefix`, used as the continuation line under each per-type
/// row of `aida db block status` (TASK-444). Resolves the effective
/// threshold/size, marks `(configured)` when any per-type override is in
/// effect, and renders the off-cases with the reason (`global opt-out`
/// vs `per-type opt-out`) so a user troubleshooting "why didn't a fresh
/// block claim?" doesn't have to grep `.aida/config.toml` to find out.
///
/// trace:TASK-444 | ai:claude
pub(crate) fn auto_claim_summary(
    cfg: &aida_core::BlockAllocationConfig,
    type_prefix: &str,
) -> String {
    if !cfg.is_enabled_for(type_prefix) {
        if !cfg.auto_claim {
            return "auto-claim: off (global opt-out)".to_string();
        }
        return "auto-claim: off (per-type opt-out)".to_string();
    }
    // trace:TASK-448 | ai:claude
    let has_override = cfg
        .per_type
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(type_prefix))
        .map(|(_, t)| t.auto_claim_threshold.is_some() || t.auto_claim_size.is_some())
        .unwrap_or(false);
    let suffix = if has_override { " (configured)" } else { "" };
    format!(
        "auto-claim: threshold {}, size {}{}",
        cfg.threshold_for(type_prefix),
        cfg.size_for(type_prefix),
        suffix
    )
}

/// Type-agnostic counterpart to `auto_claim_summary`, used by the
/// "no blocks for node N" branch of `aida db block status` (TASK-449).
/// At that point no concrete type prefix is in scope, so the summary
/// reflects the effective defaults: any `[block_allocation.*]` override
/// wins over the built-in `DEFAULT_THRESHOLD` / `DEFAULT_SIZE`, and the
/// `(configured)` tag fires whenever the user has any per-type section
/// in `.aida/config.toml` (signal that `[block_allocation]` is wired up).
///
/// trace:TASK-449 | ai:claude
pub(crate) fn global_auto_claim_summary(cfg: &aida_core::BlockAllocationConfig) -> String {
    if !cfg.auto_claim {
        // TASK-467: a per-type re-enable (e.g. `[block_allocation.bug]
        // auto_claim = true` under a global `auto_claim = false`) means
        // auto-claim still fires for those types. The type-agnostic empty-blocks
        // summary can't name them, but it must not report a flat "off" that
        // hides the per-type wiring — surface that it's re-enabled per-type.
        // trace:TASK-467 | ai:claude
        if cfg.per_type.values().any(|t| t.auto_claim == Some(true)) {
            return "auto-claim: off globally (re-enabled per-type)".to_string();
        }
        return "auto-claim: off (global opt-out)".to_string();
    }
    let configured = !cfg.per_type.is_empty();
    let suffix = if configured { " (configured)" } else { "" };
    format!(
        "auto-claim: threshold {}, size {}{}",
        cfg.threshold_for("*"),
        cfg.size_for("*"),
        suffix,
    )
}

/// Outcome of a successful auto-claim — used by `add_requirement_cli` to
/// print the one-line info notice ("Auto-claimed BUG-517..616 (threshold
/// crossed: 18 remaining → 118)"). `previous_remaining` is the aggregate
/// before the claim; `new_remaining` is the aggregate after.
/// trace:TASK-281 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct AutoClaimOutcome {
    pub(crate) label: String,
    pub(crate) previous_remaining: u32,
    pub(crate) new_remaining: u32,
}

/// Ensure the (node, type) pair has at least `cfg.threshold_for(type)`
/// IDs remaining; if not (and auto-claim is enabled per config), claim a
/// fresh block of `cfg.size_for(type)`. Scope-aware: under
/// `IdCounterScope::Global` the check + claim target the shared `*` block.
///
/// Returns:
/// - `Ok(None)` — no action taken (auto-claim disabled OR above threshold).
/// - `Ok(Some(outcome))` — a fresh block was claimed; caller should print
///   the info notice.
/// - `Err(e)` — the claim's push failed after retries. BUG-372 upgraded the
///   `aida add` caller to fail loud instead of dispensing from a potentially
///   stale local block, because continuing there can recreate the cross-clone
///   ID collision that lost specs during the PR-270 pull.
///
/// trace:TASK-281 | ai:claude
pub(crate) fn ensure_block_capacity(
    store_path: &std::path::Path,
    project_dir: &std::path::Path,
    node_id: &str,
    type_prefix: &str,
) -> Result<Option<AutoClaimOutcome>> {
    use aida_core::BlockRegistry;

    let cfg = read_block_allocation_config(project_dir)?;
    let scope = read_id_counter_scope(project_dir);

    // Under Global scope the shared `*` block backs every type; check and
    // claim against it. The user's per-type config sections still drive
    // size/threshold when they explicitly configure `[block_allocation."*"]`.
    let effective_prefix = if scope == aida_core::IdCounterScope::Global {
        aida_core::IdCounterScope::GLOBAL_TYPE_PREFIX.to_string()
    } else {
        type_prefix.to_uppercase()
    };

    if !cfg.is_enabled_for(&effective_prefix) {
        return Ok(None);
    }

    let blocks_path = store_path.join("registry").join("blocks.yaml");
    let registry = BlockRegistry::load(&blocks_path).unwrap_or_default();

    // No active block for this (node, type) at all? Don't auto-claim here —
    // that path is owned by `auto_allocate_initial_blocks` (on `aida node
    // acquire`) and the explicit `aida db block claim` command. Auto-claim
    // is for refilling capacity that's running low, not bootstrap.
    if registry
        .find_active_block_or_global(node_id, &effective_prefix)
        .is_none()
    {
        return Ok(None);
    }

    let previous_remaining = registry.aggregate_remaining(node_id, &effective_prefix);
    let threshold = cfg.threshold_for(&effective_prefix);
    if previous_remaining >= threshold {
        return Ok(None);
    }

    // Crossed the threshold — claim a fresh block of the configured size.
    // Use OnThresholdCross so the inner allocator skips its init-time
    // idempotency guard (which would otherwise refuse because we already
    // have an active block — the whole point of this call is to add a
    // second one).
    let size = cfg.size_for(&effective_prefix);
    let email = aida_core::git_ops::git_config_get("user.email").ok();
    let hn = hostname();
    let label = auto_allocate_block_inner(
        store_path,
        node_id,
        &hn,
        email.as_deref(),
        &effective_prefix,
        size,
        BlockClaimReason::OnThresholdCross,
    )?
    .ok_or_else(|| anyhow::anyhow!("inner allocator returned None on OnThresholdCross"))?;

    // Re-load to compute new_remaining off the persisted registry —
    // a single source of truth, no in-memory math.
    let registry = BlockRegistry::load(&blocks_path).unwrap_or_default();
    let new_remaining = registry.aggregate_remaining(node_id, &effective_prefix);

    Ok(Some(AutoClaimOutcome {
        label,
        previous_remaining,
        new_remaining,
    }))
}

#[cfg(test)]
#[path = "tests/block_allocation_reader_tests.rs"]
mod block_allocation_reader_tests;

#[cfg(test)]
#[path = "tests/merge_contiguous_blocks_tests.rs"]
mod merge_contiguous_blocks_tests;
