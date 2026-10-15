//! `aida rel` edge add/remove, relationship type matching and rows, tree, comment and card rendering (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Read node_id from the store's node.toml; defaults to 1 for unregistered nodes.
pub(crate) fn load_node_id(store_path: &std::path::Path) -> String {
    use aida_core::NodeConfig;
    let node_config_path = store_path.join(".aida").join("node.toml");
    if node_config_path.exists() {
        NodeConfig::load(&node_config_path)
            .map(|c| c.node_id)
            .unwrap_or_else(|_| "1".to_string())
    } else {
        // Fall back to dispenser.toml node_id (still a stringified-numeric
        // for legacy stores written before EPIC-9). trace:STORY-41
        let dispenser_path = store_path.join(".aida").join("dispenser.toml");
        if dispenser_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&dispenser_path) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.starts_with("node_id") {
                        if let Some(val) = line.split('=').nth(1) {
                            let trimmed = val.trim().trim_matches('"');
                            if !trimmed.is_empty() {
                                return trimmed.to_string();
                            }
                        }
                    }
                }
            }
        }
        "1".to_string()
    }
}

/// Get the local hostname for informational block registry labels.
pub(crate) fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// Load or create the distributed dispenser for a git-backed store.
/// Reads node config from {store}/.aida/node.toml; defaults to node_id=1
/// if no node registration has happened yet.
pub(crate) fn load_dispenser(
    store_path: &std::path::Path,
) -> Result<aida_core::models::DispenserHandle> {
    use aida_core::dispenser::{FileDispenser, IdMode};
    use aida_core::node::NodeConfig;

    let aida_dir = store_path.join(".aida");
    std::fs::create_dir_all(&aida_dir)?;

    let node_config_path = aida_dir.join("node.toml");
    let dispenser_path = aida_dir.join("dispenser.toml");

    // Load node_id from config, or default to "1" for local-only.
    // trace:STORY-41 | ai:claude
    let node_id: String = if node_config_path.exists() {
        NodeConfig::load(&node_config_path)?.node_id
    } else {
        "1".to_string() // default for unregistered local node
    };

    let mode = IdMode::Distributed { node_id };
    let dispenser = FileDispenser::open(dispenser_path, mode)?;

    Ok(aida_core::models::DispenserHandle(std::sync::Arc::new(
        dispenser,
    )))
}

/// TASK-102: human-readable phrase for a relationship edge, shared by
/// `aida show`'s inline enumeration and the centralized renderer so the two
/// stay consistent. trace:TASK-102 | ai:claude
pub(crate) fn relationship_phrase(rt: &aida_core::models::RelationshipType) -> String {
    use aida_core::models::RelationshipType as R;
    match rt {
        R::Parent => "is parent of".to_string(),
        R::Child => "is child of".to_string(),
        R::Duplicate => "is duplicate of".to_string(),
        R::Verifies => "verifies".to_string(),
        R::VerifiedBy => "is verified by".to_string(),
        R::References => "references".to_string(),
        R::BlockedBy => "is blocked by".to_string(),
        R::Blocks => "blocks".to_string(),
        // trace:TASK-1176 | ai:claude
        R::SupersededBy => "is superseded by".to_string(),
        R::Supersedes => "supersedes".to_string(),
        R::Custom(name) => name.clone(),
    }
}

/// STORY-446: add a BlockedBy edge from `spec_id` to `blocker_id`, plus the
/// inverse Blocks edge on the blocker — atomically and idempotently. Returns
/// the blocker's display id for messaging. The pickability gate
/// (`aida_core::pickability`) already matches the typed BlockedBy variant, so
/// this is purely the ergonomic declaration path (STORY-333 added the variant).
/// trace:STORY-446 | ai:claude
pub(crate) fn add_blocked_by_edge(
    backend: &aida_core::CachedGitBackend,
    spec_id: &str,
    blocker_id: &str,
) -> Result<String> {
    use aida_core::models::{Relationship, RelationshipType};
    use aida_core::DatabaseBackend;

    let mut req = backend
        .get_requirement_unambiguous(spec_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(spec_id, None))?;
    let blocker = backend
        .get_requirement_unambiguous(blocker_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(blocker_id, None))?;
    if blocker.id == req.id {
        anyhow::bail!("a requirement cannot be blocked by itself ({})", spec_id);
    }
    let blocker_display = blocker
        .spec_id
        .clone()
        .unwrap_or_else(|| blocker_id.to_string());

    // Idempotent: only add the BlockedBy edge if absent.
    if !req
        .relationships
        .iter()
        .any(|r| matches!(r.rel_type, RelationshipType::BlockedBy) && r.target_id == blocker.id)
    {
        req.relationships.push(Relationship {
            rel_type: RelationshipType::BlockedBy,
            target_id: blocker.id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
        req.modified_at = chrono::Utc::now();
        backend.update_requirement(&req)?;
    }

    // Inverse Blocks edge on the blocker (also idempotent).
    let mut blocker = backend.get_requirement_unambiguous(blocker_id)?.unwrap(); // trace:TASK-1468 | ai:claude
    if !blocker
        .relationships
        .iter()
        .any(|r| matches!(r.rel_type, RelationshipType::Blocks) && r.target_id == req.id)
    {
        blocker.relationships.push(Relationship {
            rel_type: RelationshipType::Blocks,
            target_id: req.id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
        blocker.modified_at = chrono::Utc::now();
        backend.update_requirement(&blocker)?;
    }
    Ok(blocker_display)
}

/// TASK-1176: record that `spec_id` was REPLACED BY `successor_id` — a typed
/// `SupersededBy` edge on the superseded spec plus the inverse `Supersedes`
/// edge on the successor, atomically and idempotently. Returns the successor's
/// display id for messaging.
///
/// This is the first-class alternative to the `superseded-by:ADR-N` string tag
/// downstream projects had to invent: a typed edge is walkable by `aida graph`
/// / `query_graph`, survives an id rename, and cannot silently disagree with
/// the status. Shaped deliberately like [`add_blocked_by_edge`] so the two
/// bidirectional-edge writers stay recognizably the same code.
// trace:TASK-1176 | ai:claude
pub(crate) fn add_superseded_by_edge(
    backend: &aida_core::CachedGitBackend,
    spec_id: &str,
    successor_id: &str,
) -> Result<String> {
    use aida_core::models::{Relationship, RelationshipType};
    use aida_core::DatabaseBackend;

    let mut req = backend
        .get_requirement_unambiguous(spec_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(spec_id, None))?;
    let successor = backend
        .get_requirement_unambiguous(successor_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(successor_id, None))?;
    if successor.id == req.id {
        anyhow::bail!("a requirement cannot supersede itself ({})", spec_id);
    }
    let successor_display = successor
        .spec_id
        .clone()
        .unwrap_or_else(|| successor_id.to_string());

    if !req.relationships.iter().any(|r| {
        matches!(r.rel_type, RelationshipType::SupersededBy) && r.target_id == successor.id
    }) {
        req.relationships.push(Relationship {
            rel_type: RelationshipType::SupersededBy,
            target_id: successor.id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
        req.modified_at = chrono::Utc::now();
        backend.update_requirement(&req)?;
    }

    // Inverse Supersedes edge on the successor (also idempotent).
    let mut successor = backend.get_requirement_unambiguous(successor_id)?.unwrap(); // trace:TASK-1468 | ai:claude
    if !successor
        .relationships
        .iter()
        .any(|r| matches!(r.rel_type, RelationshipType::Supersedes) && r.target_id == req.id)
    {
        successor.relationships.push(Relationship {
            rel_type: RelationshipType::Supersedes,
            target_id: req.id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
        successor.modified_at = chrono::Utc::now();
        backend.update_requirement(&successor)?;
    }
    Ok(successor_display)
}

// STORY-1434: a carved-out acceptance criterion previously left two halves
// that could drift apart — stale text staying in the description as the
// authoritative gate, and the correction landing only in a comment the
// default `aida show` never surfaced. These names are the shared vocabulary
// for closing that gap: the relationship names the typed edge (a `Custom`
// variant, so no enum-wide match site needs updating — walkable today by
// `aida graph --follow carved-out-to` / `query_graph`), and the comment
// markers are the prefixes `aida show`'s default view treats as visible
// corrections rather than buried commentary. trace:STORY-1434 | ai:claude
/// The typed edge a carve-out writes on the SOURCE spec: "this spec carved a
/// criterion out, now carried by the target". Stored as
/// `RelationshipType::Custom(CARVED_OUT_TO_REL)` — see the module doc above
/// for why a new enum variant isn't needed.
pub(crate) const CARVED_OUT_TO_REL: &str = "carved-out-to";

/// The reciprocal edge on the TARGET spec: "this spec now carries a
/// criterion carved from the source".
pub(crate) const CARVED_FROM_REL: &str = "carved-from";

/// Comment-body prefixes `aida show`'s DEFAULT view (no `-c`/`--comments`)
/// surfaces inline right after the description, instead of leaving them
/// invisible until someone thinks to pass `-c` or scrolls hundreds of lines
/// into the YAML. Matched case-insensitively against the start of the
/// (trimmed) comment body. `CARVE-OUT` is what `--carve-out` itself writes;
/// `CORRECTION` and `PROXY DECISION` are the free-text conventions the
/// advisor seat was already using by hand for the same kind of "the
/// description says X but that's stale" note (the STORY-1434 filing cites
/// three real instances of exactly that shape).
pub(crate) const DEFAULT_VISIBLE_COMMENT_MARKERS: &[&str] =
    &["CARVE-OUT", "CORRECTION", "PROXY DECISION"];

/// True when `body` opens with one of [`DEFAULT_VISIBLE_COMMENT_MARKERS`]
/// (case-insensitive), followed by a non-alphanumeric character (`:`, `-`,
/// whitespace, end-of-string, …) so `CORRECTIONAL` doesn't false-positive on
/// the `CORRECTION` marker.
// trace:STORY-1434 | ai:claude
pub(crate) fn is_default_visible_comment(body: &str) -> bool {
    let trimmed = body.trim_start();
    // trace:BUG-1713 | ai:antigravity
    DEFAULT_VISIBLE_COMMENT_MARKERS.iter().any(|marker| {
        trimmed.len() >= marker.len()
            && trimmed.is_char_boundary(marker.len())
            && trimmed[..marker.len()].eq_ignore_ascii_case(marker)
            && trimmed[marker.len()..]
                .chars()
                .next()
                .map(|c| !c.is_alphanumeric())
                .unwrap_or(true)
    })
}

/// Collect every comment (top-level and nested reply) whose body matches
/// [`is_default_visible_comment`], in document order.
// trace:STORY-1434 | ai:claude
pub(crate) fn collect_default_visible_comments<'a>(
    comments: &'a [aida_core::Comment],
    out: &mut Vec<&'a aida_core::Comment>,
) {
    for c in comments {
        if is_default_visible_comment(&c.content) {
            out.push(c);
        }
        collect_default_visible_comments(&c.replies, out);
    }
}

/// Strike `criterion` out of `description`, replacing the first verbatim
/// occurrence with a short pointer at the target that now carries it.
/// Returns `None` (no-op) when `criterion` isn't found — a carve-out of text
/// that doesn't match verbatim is refused by the caller rather than silently
/// doing nothing, so this stays a pure "did it match" signal. Pure and
/// backend-free so the substring/pointer behavior is unit-testable without a
/// git store.
// trace:STORY-1434 | ai:claude
pub(crate) fn carve_out_description(
    description: &str,
    criterion: &str,
    target_display: &str,
) -> Option<String> {
    if criterion.is_empty() || !description.contains(criterion) {
        return None;
    }
    let marker = format!("[carved out \u{2192} {target_display} \u{2014} see carve-out log]");
    Some(description.replacen(criterion, &marker, 1))
}

/// The target ids of every `carved-out-to` edge on `req` — i.e. the specs
/// that now carry a criterion this spec used to gate on. Empty when the spec
/// carries no carve-out.
// trace:STORY-1434 | ai:claude
pub(crate) fn carved_out_targets(req: &aida_core::Requirement) -> Vec<uuid::Uuid> {
    req.relationships
        .iter()
        .filter(|r| {
            matches!(&r.rel_type, RelationshipType::Custom(name) if name.eq_ignore_ascii_case(CARVED_OUT_TO_REL))
        })
        .map(|r| r.target_id)
        .collect()
}

/// Apply a carve-out to an ALREADY-RESOLVED-AND-VALIDATED `req`: push the
/// typed `Custom("carved-out-to")` edge (idempotent) and the `CARVE-OUT:`
/// comment [`is_default_visible_comment`] recognizes. Pure — no backend I/O —
/// so the caller can fold this into the SAME `req` mutation the rest of
/// `aida edit`'s scalar fields go through and save it in the one atomic
/// write, instead of a separate follow-up write that could land only
/// half-applied (the description struck but the edge/comment lost, or vice
/// versa, on a failure in between).
// trace:STORY-1434 | ai:claude
pub(crate) fn apply_carve_out(
    req: &mut aida_core::Requirement,
    target_id: uuid::Uuid,
    source_display: &str,
    target_display: &str,
    criterion: &str,
    reason: Option<&str>,
) {
    use aida_core::models::{Relationship, RelationshipType};

    if !req.relationships.iter().any(|r| {
        matches!(&r.rel_type, RelationshipType::Custom(name) if name.eq_ignore_ascii_case(CARVED_OUT_TO_REL))
            && r.target_id == target_id
    }) {
        req.relationships.push(Relationship {
            rel_type: RelationshipType::Custom(CARVED_OUT_TO_REL.to_string()),
            target_id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
    }

    let reason_line = reason
        .map(|r| r.trim())
        .filter(|r| !r.is_empty())
        .unwrap_or("no reason given");
    let comment_body = format!(
        "CARVE-OUT: \"{criterion}\" is no longer carried by {source_display} — it is now \
         carried by {target_display}.\n\nReason: {reason_line}"
    );
    let mut comment = aida_core::Comment::new(get_default_author(), comment_body);
    // trace:TASK-330 | ai:claude — stamp the producing session, same as
    // `aida comment add`'s comment.
    comment.session_id = resolve_current_session_id();
    req.comments.push(comment);
}

/// Write the inverse `Custom("carved-from")` edge on the carve-out TARGET,
/// after the source's atomic save has landed. Idempotent. The target is
/// re-fetched by UUID (never re-resolved from a user-typed id — the caller
/// already resolved and validated it once via `get_requirement_unambiguous`),
/// so this can only fail on a genuine backend error or the target having
/// disappeared between resolution and this call — either way that is an
/// error the caller must surface, not swallow as a warning: a one-sided
/// carve-out (source struck, no inverse edge) is exactly the kind of
/// half-applied state this spec exists to prevent.
// trace:STORY-1434 | ai:claude
pub(crate) fn add_carved_from_edge(
    backend: &aida_core::CachedGitBackend,
    target_id: uuid::Uuid,
    target_display: &str,
    source_id: uuid::Uuid,
    source_display: &str,
) -> Result<()> {
    use aida_core::models::{Relationship, RelationshipType};
    use aida_core::DatabaseBackend;

    let mut target = backend.get_requirement(&target_id)?.ok_or_else(|| {
        anyhow::anyhow!(
            "carve-out target {} disappeared before the inverse carved-from edge could be \
             written — the carve-out edge/comment on the source are saved, but the reciprocal \
             edge on the target is missing; re-run `aida rel add {} {} --type carved-from` \
             to repair it by hand",
            target_id,
            target_display,
            source_display
        )
    })?;
    if !target.relationships.iter().any(|r| {
        matches!(&r.rel_type, RelationshipType::Custom(name) if name.eq_ignore_ascii_case(CARVED_FROM_REL))
            && r.target_id == source_id
    }) {
        target.relationships.push(Relationship {
            rel_type: RelationshipType::Custom(CARVED_FROM_REL.to_string()),
            target_id: source_id,
            created_at: Some(chrono::Utc::now()),
            created_by: Some(get_default_author()),
        });
        target.modified_at = chrono::Utc::now();
        backend.update_requirement(&target)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_1434_carve_out_tests.rs"]
mod story_1434_carve_out_tests;

/// STORY-446: remove the BlockedBy edge from `spec_id` to `blocker_id` and the
/// inverse Blocks edge on the blocker. No-op (returns the display id) when the
/// edge is absent. trace:STORY-446 | ai:claude
pub(crate) fn remove_blocked_by_edge(
    backend: &aida_core::CachedGitBackend,
    spec_id: &str,
    blocker_id: &str,
) -> Result<String> {
    use aida_core::models::RelationshipType;
    use aida_core::DatabaseBackend;

    let mut req = backend
        .get_requirement_unambiguous(spec_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(spec_id, None))?;
    let blocker = backend
        .get_requirement_unambiguous(blocker_id)? // trace:TASK-1468 | ai:claude
        .ok_or_else(|| not_found::requirement_not_found(blocker_id, None))?;
    let blocker_display = blocker
        .spec_id
        .clone()
        .unwrap_or_else(|| blocker_id.to_string());

    let before = req.relationships.len();
    req.relationships.retain(|r| {
        !(matches!(r.rel_type, RelationshipType::BlockedBy) && r.target_id == blocker.id)
    });
    if req.relationships.len() != before {
        req.modified_at = chrono::Utc::now();
        backend.update_requirement(&req)?;
    }

    let mut blocker = backend.get_requirement_unambiguous(blocker_id)?.unwrap(); // trace:TASK-1468 | ai:claude
    let inv_before = blocker.relationships.len();
    blocker
        .relationships
        .retain(|r| !(matches!(r.rel_type, RelationshipType::Blocks) && r.target_id == req.id));
    if blocker.relationships.len() != inv_before {
        blocker.modified_at = chrono::Utc::now();
        backend.update_requirement(&blocker)?;
    }
    Ok(blocker_display)
}

/// Handle commands routed to the GitBackend (when --file points to a directory).
/// Resolve an `aida history --id <X>` / positional `aida history <X>`
/// argument to the canonical spec_id the orphan-branch event decoder keys
/// on. `aida history` walks the git log and tags every event with the
/// YAML's `spec_id`, so a UUID (which `aida show` prints) or an agreed_id
/// never matched the filter and produced an empty "(no recent activity)" —
/// the symptom BUG-588 reports. When the argument resolves to exactly one
/// live requirement (by spec_id, agreed_id, or UUID) we substitute its
/// canonical spec_id.
///
/// TASK-1480: this is also the "invalid or ambiguous IDs get a clear error"
/// gate. Two cases refuse outright: a string that can't possibly be a spec
/// id (BUG-599's format hint) and one that resolves to more than one
/// requirement ([`aida_core::id_collisions::AmbiguousIdError`], which
/// already names each candidate's unambiguous handle). A well-formed id
/// that simply isn't *live* right now — deleted, or never assigned — is
/// passed through unchanged rather than rejected here: a deleted spec can
/// still have real history to show, so `history::run` is the one that
/// decides, once it knows whether the id has any recorded events at all.
/// trace:BUG-588 | ai:claude
// trace:TASK-1480 | ai:claude
pub(crate) fn resolve_history_id_filter<B: aida_core::db::DatabaseBackend>(
    backend: &B,
    raw: &str,
) -> Result<String> {
    let trimmed = raw.trim();
    let looks_like_uuid = uuid::Uuid::parse_str(trimmed).is_ok();
    if !looks_like_uuid && !aida_core::object_store::valid_spec_id_format(trimmed) {
        return Err(crate::not_found::invalid_spec_id_format(trimmed));
    }
    // `get_requirement_unambiguous` already turns a multi-match into an
    // `anyhow::Error` (AmbiguousIdError's own Display), so `?` here IS the
    // "ambiguous id gets a clear error" behavior. trace:TASK-1480 | ai:claude
    // Read-only: tolerant resolver. trace:BUG-1670 | ai:claude
    match backend.get_requirement_unambiguous_for_read(trimmed)? {
        Some(req) => Ok(req.spec_id.clone().unwrap_or_else(|| trimmed.to_string())),
        None => Ok(trimmed.to_string()),
    }
}

/// TASK-679: a parent/child edge is canonically BIDIRECTIONAL so both ends
/// reflect the link — matching `aida add --parent`, which writes the parent's
/// `Parent --> child` edge and the child's reciprocal `Child --> parent` edge.
/// `aida rel add` previously wrote only the source-side edge for these types.
/// Any other edge type keeps the explicit `--bidirectional` opt-in. Pure so
/// the canonical-edge decision is unit-testable. trace:TASK-679 | ai:claude
pub(crate) fn rel_should_write_inverse(
    rel_type: &RelationshipType,
    bidirectional_flag: bool,
) -> bool {
    bidirectional_flag || matches!(rel_type, RelationshipType::Parent | RelationshipType::Child)
}

/// Parse the relationship vocabulary accepted by `aida rel add` / `aida rel
/// remove`.
///
/// `related` is the natural spelling for a general link, but storing it as a
/// custom edge makes the link invisible to standard graph traversal; this
/// normalizes it (and the other input-only aliases like `depends-on` /
/// `verified_by` / `replaced_by`) to the matching standard taxonomy member.
/// Thin wrapper around the shared parser in aida-core
/// (`RelationshipType::parse_relationship_type`) so `rel add`, `rel remove`
/// and the MCP `relationship_type` param all resolve the same spelling the
/// same way — see that function's doc comment for why it's kept separate
/// from `RelationshipType::from_str`, the core/Deserialize parser.
// trace:BUG-1471 | ai:codex trace:BUG-1602 | ai:claude
pub(crate) fn cli_relationship_type(input: &str) -> RelationshipType {
    RelationshipType::parse_relationship_type(input)
}

/// Does a stored edge match the `--type` given to `rel remove`?
///
/// Exact type match, plus: asking to remove `references` (or its `related`
/// alias) also matches a stored legacy custom `related` / `related-to` /
/// `relates-to` edge, and asking for one of those legacy spellings matches
/// the whole family, so the obvious hand repair does not leave a stale edge
/// behind. Before this, the git-store `rel remove` ignored `--type` and
/// deleted every edge to the target.
// trace:TASK-1426 | ai:claude
pub(crate) fn rel_remove_matches(stored: &RelationshipType, requested: &RelationshipType) -> bool {
    use crate::related_edge_migration::is_legacy_related;
    if stored == requested {
        return true;
    }
    let requested_is_references_family =
        *requested == RelationshipType::References || is_legacy_related(requested);
    requested_is_references_family
        && (is_legacy_related(stored)
            || (is_legacy_related(requested) && *stored == RelationshipType::References))
}

pub(crate) fn looks_like_cross_store_spec_ref(s: &str) -> bool {
    let Some((project, spec)) = s.split_once('#') else {
        return false;
    };
    !project.trim().is_empty()
        && !spec.trim().is_empty()
        && spec
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
}

pub(crate) fn reject_cross_store_spec_ref(raw: &str) -> Result<()> {
    if looks_like_cross_store_spec_ref(raw) {
        anyhow::bail!(
            "cross-store requirement reference `{raw}` cannot be resolved as a local graph edge. \
             Record it in a spec comment as `{raw}` until AIDA has first-class cross-store links."
        );
    }
    Ok(())
}

pub(crate) fn relationship_terminal_ambiguity_warning(
    rel_type: &RelationshipType,
    source: &Requirement,
    target: &Requirement,
) -> Option<String> {
    if matches!(rel_type, RelationshipType::Parent | RelationshipType::Child) {
        return None;
    }
    if is_terminal_status(&source.status) || !is_terminal_status(&target.status) {
        return None;
    }
    Some(format!(
        "target {} is {} while source {} is {}; if this was meant for another store, do not use this local edge. Record the cross-store reference in a comment instead.",
        target.spec_id.as_deref().unwrap_or("?"),
        target.status,
        source.spec_id.as_deref().unwrap_or("?"),
        source.status,
    ))
}

// TASK-928 (SPIKE-71 source-side fix): a `parent:<SPEC-ID>` tag must
// materialize the REAL bidirectional parent/child edge, not just sit on the
// spec as an opaque string. The canonical `--parent` flag already writes the
// edge; a spec filed with only `--tags "parent:EPIC"` used to be orphaned
// from the graph — the exact trap behind SPIKE-71 (`aida graph tree` and the
// TUI focus-lens never saw it). This closes that gap for `aida add` and the
// tag-changing `aida edit` path. (Plain `//`, not `///`: keeps the SPEC-ID
// breadcrumb out of any --help surface — substrate-as-bouncer pre-commit gate.)
//
// Contract:
//   - Additive — the `parent:` tag is left in place (the edge is *added*).
//   - Lenient — an unresolvable / self / empty target is a silent no-op
//     (tag kept, NO error), per the spec.
//   - Idempotent — an already-present Child->parent edge is skipped, and the
//     reciprocal Parent->child edge is deduped independently.
//   - Bidirectional — writes BOTH ends (same shape as `aida add --parent`),
//     so the cache write-through on each `update_requirement` keeps the graph
//     immediately consistent without a manual `aida cache rebuild`.
//
// Returns the linked parent's display id when a new edge was written (for
// caller messaging), `None` when nothing changed. trace:TASK-928 | ai:claude
pub(crate) fn ensure_parent_edge_from_tag(
    backend: &aida_core::CachedGitBackend,
    child_spec_id: &str,
) -> Result<Option<String>> {
    use aida_core::models::{Relationship, RelationshipType};

    // trace:TASK-1468 | ai:claude
    let Some(child) = backend.get_requirement_unambiguous(child_spec_id)? else {
        return Ok(None);
    };
    // First `parent:<ID>` tag, if any (a spec carrying several is degenerate;
    // honor the first deterministically).
    let Some(target) = child
        .tags
        .iter()
        .filter_map(|t| t.strip_prefix("parent:").map(|s| s.trim().to_string()))
        .find(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    // Lenient resolve — an unknown target leaves the tag untouched, no error.
    // trace:TASK-1468 | ai:claude
    let Some(parent) = backend.get_requirement_unambiguous(&target)? else {
        return Ok(None);
    };
    if parent.id == child.id {
        return Ok(None); // a spec cannot be its own parent
    }
    // Idempotent: skip when the child already records the Child→parent edge.
    let child_linked = child
        .relationships
        .iter()
        .any(|r| r.target_id == parent.id && r.rel_type == RelationshipType::Child);
    let parent_linked = parent
        .relationships
        .iter()
        .any(|r| r.target_id == child.id && r.rel_type == RelationshipType::Parent);
    if child_linked && parent_linked {
        return Ok(None);
    }

    let now = chrono::Utc::now();
    if !child_linked {
        let mut child_mut = child.clone();
        child_mut.relationships.push(Relationship {
            target_id: parent.id,
            rel_type: RelationshipType::Child,
            created_at: Some(now),
            created_by: None,
        });
        child_mut.modified_at = now;
        backend.update_requirement(&child_mut)?;
    }
    if !parent_linked {
        let mut parent_mut = parent.clone();
        parent_mut.relationships.push(Relationship {
            target_id: child.id,
            rel_type: RelationshipType::Parent,
            created_at: Some(now),
            created_by: None,
        });
        parent_mut.modified_at = now;
        backend.update_requirement(&parent_mut)?;
    }
    Ok(Some(parent.spec_id.clone().unwrap_or(target)))
}

/// The canonical user-facing spellings of the STANDARD relationship types — the
/// ones `RelationshipType::from_str` recognizes and the graph queries
/// (`--blocked-by`, `--blocks`, `--tree`, `--impact`) actually traverse. The
/// did-you-mean lens for `aida rel add --type` is computed against this set.
/// trace:TASK-887 | ai:claude
pub(crate) const STANDARD_REL_TYPES: &[&str] = &[
    "parent",
    "child",
    "duplicate",
    "verifies",
    "verified-by",
    "references",
    "related",
    "blocked-by",
    "blocks",
    // trace:TASK-1176 | ai:claude — the supersede lineage pair.
    "superseded-by",
    "supersedes",
];

/// Plain Levenshtein edit distance between two strings (case folding is the
/// caller's job). Small inputs only — relationship-type names are a handful of
/// chars — so the O(n*m) DP is fine. trace:TASK-887 | ai:claude
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// TASK-887: when `aida rel add --type <x>` gets a value that ISN'T a known
/// standard relationship type, it's stored as a `Custom` edge — which the graph
/// traversals silently won't follow. Returns the nearest standard type when the
/// input is a plausible typo of one (edit distance small relative to its
/// length), for a "did you mean" hint. Returns `None` for an already-standard
/// type or a value too far from any standard to be a typo (a genuine custom
/// type). Pure → unit-testable. trace:TASK-887 | ai:claude
pub(crate) fn nearest_standard_rel_type(input: &str) -> Option<&'static str> {
    let needle = input.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    // Already standard (or an accepted alias normalizing to one) → no hint.
    if !matches!(cli_relationship_type(&needle), RelationshipType::Custom(_)) {
        return None;
    }
    let mut best: Option<(&'static str, usize)> = None;
    for &std_ty in STANDARD_REL_TYPES {
        let d = levenshtein(&needle, std_ty);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((std_ty, d));
        }
    }
    match best {
        // Only call it a typo when it's close: ≤2 edits. Empirically the
        // standard-type typos sit at d≤2 while deliberate custom types
        // ("implements", "relates-to") sit at d≥6, so this cleanly
        // separates the two without nagging genuine custom edges.
        Some((ty, d)) if d <= 2 => Some(ty),
        _ => None,
    }
}

/// Find the nearest existing spec id to a requested one, for the not-found
/// did-you-mean hint. Case-insensitive Levenshtein with a length-aware budget:
/// short ids (≤3 chars) tolerate 1 edit, longer ids up to 2 — so a typo like
/// `TASK-11` → `TASK-1` is caught while an unrelated id is left alone. An exact
/// (case-insensitive) match returns None because that would have resolved on
/// the found path. Deterministic: lowest edit distance wins, ties prefer an id
/// that is a prefix of / prefixed by the request (the extra-digit / dropped-
/// suffix typo), then the lexicographically-smallest candidate. Pure over the
/// caller-supplied id set → unit-testable.
// trace:TASK-1082 | ai:claude
pub(crate) fn nearest_spec_id(requested: &str, known_ids: &[String]) -> Option<String> {
    let needle = requested.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    // If the requested id exactly (case-insensitively) exists, it's not a typo
    // — suggest nothing. In practice this fn only runs on the not-found branch,
    // but the guard keeps it honest if invoked with a resolvable id.
    if known_ids
        .iter()
        .any(|k| k.trim().eq_ignore_ascii_case(&needle))
    {
        return None;
    }
    let max_dist = if needle.len() <= 3 { 1 } else { 2 };
    // Sort key: (distance, not-prefix-related, candidate). Lower is better, so
    // `!prefix_related` sorts prefix matches first among equal-distance ties.
    let mut best: Option<(usize, bool, String)> = None;
    for known in known_ids {
        let cand = known.trim();
        if cand.is_empty() {
            continue;
        }
        let cand_lc = cand.to_lowercase();
        let d = levenshtein(&needle, &cand_lc);
        // d == 0 is the found path — never suggest the request back to itself.
        if d == 0 || d > max_dist {
            continue;
        }
        let prefix_related = needle.starts_with(&cand_lc) || cand_lc.starts_with(&needle);
        let key = (d, !prefix_related, cand.to_string());
        if best.as_ref().is_none_or(|b| key < *b) {
            best = Some(key);
        }
    }
    best.map(|(_, _, id)| id)
}

/// Extract the requested id from a "Requirement not found: <id>" error's first
/// line, so the top-level handler can compute a did-you-mean suggestion.
/// Returns None for not-found messages that carry no id (the legacy
/// `.context("Requirement not found")` chains, whose first line has no `: <id>`).
/// `invalid_spec_id_format` appends " (not a valid spec ID)" — the trailing
/// parenthetical is stripped so the bare id is returned. Pure → unit-testable.
// trace:TASK-1082 | ai:claude
pub(crate) fn not_found_requested_id(msg: &str) -> Option<String> {
    let first = msg.lines().next()?.trim();
    let rest = first.strip_prefix("Requirement not found: ")?;
    let id = rest.split(" (").next().unwrap_or(rest).trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Best-effort gather of every known spec id (plus agreed id) from the cache,
/// for the not-found did-you-mean lens. Cache-backed and read-only; any failure
/// (no store attached, cache locked, wrong directory) yields an empty set so
/// the caller simply omits the suggestion. Runs only on the not-found error
/// path, so the one-shot cache read is off the hot path.
// trace:TASK-1082 | ai:claude
pub(crate) fn known_spec_ids_for_suggestion(project_root: &std::path::Path) -> Vec<String> {
    let store_path = project_root.join(".aida-store");
    let Ok(dispenser) = load_dispenser(&store_path) else {
        return Vec::new();
    };
    let Ok(inner) = aida_core::GitBackend::new(&store_path).map(|b| b.with_dispenser(dispenser))
    else {
        return Vec::new();
    };
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let Ok(backend) = aida_core::CachedGitBackend::with_inner(inner, &cache_path) else {
        return Vec::new();
    };
    // Suggest against EVERY id, including archived/deferred rows — a typo should
    // still resolve to a real (if hidden) spec.
    let filter = aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        defer: aida_core::DeferFilter::Both,
        ..Default::default()
    };
    let Ok(rows) = backend.list_summaries(&filter) else {
        return Vec::new();
    };
    let mut ids = Vec::with_capacity(rows.len());
    for r in rows {
        if let Some(s) = r.spec_id {
            ids.push(s);
        }
        if let Some(a) = r.agreed_id {
            ids.push(a);
        }
    }
    ids
}

/// Glue for the top-level error handler: if `msg` is a "Requirement not found"
/// error carrying an id, resolve the nearest existing spec id and return a
/// ready-to-print `did you mean <ID>?` string. None when the message isn't a
/// not-found-with-id, or nothing is close enough to suggest.
// trace:TASK-1082 | ai:claude
pub(crate) fn did_you_mean_for_not_found(msg: &str) -> Option<String> {
    let requested = not_found_requested_id(msg)?;
    let project_root = find_main_worktree_root().ok()?;
    let known = known_spec_ids_for_suggestion(&project_root);
    let suggestion = nearest_spec_id(&requested, &known)?;
    Some(format!("did you mean {suggestion}?"))
}

// TASK-679: fully-isolated unit tests for the canonical parent/child edge
// behaviour of `aida rel add`. These exercise the model layer directly (no
// filesystem, no git) plus the pure `rel_should_write_inverse` decision.
// trace:TASK-679 | ai:claude
#[cfg(test)]
#[path = "tests/task_679_canonical_rel_tests.rs"]
mod task_679_canonical_rel_tests;

// TASK-928 (SPIKE-71 source-side fix): end-to-end tests over a real
// CachedGitBackend (temp git store + cache), proving:
//   * a `parent:<ID>` tag materializes the real bidirectional edge (part B);
//   * the edge is visible to a fresh full-store load + graph walk WITHOUT a
//     manual `aida cache rebuild` (part D — the cache write-through restamps
//     HEAD, and graph reads the worktree YAML via inner.load());
//   * the helper is lenient (bad target = no-op) and idempotent.
// trace:TASK-928 | ai:claude
#[cfg(test)]
#[path = "tests/task_928_parent_tag_edge_tests.rs"]
mod task_928_parent_tag_edge_tests;

// TASK-1426: fixture tests for `aida db migrate-related-edges` and for
// `rel remove` honoring `--type` against stored custom related edges.
// trace:TASK-1426 | ai:claude
#[cfg(test)]
#[path = "tests/task_1426_related_edge_migration_tests.rs"]
mod task_1426_related_edge_migration_tests;

// TASK-1488: fixture tests for the widened `aida db migrate-related-edges`
// selection predicate — any Custom edge whose spelling parses to a standard
// type via `RelationshipType::parse_relationship_type`, not just the
// `related` family. trace:TASK-1488 | ai:claude
#[cfg(test)]
#[path = "tests/task_1488_related_edge_migration_widen_tests.rs"]
mod task_1488_related_edge_migration_widen_tests;

// BUG-1602: handler-level tests of `aida rel remove` itself (typed removal,
// --bidirectional, the parent/child pair, the legacy Custom-related family)
// through the real Command::Rel dispatch path — task_1426's tests above only
// cover the pure `rel_remove_matches` helper. trace:BUG-1602 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1602_rel_remove_handler_tests.rs"]
mod bug_1602_rel_remove_handler_tests;

// BUG-1611: lifecycle authority guard at queue done, the forced reopen, and
// zen auto-approve. trace:BUG-1611 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1611_lifecycle_authority_tests.rs"]
mod bug_1611_lifecycle_authority_tests;

// BUG-1732: `load_store_for_lookup` is project-root-scoped, so no
// project-scoped read can surface another project's requirements.
// trace:BUG-1732 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1732_store_lookup_scope_tests.rs"]
mod bug_1732_store_lookup_scope_tests;

// trace:TASK-1468 | ai:claude
#[cfg(test)]
#[path = "tests/task_1468_ambiguous_write_ids_tests.rs"]
mod task_1468_ambiguous_write_ids_tests;

#[cfg(test)]
#[path = "tests/task_887_888_input_validation_tests.rs"]
mod task_887_888_input_validation_tests;

#[cfg(test)]
#[path = "tests/task_1082_did_you_mean_tests.rs"]
mod task_1082_did_you_mean_tests;

/// Modern `aida rel list` over the git-canonical backend. Supports three
/// modes (global / outgoing / incoming) plus `--type`, `--dangling`, and
/// `--all` filters. Output uses a uniform `FROM → TO   TITLE` row format
/// across all modes for grep-friendly behavior. trace:TASK-65 | ai:claude
pub(crate) fn handle_rel_list_modern(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    source: Option<&str>,
    target: Option<&str>,
    type_filter: Option<&str>,
    dangling_only: bool,
    include_all: bool,
    limit: Option<usize>,
) -> Result<()> {
    use aida_core::DatabaseBackend;

    // Default cap for the unfiltered global firehose so `aida rel list` with
    // no args doesn't dump every edge on a large store. Any filter (source /
    // target / type / dangling) or an explicit --limit overrides it.
    // trace:TASK-778
    const GLOBAL_AUTO_CAP: usize = 50;

    if source.is_some() && target.is_some() {
        anyhow::bail!(
            "pass either a source (positional or --source) or --target, not both — \
             they pick different edge sets"
        );
    }

    // Normalize the type filter against the canonical names. We let custom
    // types through as exact string compare so user-defined relationship
    // names still filter correctly.
    let type_filter_norm: Option<RelationshipType> = type_filter.map(RelationshipType::from_str);

    // Build the universe we need to read. Outgoing/global: walk every
    // requirement. Incoming: same — there's no inverse index, so a target
    // query is "find every edge whose target_id resolves to this UUID".
    let all_reqs = backend.list_requirements(false)?;

    // BUG-573: `aida rel list` is a READ-ONLY query. When the named source /
    // target spec doesn't resolve there are simply no edges to list — that's
    // an empty result, not a failure. Emitting a non-zero exit here was the
    // single biggest distortion in `aida usage errors` (loops/agents query
    // relationships for specs that may be archived/removed) and papercut every
    // hook that called it. Surface the "not found" message to stderr (still
    // visible) and exit 0 — consistent with the existing empty-result branch
    // below, which also prints a "(no … match)" line and returns Ok. Genuine
    // bad-arguments (source AND target both passed) still bail above with a
    // non-zero exit. trace:BUG-573 | ai:claude
    //
    // For target mode we need the target's UUID up front to compare against
    // each edge's target_id field.
    let target_uuid: Option<uuid::Uuid> = if let Some(t) = target {
        match backend.get_requirement_by_spec_id(t)? {
            Some(req) => Some(req.id),
            None => {
                eprintln!("{}", not_found::requirement_not_found(t, Some(store_path)));
                return Ok(());
            }
        }
    } else {
        None
    };

    // For source mode the legacy behavior is "list relationships for one
    // requirement"; we keep that shape, just routed through the same
    // emitter. trace:TASK-65 | ai:claude
    let source_uuid: Option<uuid::Uuid> = if let Some(s) = source {
        match backend.get_requirement_by_spec_id(s)? {
            Some(req) => Some(req.id),
            None => {
                eprintln!("{}", not_found::requirement_not_found(s, Some(store_path)));
                return Ok(());
            }
        }
    } else {
        None
    };

    // Index reqs by uuid for fast target-resolution during printing.
    use std::collections::HashMap;
    let req_by_uuid: HashMap<uuid::Uuid, &aida_core::Requirement> =
        all_reqs.iter().map(|r| (r.id, r)).collect();

    // Collect matching edges as (from_req, &Relationship, target_req_or_none).
    let mut rows: Vec<RelRow> = Vec::new();
    let mut hidden_terminal = 0usize;
    for req in &all_reqs {
        if let Some(s) = source_uuid {
            if req.id != s {
                continue;
            }
        }
        for rel in &req.relationships {
            if let Some(t) = target_uuid {
                if rel.target_id != t {
                    continue;
                }
            }
            if let Some(ref tf) = type_filter_norm {
                if !rel_type_matches(&rel.rel_type, tf) {
                    continue;
                }
            }
            let target_req = req_by_uuid.get(&rel.target_id).copied();
            if dangling_only && target_req.is_some() {
                continue;
            }
            // Hide edges where both endpoints are terminal in global mode
            // (no specific source/target asked). When the user explicitly
            // names a source or target they get the full picture for that
            // node regardless of status. trace:TASK-65 | ai:claude
            let global_mode = source_uuid.is_none() && target_uuid.is_none();
            if global_mode && !include_all {
                let from_terminal = is_terminal_status(&req.status);
                let to_terminal = target_req
                    .map(|t| is_terminal_status(&t.status))
                    .unwrap_or(false);
                if from_terminal && to_terminal {
                    hidden_terminal += 1;
                    continue;
                }
            }
            rows.push(RelRow {
                from_spec: req.spec_id.clone().unwrap_or_else(|| "?".into()),
                from_status: req.status.to_string(),
                rel_type: rel.rel_type.clone(),
                target_id_uuid: rel.target_id,
                target_spec: target_req
                    .and_then(|t| t.spec_id.clone())
                    .unwrap_or_else(|| {
                        // BUG-53: tombstone form for deleted targets.
                        let s = rel.target_id.to_string();
                        format!("{}…", &s[..s.len().min(8)])
                    }),
                target_status: target_req.map(|t| t.status.to_string()),
                target_title: target_req
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| "(removed — see `aida doctor verify-relationships`)".into()),
                target_resolved: target_req.is_some(),
            });
        }
    }

    if rows.is_empty() {
        let mode = if target_uuid.is_some() {
            "incoming"
        } else if source_uuid.is_some() {
            "outgoing"
        } else {
            "global"
        };
        println!(
            "{}",
            format!("(no {mode} relationships match the filter)").yellow()
        );
        if hidden_terminal > 0 {
            println!(
                "{}",
                format!(
                    "  ({hidden_terminal} hidden between Completed/Rejected reqs — pass --all to see them)"
                )
                .dimmed()
            );
        }
        return Ok(());
    }

    // Apply the row cap. An explicit --limit (when non-zero) always wins;
    // otherwise the unfiltered global listing auto-caps to avoid a firehose.
    // A source/target/type/dangling filter lifts the auto-cap. trace:TASK-778
    let total_rows = rows.len();
    let is_filtered =
        source.is_some() || target.is_some() || type_filter.is_some() || dangling_only;
    let effective_cap: Option<usize> = match limit {
        Some(0) => None,                               // explicit "no cap"
        Some(n) => Some(n),                            // explicit cap
        None if !is_filtered => Some(GLOBAL_AUTO_CAP), // unfiltered global firehose
        None => None,                                  // filtered: show all matches
    };
    let mut truncated_by: Option<(usize, bool)> = None; // (shown, was_auto_cap)
    if let Some(cap) = effective_cap {
        if rows.len() > cap {
            rows.truncate(cap);
            truncated_by = Some((cap, limit.is_none()));
        }
    }

    // Column widths sized to data.
    let from_w = rows
        .iter()
        .map(|r| r.from_spec.len())
        .max()
        .unwrap_or(8)
        .max(4);
    let type_w = rows
        .iter()
        .map(|r| rel_type_label(&r.rel_type).len())
        .max()
        .unwrap_or(8)
        .max(4);
    let to_w = rows
        .iter()
        .map(|r| r.target_spec.len())
        .max()
        .unwrap_or(10)
        .max(2);

    println!(
        "{}",
        format!(
            "{:<from_w$}  {:<type_w$}  {:<to_w$}  TITLE",
            "FROM",
            "TYPE",
            "TO",
            from_w = from_w,
            type_w = type_w,
            to_w = to_w,
        )
        .dimmed()
    );

    for r in &rows {
        let from = format!("{:<from_w$}", r.from_spec, from_w = from_w);
        let typ = rel_type_label(&r.rel_type);
        let typ_cell = format!("{:<type_w$}", typ, type_w = type_w);
        let to = format!("{:<to_w$}", r.target_spec, to_w = to_w);
        let title = shorten_title(&r.target_title, 60);
        let marker = if !r.target_resolved {
            format!("{} ", crate::glyph(crate::glyphs::Glyph::Warning))
                .red()
                .to_string()
        } else if r
            .target_status
            .as_deref()
            .map(is_terminal_status_str)
            .unwrap_or(false)
        {
            "· ".dimmed().to_string()
        } else {
            "  ".into()
        };
        println!(
            "{marker}{from}  {typ_cell}  {to}  {title}",
            title = title.dimmed()
        );
    }

    if let Some((shown, was_auto_cap)) = truncated_by {
        let hint = if was_auto_cap {
            " — pass --limit 0 for all, or a filter (--source/--target/--type)"
        } else {
            " — raise or drop --limit (0 = all) to see more"
        };
        println!("\n{} of {} edges shown{}", shown, total_rows, hint.dimmed());
    } else {
        println!("\n{} edges", rows.len());
    }
    if hidden_terminal > 0 {
        println!(
            "{}",
            format!(
                "  ({hidden_terminal} hidden between Completed/Rejected reqs — pass --all to see them)"
            )
            .dimmed()
        );
    }
    let _ = store_path; // suppress unused — already used for error context above
    Ok(())
}

/// One row emitted by `aida rel list`. Decoupling the struct from the
/// rendering loop keeps the column widths calculable in one pass.
pub(crate) struct RelRow {
    pub(crate) from_spec: String,
    #[allow(dead_code)] // surfaced through future --show-status etc.
    pub(crate) from_status: String,
    pub(crate) rel_type: RelationshipType,
    #[allow(dead_code)] // kept for future --uuid / --json renderers
    pub(crate) target_id_uuid: uuid::Uuid,
    pub(crate) target_spec: String,
    pub(crate) target_status: Option<String>,
    pub(crate) target_title: String,
    pub(crate) target_resolved: bool,
}

/// Compare a relationship-type from storage to a user-supplied filter.
/// Canonical types match by Display name (case-insensitive); custom types
/// match exact string. trace:TASK-65 | ai:claude
pub(crate) fn rel_type_matches(actual: &RelationshipType, want: &RelationshipType) -> bool {
    match (actual, want) {
        (RelationshipType::Custom(a), RelationshipType::Custom(b)) => a.eq_ignore_ascii_case(b),
        (RelationshipType::Custom(_), _) | (_, RelationshipType::Custom(_)) => false,
        (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

/// Short label for a RelationshipType in tabular output.
pub(crate) fn rel_type_label(rt: &RelationshipType) -> String {
    match rt {
        RelationshipType::Parent => "parent".to_string(),
        RelationshipType::Child => "child".to_string(),
        RelationshipType::Duplicate => "duplicate".to_string(),
        RelationshipType::Verifies => "verifies".to_string(),
        RelationshipType::VerifiedBy => "verified-by".to_string(),
        RelationshipType::References => "references".to_string(),
        // trace:STORY-333 | ai:claude
        RelationshipType::BlockedBy => "blocked-by".to_string(),
        RelationshipType::Blocks => "blocks".to_string(),
        // trace:TASK-1176 | ai:claude
        RelationshipType::SupersededBy => "superseded-by".to_string(),
        RelationshipType::Supersedes => "supersedes".to_string(),
        RelationshipType::Custom(s) => s.clone(),
    }
}

pub(crate) fn shorten_title(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Result of the STORY-63 scope-fallback resolver.
/// trace:STORY-63 | ai:claude
pub(crate) struct ScopeFallback<'a> {
    /// Total number of Approved children of the scope (informational —
    /// we display this count in the rendered message).
    pub(crate) approved_count: usize,
    /// The chosen pick after priority + created_at sort.
    pub(crate) pick: &'a Requirement,
}

/// Find the highest-priority approved child of `lease.scope` that no
/// session is already mid-work on. Returns `None` when:
///   - the scope is a path-glob / free-form string we can't resolve to
///     a spec (no ancestry to walk)
///   - any child of the scope is already InProgress (don't pick a
///     parallel within the same EPIC)
///   - no child satisfies status=Approved + the role scope filter
///
/// Priority order: High > Medium > Low, then created_at ascending so
/// the oldest approved item wins ties (closest to the project's
/// implicit work order).
/// trace:STORY-63 | ai:claude
pub(crate) fn scope_fallback_pick<'a>(
    store: &'a RequirementsStore,
    lease: &SessionLease,
    role_scope: Option<&(Vec<String>, Option<String>)>,
) -> Option<ScopeFallback<'a>> {
    let scope_lc = lease.scope.to_ascii_lowercase();
    let scope_req = store.requirements.iter().find(|r| {
        let spec_match = r
            .spec_id
            .as_deref()
            .map(|s| s.to_ascii_lowercase() == scope_lc)
            .unwrap_or(false);
        let agreed_match = r
            .agreed_id
            .as_deref()
            .map(|s| s.to_ascii_lowercase() == scope_lc)
            .unwrap_or(false);
        spec_match || agreed_match
    })?;

    let child_ids: HashSet<Uuid> = scope_req
        .relationships
        .iter()
        .filter(|r| r.rel_type == RelationshipType::Parent)
        .map(|r| r.target_id)
        .collect();
    if child_ids.is_empty() {
        return None;
    }
    let children: Vec<&Requirement> = store
        .requirements
        .iter()
        .filter(|r| child_ids.contains(&r.id))
        .collect();

    // If anything under this scope is already in flight, the session's
    // attention belongs to that — don't suggest a second item.
    if children
        .iter()
        .any(|r| r.status == RequirementStatus::InProgress)
    {
        return None;
    }

    let approved_count = children
        .iter()
        .filter(|r| r.status == RequirementStatus::Approved)
        .count();

    let mut candidates: Vec<&Requirement> = children
        .iter()
        .copied()
        .filter(|r| r.status == RequirementStatus::Approved)
        .filter(|r| {
            // Apply the active role's tag/status scope filter if present
            // (mirrors the existing queue-next post-filter so the two
            // entry points behave consistently).
            let Some((scope_tags, scope_status)) = role_scope else {
                return true;
            };
            if let Some(want) = scope_status {
                if !format!("{}", r.status).eq_ignore_ascii_case(want)
                    && !format!("{:?}", r.status).eq_ignore_ascii_case(want)
                {
                    return false;
                }
            }
            for tag in scope_tags {
                if !r.tags.iter().any(|t| t == tag) {
                    return false;
                }
            }
            true
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| {
        priority_rank(&a.priority)
            .cmp(&priority_rank(&b.priority))
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    Some(ScopeFallback {
        approved_count,
        pick: candidates[0],
    })
}

/// Sort key for priority — lower rank wins (High = 0, Medium = 1,
/// Low = 2) so an ascending sort puts High first.
/// trace:STORY-63 | ai:claude
pub(crate) fn priority_rank(p: &RequirementPriority) -> u8 {
    match p {
        RequirementPriority::High => 0,
        RequirementPriority::Medium => 1,
        RequirementPriority::Low => 2,
    }
}

/// Render `root` and its descendants as an indented tree, two spaces per
/// level. Each node prints as `<status-glyph> <ID>  <Status>  <title>`.
/// Children are walked via rel_type:Parent edges (AIDA's
/// parent-points-at-child storage convention). Recursion stops at
/// `max_depth` (the root is depth 0). Cycles are guarded by a visited
/// set — defensive only; the data model shouldn't allow them.
/// trace:STORY-62 | ai:claude
pub(crate) fn render_tree(
    backend: &aida_core::CachedGitBackend,
    root: &Requirement,
    max_depth: usize,
) -> Result<()> {
    let mut visited: HashSet<Uuid> = HashSet::new();
    render_tree_node(backend, root, 0, max_depth, &mut visited)
}

pub(crate) fn render_tree_node(
    backend: &aida_core::CachedGitBackend,
    req: &Requirement,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<Uuid>,
) -> Result<()> {
    if !visited.insert(req.id) {
        return Ok(()); // already rendered — treat as cycle, skip silently
    }
    let indent = "  ".repeat(depth);
    let status = req.effective_status().to_string();
    // Glyph hint without emoji (CLAUDE.md house rule): two-state mark on
    // the most useful axis — completed vs everything else.
    let glyph = if status.eq_ignore_ascii_case("completed") {
        crate::glyph(crate::glyphs::Glyph::Check)
            .green()
            .to_string()
    } else {
        "○".dimmed().to_string()
    };
    let id_label = req.display_id();
    println!(
        "{}{} {:<14} {:<12} {}",
        indent, glyph, id_label, status, req.title,
    );
    if depth >= max_depth {
        return Ok(());
    }
    // Collect direct children (Parent edges on this req point to children).
    let mut children: Vec<Requirement> = Vec::new();
    for rel in &req.relationships {
        if rel.rel_type == RelationshipType::Parent {
            if let Some(child) = backend.get_requirement(&rel.target_id)? {
                children.push(child);
            }
        }
    }
    // Stable order: by display_id ascending so the same tree prints the
    // same way across runs.
    children.sort_by_key(|a| a.display_id());
    for child in &children {
        render_tree_node(backend, child, depth + 1, max_depth, visited)?;
    }
    Ok(())
}

pub(crate) fn print_comment(comment: &Comment, indent: usize) {
    print_comment_with(comment, indent, None);
}

/// [`print_comment`], rendering each body as terminal markdown laid out for
/// `markdown_width` columns when it is `Some` (human output at a TTY, not
/// `--raw`). `None` prints the body exactly as stored.
// trace:STORY-1652 | ai:claude
pub(crate) fn print_comment_with(comment: &Comment, indent: usize, markdown_width: Option<usize>) {
    let indent_str = "  ".repeat(indent);
    println!();
    println!("{}{}:", indent_str, comment.id.to_string().yellow());
    let edited_marker = if comment.modified_at > comment.created_at {
        format!(
            " (edited {})",
            comment
                .modified_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
        )
        .dimmed()
        .to_string()
    } else {
        String::new()
    };
    println!(
        "{}  {} {} at {}{}",
        indent_str,
        "By:".dimmed(),
        comment.author.cyan(),
        comment
            .created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
            .dimmed(),
        edited_marker,
    );
    // trace:TASK-330 | ai:claude — surface the producing session so a reader
    // can correlate the comment back to a session (`aida session list`).
    if let Some(short) = comment.short_session_id() {
        println!("{}  {} {}", indent_str, "Session:".dimmed(), short.dimmed());
    }
    // trace:BUG-1534 | ai:claude — a relayed claim names its original seat.
    if let Some(orig) = comment.relayed_from.as_deref() {
        println!(
            "{}  {} {}",
            indent_str,
            "Relayed:".dimmed(),
            aida_core::mailbox::provenance_label(&comment.author, Some(orig)).magenta()
        );
    }
    match markdown_width {
        Some(width) => {
            let body_indent = format!("{indent_str}  ");
            let rendered = crate::markdown_render::render_markdown_for_terminal(
                &comment.content,
                width.saturating_sub(body_indent.len()),
            );
            for line in rendered.lines() {
                if line.is_empty() {
                    println!();
                } else {
                    println!("{body_indent}{line}");
                }
            }
        }
        None => println!("{}  {}", indent_str, comment.content),
    }

    if !comment.replies.is_empty() {
        for reply in &comment.replies {
            print_comment_with(reply, indent + 1, markdown_width);
        }
    }
}

/// Resolve a user-supplied comment identifier into a concrete Uuid by
/// matching against the requirement's comment tree. Accepts:
/// - a full UUID string (e.g., `019df478-7a34-7f92-8d46-b00e0d1eeda7`)
/// - a UUID prefix (e.g., `019df478`) — must uniquely match one comment
///   Returns an error on no-match or ambiguous-prefix.
///   trace:SPIKE-2 | ai:claude
pub(crate) fn resolve_comment_uuid(req: &aida_core::Requirement, query: &str) -> Result<Uuid> {
    if let Ok(parsed) = Uuid::parse_str(query) {
        // Verify the exact UUID exists in the tree, even if parse succeeded.
        if collect_comment_ids(&req.comments)
            .into_iter()
            .any(|id| id == parsed)
        {
            return Ok(parsed);
        }
        anyhow::bail!("No comment with id {} on this requirement", parsed);
    }

    let q = query.to_lowercase();
    let matches: Vec<Uuid> = collect_comment_ids(&req.comments)
        .into_iter()
        .filter(|id| id.to_string().to_lowercase().starts_with(&q))
        .collect();
    match matches.len() {
        0 => anyhow::bail!(
            "No comment matches '{}' on this requirement (use `aida comment list <REQ>` to see ids)",
            query
        ),
        1 => Ok(matches[0]),
        n => anyhow::bail!(
            "Ambiguous comment prefix '{}' — matches {} comments. Use a longer prefix.",
            query,
            n
        ),
    }
}

/// Walk the (potentially nested) comment tree and collect every comment id.
pub(crate) fn collect_comment_ids(comments: &[Comment]) -> Vec<Uuid> {
    let mut out = Vec::new();
    for c in comments {
        out.push(c.id);
        out.extend(collect_comment_ids(&c.replies));
    }
    out
}

/// Section headings the prompt-generator looks for in a requirement's
/// description. The first match (case-insensitive) wins; everything from
/// that heading until the next `## ` heading or end-of-string is the
/// extracted body.
/// trace:STORY-67 | ai:claude
pub(crate) const ACCEPTANCE_SECTION_HEADINGS: &[&str] = &[
    "Acceptance",
    "Verify",
    "Test cases",
    "Tests",
    "Verification",
];

/// Density levels for the `aida show --card` spec card. trace:TASK-265
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardDensity {
    /// One-line id/type/priority/status/title summary, no box. For
    /// autonomous / scripted flows that just need the spec named.
    Brief,
    /// Boxed card: a truncated lead-prose description + the acceptance
    /// criteria. The default.
    Balanced,
    /// Boxed card with the full description, no truncation. For deep
    /// dives.
    Full,
}

/// The headings the spec card renders relationships under, in render order.
///
/// This list is the card's completeness contract: every label
/// [`card_rel_label`] can return MUST appear here. A label with no matching
/// heading does not render under a rough heading — it does not render at all.
/// That is how relabelling `Custom` edges without adding this third bucket
/// made every stored `Custom` edge (≈ 1480 of them store-wide, including this
/// spec's own `implemented-by`) vanish from the card: a wrong label traded for
/// no label, which is BUG-1471's own defect class.
// trace:BUG-1471 | ai:claude
pub(crate) const CARD_REL_BUCKETS: &[&str] = &["Parent", "Related", "Custom"];

/// Map a relationship type to the field label the spec card buckets it
/// under. AIDA's `RelationshipType` reads as "I am X to the target", so a
/// `Child` edge means the target is this spec's parent.
///
/// Every value returned here must be listed in [`CARD_REL_BUCKETS`].
// trace:TASK-265 | ai:claude
pub(crate) fn card_rel_label(rt: &RelationshipType) -> &'static str {
    match rt {
        // This spec is a child of the target → target is the parent.
        RelationshipType::Child => "Parent",
        // Custom edges must not be presented with a label that looks like a
        // standard type: that taught users to type `--type related`, creating
        // graph-inert edges. They get their own bucket instead, with the
        // edge's real name shown inline by `CardRel::render`.
        // trace:BUG-1471 | ai:codex
        RelationshipType::Custom(_) => "Custom",
        // Standard non-parent edges share the general Related bucket.
        _ => "Related",
    }
}

/// The edge's own name, when the bucket heading does not already carry it.
///
/// `Child` is the sole exception because its `Parent` heading already names
/// the target's role. Custom and standard edges under neutral headings print
/// their canonical names, so blocking, verification, reference, duplicate,
/// and supersession edges cannot collapse into indistinguishable target ids.
// trace:BUG-1471 trace:BUG-1584 | ai:codex
pub(crate) fn card_rel_edge_name(rt: &RelationshipType) -> Option<String> {
    match rt {
        RelationshipType::Child => None,
        _ => Some(rt.to_string()),
    }
}

/// One relationship as the spec card renders it: the heading it falls under,
/// the edge's own name when that heading does not carry it, and the resolved
/// target.
// trace:BUG-1471 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CardRel {
    pub(crate) bucket: &'static str,
    pub(crate) edge: Option<String>,
    pub(crate) id: String,
    pub(crate) title: String,
}

impl CardRel {
    /// Build a card row from a relationship type and its already-resolved
    /// target id/title.
    // trace:BUG-1471 | ai:claude
    pub(crate) fn new(rt: &RelationshipType, id: String, title: String) -> Self {
        CardRel {
            bucket: card_rel_label(rt),
            edge: card_rel_edge_name(rt),
            id,
            title,
        }
    }

    /// One rendered entry: `[edge-name] ID — title`, with the bracketed name
    /// present only for an edge whose heading does not name it.
    // trace:BUG-1471 | ai:claude
    pub(crate) fn render(&self) -> String {
        let target = if self.title.is_empty() {
            self.id.clone()
        } else {
            format!("{} — {}", self.id, self.title)
        };
        match &self.edge {
            Some(name) => format!("[{name}] {target}"),
            None => target,
        }
    }
}

/// Group a card's relationships into the headed sections it prints, in
/// [`CARD_REL_BUCKETS`] order, skipping empty ones.
///
/// Rendering through this one function is what keeps the card total. The
/// previous shape hand-rolled one filter per heading at the call site, so
/// adding a label without adding its heading dropped those edges on the floor
/// with nothing to notice it.
// trace:BUG-1471 | ai:claude
pub(crate) fn card_rel_sections(rels: &[CardRel]) -> Vec<(&'static str, String)> {
    let mut sections = Vec::new();
    for bucket in CARD_REL_BUCKETS {
        let joined = rels
            .iter()
            .filter(|r| r.bucket == *bucket)
            .map(CardRel::render)
            .collect::<Vec<_>>()
            .join(", ");
        if !joined.is_empty() {
            sections.push((*bucket, joined));
        }
    }
    sections
}

/// The lead prose of a requirement description — everything before the
/// first `## ` section heading. AIDA descriptions front-load a plain
/// summary, then break into `## Acceptance`, `## Origin`, etc.; the
/// balanced card shows just that summary. trace:TASK-265 | ai:claude
pub(crate) fn card_lead_prose(description: &str) -> &str {
    let mut from = 0usize;
    while let Some(rel) = description[from..].find("## ") {
        let abs = from + rel;
        // Only treat it as a heading when it starts a line.
        if abs == 0 || description.as_bytes()[abs - 1] == b'\n' {
            return description[..abs].trim_end();
        }
        from = abs + 3;
    }
    description.trim_end()
}

/// Truncate text to at most `max_paragraphs` blank-line-separated
/// paragraphs and roughly `max_chars` characters, never cutting inside a
/// paragraph (so the result never ends mid-sentence). The first paragraph
/// is always kept even if it alone exceeds the char budget. Returns the
/// kept text and whether anything was dropped. trace:TASK-265 | ai:claude
pub(crate) fn card_truncate_paragraphs(
    text: &str,
    max_paragraphs: usize,
    max_chars: usize,
) -> (String, bool) {
    let paragraphs: Vec<&str> = text
        .split("\n\n")
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut total = 0usize;
    for (i, para) in paragraphs.iter().enumerate() {
        if i >= max_paragraphs {
            break;
        }
        if i > 0 && total + para.chars().count() > max_chars {
            break;
        }
        kept.push(para);
        total += para.chars().count();
    }
    let truncated = kept.len() < paragraphs.len();
    (kept.join("\n\n"), truncated)
}

/// Count `- [ ]` / `* [ ]` checklist items in an acceptance-section body.
/// trace:TASK-265 | ai:claude
pub(crate) fn card_count_acceptance(body: &str) -> usize {
    body.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("- [") || t.starts_with("* [")
        })
        .count()
}

/// Lead the spec card's brief with the spec's cached `aida intent`
/// comprehension (TASK-838). When a FRESH comprehension exists, render its
/// `llm` register at the top of the brief, clearly labeled AI-generated, ahead
/// of the description and plan brief. When absent or stale, emit a one-line
/// nudge and continue — NEVER block pickup on an AI pass, never generate inline.
///
/// Computes the fresh neighborhood hash exactly as `aida intent` does (reusing
/// [`build_intent_neighborhood`] + [`intent::is_stale`] via
/// [`intent::decide_pickup_intent`]), so the brief and `aida intent` agree on
/// drift. Feature-detects the `Option<SpecIntent>`: a store predating STORY-631
/// (or a spec never run through `aida intent`) takes the absent-note path with
/// no other behavior change. Store-load failure is non-fatal — the card is a
/// convenience surface, so we silently skip the intent block rather than abort
/// the whole card. trace:TASK-838 | ai:claude
pub(crate) fn render_card_intent(req: &aida_core::Requirement, store_path: &std::path::Path) {
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    // The full requirement set lets build_intent_neighborhood resolve the
    // spec's immediate neighbors for the same drift hash `aida intent` computes.
    let Some(store) = load_store_for_lookup(&project_root) else {
        return;
    };
    let inputs = build_intent_neighborhood(req, &store.requirements);
    let fresh_hash = inputs.source_hash();
    let disp = req.display_id();

    match intent::decide_pickup_intent(req.intent.as_ref(), &fresh_hash) {
        intent::PickupIntent::Render {
            llm,
            model,
            generated_at,
        } => {
            // Lead the brief. Label it AI-generated so no reader mistakes the
            // prose for hand-authored ground truth.
            println!(
                "  {}",
                format!(
                    "{} Intent (AI-generated):",
                    crate::glyph(crate::glyphs::Glyph::Arrow)
                )
                .bold()
            );
            println!(
                "    {}",
                format!("model={model} · generated {generated_at}").dimmed()
            );
            for line in llm.trim().lines() {
                println!("    {line}");
            }
            println!();
        }
        intent::PickupIntent::Note { stale } => {
            println!(
                "  {} {}",
                format!("{} Intent:", crate::glyph(crate::glyphs::Glyph::Arrow)).bold(),
                intent::pickup_intent_note(&disp, stale).dimmed()
            );
            println!();
        }
    }
}

/// Render a requirement as a compact, boxed "spec card" — the rendering
/// behind `aida show --card`. The /aida-pickup skill calls this at session
/// start so the spec's contract stays in terminal scrollback for the whole
/// working session, with no separate `aida show`. The plain `aida show`
/// view remains the canonical detail surface.
///
/// `rels` is the requirement's relationships already resolved by the
/// caller to [`CardRel`] rows.
// trace:TASK-265 | ai:claude
pub(crate) fn render_spec_card(
    req: &aida_core::Requirement,
    rels: &[CardRel],
    store_path: &std::path::Path,
    density: CardDensity,
    no_git: bool,
    verbose: bool,
) {
    let id = req.display_id();
    let req_type = req.req_type.to_string();
    let priority = req.effective_priority();

    // Brief: a single line, no box — for autonomous / scripted flows.
    // TASK-269: badge the status (glyph + colour) here too. trace:TASK-269
    if density == CardDensity::Brief {
        println!(
            "{} · {} · {} · {} · {}",
            id,
            req_type,
            priority,
            status_display::parked_status_badge(req),
            req.title
        );
        return;
    }

    const WIDTH: usize = 72;
    let rule = "═".repeat(WIDTH);

    // Header: a flow-active marker then `<ID> — <title>`, title trimmed to keep the banner tidy.
    let prefix = format!(
        "{} {} — ",
        crate::glyph(crate::glyphs::Glyph::FlowActive),
        id
    );
    let budget = WIDTH.saturating_sub(prefix.chars().count());
    let title = if req.title.chars().count() > budget {
        let t: String = req.title.chars().take(budget.saturating_sub(1)).collect();
        format!("{}…", t)
    } else {
        req.title.clone()
    };
    println!("{}", rule.dimmed());
    println!("{}{}", prefix.cyan().bold(), title.cyan().bold());
    println!("{}", rule.dimmed());
    println!();

    // One-liner: ID · type · priority · status (badged — TASK-269).
    // STORY-333: surface `[human-only]` chip when the marker is set, so a
    // reader scanning the spec card can see at a glance why the orchestrator
    // skips it. trace:STORY-333 | ai:claude
    let human_only_chip = if req.human_only {
        format!(" · {}", "[human-only]".magenta().bold())
    } else {
        String::new()
    };
    println!(
        "  {} · {} · {} · {}{}",
        id.bold(),
        req_type,
        priority,
        status_display::parked_status_badge(req),
        human_only_chip,
    );
    println!();

    // Key fields. "Uncategorized" is the default feature — suppress it
    // so the card only shows a feature the user actually set.
    let mut printed_field = false;
    if !req.feature.is_empty() && req.feature != "Uncategorized" {
        println!(
            "  {} {}",
            format!("{} Feature:", crate::glyph(crate::glyphs::Glyph::Arrow)).bold(),
            req.feature
        );
        printed_field = true;
    }
    if !req.tags.is_empty() {
        let tags: Vec<String> = req.tags.iter().cloned().collect();
        println!(
            "  {} {}",
            format!("{} Tags:", crate::glyph(crate::glyphs::Glyph::Arrow)).bold(),
            tags.join(", ")
        );
        printed_field = true;
    }
    // BUG-1471: render EVERY heading the card defines, from one place. A
    // reader treats this block as the spec's relationship list, so an edge
    // that lands in no bucket is worse than one under a rough heading — it
    // is silently absent. trace:BUG-1471 | ai:claude
    for (heading, joined) in card_rel_sections(rels) {
        println!(
            "  {} {}",
            format!("{} {}:", crate::glyph(crate::glyphs::Glyph::Arrow), heading).bold(),
            joined
        );
        printed_field = true;
    }
    if printed_field {
        println!();
    }

    // TASK-838: lead the implementer brief with the spec's cached `aida intent`
    // comprehension — the distilled plain-terms WHY-this-exists, ahead of the
    // raw spec body and the plan brief (Critical Files / Followups). This is
    // THIN linking work: surface the already-cached `llm`-register prose, NEVER
    // generate inline (pickup must stay fast and non-AI-blocking). A spec with
    // no cached comprehension, or a STALE one (neighborhood hash drifted),
    // gets a single-line nudge toward `aida intent <spec>` and we continue.
    // Feature-detects the `Option<SpecIntent>`, so a store without STORY-631
    // intent sees only the absent note. trace:TASK-838 | ai:claude
    render_card_intent(req, store_path);

    // STORY-332: a punted spec carries its fork reason — surface it on the
    // card so a triager sees the contract and the obstacle together.
    if let Some(reason) = req.attention_reason.as_ref() {
        println!(
            "  {} {}",
            format!("{} Punted:", crate::glyph(crate::glyphs::Glyph::Warning))
                .magenta()
                .bold(),
            reason.category
        );
        println!("    {}", reason.detail);
        if let Some(lean) = &reason.lean {
            println!(
                "    {}",
                format!(
                    "{} lean: {lean}",
                    crate::glyph(crate::glyphs::Glyph::SubArrow)
                )
                .dimmed()
            );
        }
        println!();
    }

    // Description: full text for --full, otherwise the lead prose
    // truncated to ~3 paragraphs / ~500 chars on a paragraph boundary.
    // Brief already returned above; only Balanced and Full reach here.
    let (desc_body, desc_label) = if density == CardDensity::Full {
        (
            req.description.trim().to_string(),
            format!("{} Description:", crate::glyph(crate::glyphs::Glyph::Arrow)),
        )
    } else {
        let (body, truncated) = card_truncate_paragraphs(card_lead_prose(&req.description), 3, 500);
        let body = if truncated && !body.is_empty() {
            format!("{}\n\n…", body)
        } else {
            body
        };
        (
            body,
            format!(
                "{} Description (summary):",
                crate::glyph(crate::glyphs::Glyph::Arrow)
            ),
        )
    };
    if !desc_body.is_empty() {
        println!("  {}", desc_label.bold());
        for line in desc_body.lines() {
            println!("    {}", line);
        }
        println!();
    }

    // TASK-1148: the three optional narrative fields — the genuinely-new
    // metadata not derivable from git/status/trace. Each renders its own
    // labelled block only when set; absent fields print nothing.
    // trace:TASK-1148 | ai:claude
    let narrative_blocks: [(&str, &Option<String>); 3] = [
        ("Implementation summary", &req.implementation_summary),
        ("Risk notes", &req.risk_notes),
        ("Test coverage notes", &req.test_coverage_notes),
    ];
    for (label, value) in narrative_blocks {
        if let Some(text) = value {
            let text = text.trim();
            if !text.is_empty() {
                println!(
                    "  {}",
                    format!("{} {label}:", crate::glyph(crate::glyphs::Glyph::Arrow)).bold()
                );
                for line in text.lines() {
                    println!("    {}", line);
                }
                println!();
            }
        }
    }

    // Acceptance criteria as their own block (the --full description
    // already carries the section verbatim, so skip it there).
    if density == CardDensity::Balanced {
        if let Some(acc) = extract_acceptance_section(&req.description) {
            let n = card_count_acceptance(&acc);
            let label = if n == 1 {
                "1 item".to_string()
            } else {
                format!("{} items", n)
            };
            println!(
                "  {}",
                format!(
                    "{} Acceptance ({}):",
                    crate::glyph(crate::glyphs::Glyph::Arrow),
                    label
                )
                .bold()
            );
            for line in acc.lines() {
                let t = line.trim_end();
                if !t.is_empty() {
                    println!("    {}", t);
                }
            }
            println!();
        }
    }

    // TASK-313: the owning plan's brief (Critical Files / Followups /
    // Verification) inside the card box — the same brief /aida-pickup surfaces,
    // reusing `aida queue work`'s plan discovery (discover_plan_context) rather
    // than duplicating it. Renders only when a docs/plans/ file owns the spec.
    // trace:TASK-313 | ai:claude
    {
        let project_root = store_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let plan = discover_plan_context(&project_root, &req.display_id()).or_else(|| {
            req.spec_id
                .as_deref()
                .and_then(|s| discover_plan_context(&project_root, s))
        });
        if let Some(ctx) = plan {
            println!(
                "  {} {}",
                format!("{} Plan brief:", crate::glyph(crate::glyphs::Glyph::Arrow)).bold(),
                ctx.plan_file.cyan()
            );
            if !ctx.critical_files.is_empty() {
                println!(
                    "    {} ({})",
                    "Critical files".dimmed(),
                    ctx.critical_files.len()
                );
                for f in &ctx.critical_files {
                    println!("      {}", f);
                }
            }
            if !ctx.followups.is_empty() {
                println!("    {} ({})", "Followups".dimmed(), ctx.followups.len());
                for f in &ctx.followups {
                    println!("      - {}", f);
                }
            }
            if let Some(v) = &ctx.verification {
                println!("    {}", "Definition of done".dimmed());
                for l in v.lines() {
                    println!("      {}", l.dimmed());
                }
            }
            println!();
        }
    }

    // Git linkage — reuse the same section `aida show` appends, grepped
    // against every id form the spec has worn. trace:TASK-241
    if !no_git {
        let mut ids: Vec<String> = vec![req.display_id()];
        if let Some(ref a) = req.agreed_id {
            if !ids.contains(&a.to_string()) {
                ids.push(a.to_string());
            }
        }
        if let Some(ref o) = req.spec_id {
            if !ids.contains(o) {
                ids.push(o.clone());
            }
        }
        let project_root = store_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        // trace:BUG-1993 | ai:claude
        let _phase = crate::usage::phase("linkage");
        print_git_linkage(&project_root, &ids, verbose, req.filed_at.as_ref());
    }
    println!("{}", rule.dimmed());
}
