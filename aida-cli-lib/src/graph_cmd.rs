//! `aida graph` command handlers — STORY-489 cross-spec transitive queries
//! (`--blocked-by` / `--blocks` chains, `--tree` epic rollup, `--impact` reverse
//! closure, `--json`). Extracted verbatim from `main.rs` (SPIKE-78, pure
//! movement — no behavior change).

use anyhow::{Context, Result};
use colored::Colorize;

use aida_core::RelationshipType;

use crate::{agent_output_mode, parse_requirement_id, toon_status_token};

/// The TOON table name for an `aida graph` walk in a given `mode` (the string
/// `handle_graph_command` already resolved). Each graph direction gets its own
/// table name so an agent can key off the relation; `--follow`/unknown modes
/// fall back to a neutral `related`.
// trace:BUG-672 | ai:claude — plain `//` keeps the marker out of any doc/help.
fn graph_agent_table_name(mode: &str) -> &'static str {
    match mode {
        "blocked-by" => "blocked_by",
        "blocks" => "blocks",
        "impact" => "impact",
        "tree" => "tree",
        _ => "related",
    }
}

/// Render the agent-mode body of `aida graph`: a `root: <id> (<mode>)` preamble
/// (the `count:`-style header the other agent surfaces carry) plus a TOON
/// `<name>[N]{id,title,status}` table over the walk's `rows`. An EMPTY `rows`
/// still emits a valid `<name>[0]{...}:` header so an agent parses the empty and
/// filled directions uniformly. Returned as a `String` so the branch is
/// unit-testable without a store.
// trace:BUG-672 | ai:claude — plain `//` keeps the marker out of any doc/help.
fn render_graph_agent(mode: &str, root_label: &str, rows: &[Vec<String>]) -> String {
    let field_refs = ["id", "title", "status"];
    let table_name = graph_agent_table_name(mode);
    format!(
        "root: {root_label} ({mode})\n{}",
        crate::toon::table_raw(table_name, &field_refs, rows)
    )
}

/// Handler for `aida graph <SPEC>` — query the cross-spec relationship graph
/// (blocked-by / blocks chains, epic rollup, reverse impact) on top of the
/// cycle-safe `graph_walk` primitive (TASK-594). Read-only; the flagship
/// "outsmart the flat-markdown spec tools" demo.
// trace:STORY-489 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_graph_command(
    store: &aida_core::RequirementsStore,
    id_str: &str,
    blocked_by: bool,
    blocks: bool,
    tree: bool,
    impact: bool,
    follow: &[String],
    depth: Option<usize>,
    json: bool,
) -> Result<()> {
    use aida_core::graph_walk::{status_rollup, walk_union, Direction};

    let mode_count = [blocked_by, blocks, tree, impact]
        .iter()
        .filter(|b| **b)
        .count()
        + usize::from(!follow.is_empty());
    if mode_count > 1 {
        anyhow::bail!(
            "choose at most one graph mode: --blocked-by, --blocks, --tree, --impact, or --follow"
        );
    }

    let id = parse_requirement_id(id_str, store)?;
    let root = store
        .get_requirement_by_id(&id)
        .context("Requirement not found")?;
    let root_label = root.display_id();
    let root_title = root.title.clone();

    // Resolve mode → (walk specs, label). Default: tree. Each spec is a
    // (rel_types, direction) leg; impact spans two legs so a unidirectionally-
    // stored Blocks edge is still caught (BUG-411).
    type WalkSpecs = Vec<(Vec<RelationshipType>, Direction)>;
    let (specs, mode): (WalkSpecs, &str) = if blocked_by {
        (
            vec![(vec![RelationshipType::BlockedBy], Direction::Outgoing)],
            "blocked-by",
        )
    } else if blocks {
        (
            vec![(vec![RelationshipType::Blocks], Direction::Outgoing)],
            "blocks",
        )
    } else if impact {
        // Blocked by the root = X.BlockedBy→root (incoming BlockedBy) OR
        // root.Blocks→X (outgoing Blocks); a one-directional edge needs both.
        (
            vec![
                (vec![RelationshipType::BlockedBy], Direction::Incoming),
                (vec![RelationshipType::Blocks], Direction::Outgoing),
            ],
            "impact",
        )
    } else if !follow.is_empty() {
        // FR-282: traverse arbitrary named relationship types (custom or
        // built-in), outgoing. from_str maps an unknown name to Custom(name)
        // (BUG-251), so `--follow begets` walks Custom("begets") edges — the
        // query a flat per-feature spec store can't do over custom edges.
        (
            follow
                .iter()
                .map(|t| (vec![RelationshipType::from_str(t)], Direction::Outgoing))
                .collect(),
            "follow",
        )
    } else {
        // BUG-448: the spec hierarchy edge can live on EITHER endpoint, and the
        // rel_type names the TARGET's role (the convention export.rs reads): a
        // parent carries `Child → child`, while a child carries `Parent →
        // parent` (what `aida add --parent` / import write — export.rs
        // set_relationship). Unioning OUTGOING `Child` + `Parent` therefore
        // traverses the hierarchy whichever side recorded the edge, and from any
        // starting node — down via `Child`, up via `Parent` (so a query on a
        // child reaches its epic and back down to its siblings). The
        // visited/seen sets dedup the overlap. trace:BUG-448 | ai:claude
        (
            vec![(
                vec![RelationshipType::Child, RelationshipType::Parent],
                Direction::Outgoing,
            )],
            "tree",
        )
    };
    let is_tree = mode == "tree";

    // TASK-1074: `--tree` membership routes through the shared rank-oriented
    // hierarchy closure (`graph_walk::hierarchy_tree`) so `aida graph <epic>
    // --tree` and `aida focus <epic>` (the cache's `descendant_ids`) agree — the
    // old `walk_union([Child,Parent])` leaked a descendant's same-rank second
    // parent into the count. The other modes keep their own walk_union legs.
    let result = if is_tree {
        aida_core::graph_walk::hierarchy_tree(store, id, depth)
    } else {
        walk_union(store, id, &specs, depth)
    };

    if json {
        let nodes: Vec<_> = result
            .nodes
            .iter()
            .map(|nid| {
                let r = store.get_requirement_by_id(nid);
                serde_json::json!({
                    "id": r.map(|x| x.display_id()).unwrap_or_else(|| nid.to_string()),
                    "title": r.map(|x| x.title.clone()),
                    "status": r.map(|x| format!("{:?}", x.status)),
                    "resolved": r.is_some(),
                })
            })
            .collect();
        let rollup = status_rollup(store, &result.nodes);
        let out = serde_json::json!({
            "root": root_label,
            "mode": mode,
            "count": result.nodes.len(),
            "nodes": nodes,
            "rollup": {
                "total": rollup.total,
                "completed": rollup.completed,
                "done": rollup.done,
                "in_progress": rollup.in_progress,
                "remaining": rollup.remaining,
                "shelved": rollup.shelved,
                "rejected": rollup.rejected,
            },
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    // BUG-672: in agent mode the relation is emitted as a parseable TOON table
    // (an EMPTY table when the direction has no neighbors) instead of the human
    // prose `(no related specs in this direction)` / indented tree — so an agent
    // parses graph results uniformly regardless of direction or fill. The table
    // is named per direction (`blocked_by` / `blocks` / `impact` / `tree` /
    // `related`) with a `{id,title,status}` schema; a `root:` header line states
    // the queried spec + mode (the `count:`-style preamble the other agent
    // surfaces use). Status reuses `toon_status_token` so the value matches the
    // rest of the agent surface. The human TTY path below is unchanged.
    // trace:BUG-672
    if agent_output_mode() {
        let rows: Vec<Vec<String>> = result
            .nodes
            .iter()
            .map(|nid| match store.get_requirement_by_id(nid) {
                Some(r) => vec![
                    r.display_id(),
                    r.title.clone(),
                    toon_status_token(&format!("{:?}", r.status)),
                ],
                None => vec![nid.to_string(), String::new(), "unresolved".to_string()],
            })
            .collect();
        println!("{}", render_graph_agent(mode, &root_label, &rows));
        // Drill-in next-step block — into a related spec when the walk found
        // neighbors, else the root spec's own detail. trace:BUG-672
        let next = crate::help_next::graph_next(&root_label, !result.nodes.is_empty());
        if let Some(block) = crate::help_next::render(&next) {
            println!("{block}");
        }
        return Ok(());
    }

    println!(
        "{} {} {} — {}",
        "Graph".bold(),
        format!("({mode})").dimmed(),
        root_label.cyan().bold(),
        root_title
    );
    if result.nodes.is_empty() {
        println!("  {}", "(no related specs in this direction)".dimmed());
        // BUG-672: the human surface gets the same drill-in nudge — point at the
        // queried spec's own detail when the direction is empty. trace:BUG-672
        let next = crate::help_next::graph_next(&root_label, false);
        if let Some(block) = crate::help_next::render_human(&next) {
            println!("{block}");
        }
        return Ok(());
    }
    if is_tree {
        // Indented hierarchy so parents, children, and siblings are distinct
        // (BUG-534). tree_layout includes the queried node, rooted at the epic;
        // an arrow marks "you are here" when the query is not the structural root.
        for (nid, depth) in aida_core::graph_walk::tree_layout(id, &result.nodes, &result.edges) {
            let queried = nid == id;
            let lead = if queried {
                match depth {
                    0 => format!("{} ", crate::glyph(crate::glyphs::Glyph::Arrow)).to_string(),
                    d => format!(
                        "{}{} ",
                        "  ".repeat(d - 1),
                        crate::glyph(crate::glyphs::Glyph::Arrow)
                    ),
                }
            } else {
                "  ".repeat(depth)
            };
            match store.get_requirement_by_id(&nid) {
                Some(r) => {
                    let label = if queried {
                        r.display_id().cyan().bold()
                    } else {
                        r.display_id().yellow()
                    };
                    let here = if queried {
                        format!("  {}", "(queried)".dimmed())
                    } else {
                        String::new()
                    };
                    println!("{}{}  {}{}", lead, label, r.title, here);
                }
                None => println!(
                    "{}{}  {}",
                    lead,
                    nid.to_string().yellow(),
                    "(unresolved)".red()
                ),
            }
        }
    } else {
        for nid in &result.nodes {
            match store.get_requirement_by_id(nid) {
                Some(r) => println!("  {}  {}", r.display_id().yellow(), r.title),
                None => println!("  {}  {}", nid.to_string().yellow(), "(unresolved)".red()),
            }
        }
    }
    if is_tree {
        let r = status_rollup(store, &result.nodes);
        let shelved = if r.shelved > 0 {
            format!(" · {} shelved", r.shelved)
        } else {
            String::new()
        };
        let rejected = if r.rejected > 0 {
            format!(" · {} rejected", r.rejected)
        } else {
            String::new()
        };
        println!(
            "\n{} {} total · {} completed · {} in progress · {} remaining{}{}",
            "Rollup:".bold(),
            r.total,
            r.completed,
            r.in_progress,
            r.remaining,
            shelved,
            rejected
        );
    }
    // BUG-672 (Finding #4): trailing `Next:` block on the human graph surface —
    // drill into a related spec (the walk returned neighbors here). trace:BUG-672
    let next = crate::help_next::graph_next(&root_label, true);
    if let Some(block) = crate::help_next::render_human(&next) {
        println!("{block}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // BUG-672: `aida graph` in agent mode emits a parseable TOON table named per
    // direction with a `{id,title,status}` schema — an EMPTY direction still
    // emits a valid `<name>[0]{...}:` header (NOT the human prose `(no related
    // specs in this direction)`), so an agent parses empty and filled directions
    // uniformly. trace:BUG-672
    #[test]
    fn graph_agent_emits_toon_table() {
        colored::control::set_override(false);

        // Direction names map to distinct table names.
        assert_eq!(graph_agent_table_name("blocked-by"), "blocked_by");
        assert_eq!(graph_agent_table_name("blocks"), "blocks");
        assert_eq!(graph_agent_table_name("impact"), "impact");
        assert_eq!(graph_agent_table_name("tree"), "tree");
        assert_eq!(graph_agent_table_name("follow"), "related");

        // EMPTY direction: a valid zero-row TOON header, the root preamble, and
        // none of the human prose.
        let empty = render_graph_agent("blocked-by", "BUG-672", &[]);
        assert!(
            empty.contains("blocked_by[0]{id,title,status}:"),
            "empty direction is a valid empty TOON table: {empty}"
        );
        assert!(empty.contains("root: BUG-672 (blocked-by)"));
        assert!(
            !empty.contains("no related specs"),
            "no human prose leaks into the agent graph: {empty}"
        );

        // NON-EMPTY direction: one row per related spec, in the table body.
        let rows = vec![vec![
            "TASK-5".to_string(),
            "blocker thing".to_string(),
            "in-progress".to_string(),
        ]];
        let filled = render_graph_agent("blocks", "BUG-672", &rows);
        assert!(
            filled.contains("blocks[1]{id,title,status}:"),
            "filled direction is a 1-row TOON table: {filled}"
        );
        assert!(filled.contains("TASK-5") && filled.contains("in-progress"));
    }
}
