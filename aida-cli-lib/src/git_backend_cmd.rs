//! `aida db` git-backend command cluster — the storage/git-backend command
//! handler (`handle_git_backend_command`), extracted from `main.rs` (SPIKE-78
//! / TASK-1151; pure movement, no behavior change). Shared store/git helpers
//! stay in `main.rs` and are reached via `crate::`.

#![allow(clippy::too_many_arguments)]

use anyhow::{Context, Result};
use serde::Serialize;

use crate::*;

// trace:TASK-190 | ai:codex
pub(crate) fn resolve_comment_body(
    content: Option<String>,
    body_file: Option<&std::path::Path>,
    stdin: bool,
) -> Result<String> {
    if let Some(path) = body_file {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read comment body from {}", path.display()))?;
        anyhow::ensure!(!body.trim().is_empty(), "comment body file is empty");
        return Ok(body);
    }
    if stdin {
        let mut body = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut body)
            .context("failed to read comment body from stdin")?;
        anyhow::ensure!(!body.trim().is_empty(), "comment body from stdin is empty");
        return Ok(body);
    }
    Ok(content.unwrap_or_default())
}

const PROXY_APPROVAL_MARKER: &str = "[aida:proxy-approval]";

/// TASK-1456: annotate the Done rows `select_done_rework_rows` folded into
/// the open lens. A bare `Done` row in the table would just read as ordinary
/// (if slightly confusing, since Done isn't usually in this view)
/// awaiting-merge work — this makes the state explicit: which spec, and the
/// working recovery route (mirrors `QueueFreshPickup::AwaitingRework`'s hint
/// from the spec that first taught this state).
///
/// Perf (review follow-up): takes the `rework_ids` set `select_done_rework_rows`
/// already computed — a plain membership check — instead of recomputing
/// `done_spec_outstanding_refusal` per row. The earlier version's per-row
/// recompute repeated the (git-worktree-resolving) refusal check at every
/// render site, on top of the one `select_done_rework_rows` already did to
/// decide what to fold in, undermining the sub-second `aida list` contract.
// trace:TASK-1456 | ai:claude
pub(crate) fn print_rework_needed_notes(
    reqs: &[aida_core::RequirementSummary],
    rework_ids: &std::collections::HashSet<uuid::Uuid>,
) {
    if rework_ids.is_empty() {
        return;
    }
    let mut printed_header = false;
    for r in reqs {
        if !rework_ids.contains(&r.id) {
            continue;
        }
        if !printed_header {
            println!("\n{}", "Rework needed:".bold());
            printed_header = true;
        }
        let display_id = r
            .agreed_id
            .as_deref()
            .or(r.spec_id.as_deref())
            .unwrap_or("?");
        println!(
            "  {}  {} — `{}`",
            display_id.bold(),
            "reviewer requested changes; still outstanding".magenta(),
            format!("aida queue rework {display_id}").cyan(),
        );
    }
}

/// BUG-1771: the parked lens of one `aida list` row.
///
/// Every list surface that cares about the lens — the `--format json`
/// `status_lens` field and the `--status shelved` / `--status needs-decision`
/// filters — goes through here, so a row the filter selects can never be
/// labelled as the other lens. The read is a single targeted object fetch and
/// is gated on the row's stored status, so a list with no parked rows pays
/// nothing and a filtered one pays only for the parked set — small by nature,
/// since a parked spec is one waiting on a human.
///
/// Known limit, deliberately inherited rather than introduced: this reads the
/// ROW's own parked fields, so an EPIC whose derived status is NeedsAttention
/// only because a child is parked has no lens here. That is exactly what the
/// `--format json` `status_lens` field already reported before BUG-1771, and
/// making the filter disagree with the label would be the worse bug. The legacy
/// `effective_needs_attention_lens_with_source` in `lib.rs` does walk children;
/// unifying the two is its own change, on its own spec.
// trace:TASK-1572 | ai:claude
// trace:BUG-1798 | ai:claude
pub(crate) fn row_parked_lens(
    backend: &aida_core::CachedGitBackend,
    lazy_store: &mut Option<aida_core::RequirementsStore>,
    row: &aida_core::RequirementSummary,
) -> Option<status_display::NeedsAttentionLens> {
    if !row.status.eq_ignore_ascii_case("NeedsAttention") {
        return None;
    }

    // For non-epics, we don't need to walk children, so a single read suffices
    if !row.req_type.eq_ignore_ascii_case("epic") {
        let req = backend.get_requirement(&row.id).ok().flatten()?;
        return status_display::needs_attention_lens(&req);
    }

    // For epics, we must walk children, which requires the full store.
    let store = lazy_store.get_or_insert_with(|| {
        aida_core::db::DatabaseBackend::load(backend)
            .unwrap_or_else(|_| backend.load_metadata_only().unwrap_or_default())
    });

    let req = store.get_requirement_by_id(&row.id)?;
    crate::effective_needs_attention_lens_with_source(
        store,
        req,
        &aida_core::RequirementStatus::NeedsAttention,
    )
    .map(|(_, lens)| lens)
}

/// TASK-1456: `aida list --format json`'s `status_label`/`status_lens`
/// value for a row — either the folded-in-as-rework annotation (takes
/// priority; a row is never simultaneously Done and NeedsAttention) or the
/// pre-existing NeedsAttention parked lens. Extracted as a pure function so
/// the precedence and exact label/key are directly testable without
/// spinning up the list command.
// trace:TASK-1456 | ai:claude
pub(crate) fn list_json_status_label_lens(
    rework: bool,
    parked_lens: Option<&status_display::NeedsAttentionLens>,
) -> (Option<String>, Option<&'static str>) {
    if rework {
        return (Some("Rework Needed".to_string()), Some("ReworkNeeded"));
    }
    (
        parked_lens.map(|lens| lens.label()),
        parked_lens.map(|lens| lens.palette_key()),
    )
}

pub(crate) fn terminal_list_width() -> Option<usize> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        return None;
    }
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
}

pub(crate) struct ListTableOptions {
    pub(crate) show_origin: bool,
    pub(crate) show_tags: bool,
    pub(crate) no_glyph: bool,
    pub(crate) flow_header: String,
    pub(crate) terminal_width: Option<usize>,
    /// Derived driver label per in-progress row (`relay:lib-split`,
    /// `idle 26h`), shown beside the status. Empty → no driver column.
    // trace:STORY-1653 | ai:claude
    pub(crate) drivers: std::collections::HashMap<Uuid, String>,
}

/// Resolve the two whole-store facts used by `aida show` from the cache.
///
/// The cache already projects an EPIC's derived status and the durable
/// `serialize:<group>` tags used to sequence collision-prone batch members.
/// Keeping this helper free of `backend.load()` and live repository scans is
/// the load-bearing part of the single-spec read latency contract.
// trace:TASK-1268 trace:BUG-1480 | ai:codex
/// The cache-backed reads `aida show`'s canonical path is allowed to make.
///
/// COMPILER-ENFORCED, NOT TEXT-MATCHED. The regression this guards was a
/// `backend.load()` on the single-spec show path — a full-store scan costing
/// ~10s against a 4200-object store where the cache answers in under half a
/// second. The previous guard was a test that `include_str!`d this file, split
/// it on two literal markers, and asserted the substring `backend.load()` was
/// absent. That check breaks on any reformat, proves nothing about a DIFFERENT
/// expensive call, and is itself a text match standing in for a contract.
///
/// This trait exposes only the two reads the path actually needs, so `load()`
/// is not merely discouraged there — it does not exist to be called, and
/// reintroducing it is a compile error rather than a test failure.
///
/// Scoped deliberately: this is the canonical-path context helper, which is
/// where the regression was. The COST dimension is guarded separately and on
/// the real store by `aida doctor check performance`, because a structural
/// guard cannot see a slowdown that arrives without any call site changing —
/// this bug grew from 7.5s to 9.9s with no code change at all.
// trace:BUG-1480 | ai:claude
pub(crate) trait CacheOnlyReads {
    fn list_summaries(
        &self,
        filter: &aida_core::ListFilter,
    ) -> anyhow::Result<Vec<aida_core::RequirementSummary>>;
}

impl CacheOnlyReads for aida_core::CachedGitBackend {
    fn list_summaries(
        &self,
        filter: &aida_core::ListFilter,
    ) -> anyhow::Result<Vec<aida_core::RequirementSummary>> {
        aida_core::CachedGitBackend::list_summaries(self, filter)
    }
}

/// Whether `id` is shaped like a spec id OR is a UUID. A UUID is the only
/// handle that reaches the native owner of an ambiguous id, so the lookup
/// paths accept it.
// trace:BUG-1535 | ai:claude
pub(crate) fn is_spec_id_or_uuid(id: &str) -> bool {
    aida_core::object_store::valid_spec_id_format(id) || Uuid::parse_str(id.trim()).is_ok()
}

/// `aida list` must not emit two rows under one id. The display id prefers
/// `agreed_id`, so a spec whose agreed id collides with another spec's native
/// id rendered as a duplicate. Such rows drop the ambiguous agreed id for this
/// view and render under their native spec_id; a stderr note says which.
/// Best-effort: if the scan fails the rows are left untouched.
// trace:BUG-1535 | ai:claude
pub(crate) fn relabel_ambiguous_list_rows(
    backend: &aida_core::CachedGitBackend,
    rows: &mut [aida_core::RequirementSummary],
) {
    let Ok(collisions) = backend.id_collisions() else {
        return;
    };
    if collisions.is_empty() {
        return;
    }
    let relabel = aida_core::id_collisions::relabel_to_native(&collisions);
    let mut moved: Vec<String> = Vec::new();
    for row in rows.iter_mut() {
        if !relabel.contains(&row.id) {
            continue;
        }
        if let Some(agreed) = row.agreed_id.take() {
            moved.push(format!(
                "{} (agreed id {} is ambiguous)",
                row.spec_id.as_deref().unwrap_or("?"),
                agreed.to_ascii_uppercase()
            ));
        }
    }
    if !moved.is_empty() {
        eprintln!(
            "{} {} row(s) shown by native id because their agreed id also names another spec: {}. \
             `aida doctor --category id-collisions` lists every ambiguous id.",
            "Note:".yellow().bold(),
            moved.len(),
            moved.join(", ")
        );
    }
}

pub(crate) fn show_cached_context(
    backend: &impl CacheOnlyReads,
    req: &Requirement,
) -> (Option<String>, Option<String>) {
    let mut effective_status = None;
    let mut serialize_command = None;
    let mut batches: Vec<&str> = req
        .tags
        .iter()
        .filter_map(|tag| tag.strip_prefix("batch:"))
        .collect();
    batches.sort_unstable();

    let all = backend.list_summaries(&aida_core::ListFilter {
        archive: aida_core::ArchiveFilter::Both,
        defer: aida_core::DeferFilter::Both,
        ..Default::default()
    });
    let Ok(summaries) = all else {
        return (effective_status, serialize_command);
    };

    if req.req_type == RequirementType::Epic {
        effective_status = summaries
            .iter()
            .find(|summary| summary.id == req.id)
            .map(|summary| summary.status.clone());
    }

    for batch in batches {
        let tag = format!("batch:{batch}");
        let has_durable_serialize_verdict = summaries.iter().any(|summary| {
            summary.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag))
                && summary
                    .tags
                    .iter()
                    .any(|t| t.to_ascii_lowercase().starts_with("serialize:"))
        });
        if has_durable_serialize_verdict {
            serialize_command = Some(format!(
                "aida queue work --batch {batch} --auto-complete --single-branch"
            ));
            break;
        }
    }

    (effective_status, serialize_command)
}

/// The TOON `status` scalar value for the per-spec agent-mode `show`: the
/// normalized status token, annotated with the derived-status disclosure when
/// the displayed (effective) value and the stored field resolve to different
/// lifecycle states. Split out for direct unit testing, same rationale as
/// BUG-1423's `emit_ship_pr_merged`.
// trace:BUG-1767 | ai:claude
pub(crate) fn toon_show_status(effective_status: &str, stored_status: &str) -> String {
    let token = toon_status_token(effective_status);
    match status_display::derived_status_annotation(effective_status, stored_status) {
        Some(annotation) => format!("{token} ({annotation})"),
        None => token,
    }
}

// trace:TASK-1569 | ai:codex
pub(crate) fn toon_show_comment_lines(
    comments: &[aida_core::Comment],
    include_bodies: bool,
) -> Vec<String> {
    if comments.is_empty() {
        return Vec::new();
    }
    if !include_bodies {
        return vec![crate::toon::scalar(
            "comments",
            &format!("{} (--comments to print)", comments.len()),
        )];
    }

    let mut lines = vec![crate::toon::scalar("comments", &comments.len().to_string())];
    for (index, comment) in comments.iter().enumerate() {
        let value = format!(
            "author: {}\ncreated_at: {}\nbody: {}",
            comment.author,
            comment.created_at.to_rfc3339(),
            comment.content
        );
        lines.push(crate::toon::scalar(&format!("comments[{index}]"), &value));
    }
    lines
}

// trace:BUG-1207 | ai:codex
pub(crate) fn render_list_table<F>(
    reqs: &[aida_core::RequirementSummary],
    options: &ListTableOptions,
    row_routing: F,
) -> Vec<String>
where
    F: Fn(&aida_core::RequirementSummary) -> (bool, bool, bool),
{
    let flow_width = options.flow_header.chars().count();
    // trace:STORY-1653 | ai:claude — the status column widens by the driver
    // suffix (` · relay:lib-split`) only when some row resolved a driver.
    let drv_w = options
        .drivers
        .values()
        .map(|l| l.chars().count() + 3)
        .max()
        .unwrap_or(0);
    let status_w = 13 + drv_w;
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — the ID
    // column fits the widest RENDERED id (labels and `[ids]` widths
    // applied), never narrower than the historical column.
    let display_ids: Vec<String> = reqs
        .iter()
        .map(|req| {
            aida_core::node_alias::display_spec_id(
                req.agreed_id
                    .as_deref()
                    .or(req.spec_id.as_deref())
                    .unwrap_or("?"),
            )
        })
        .collect();
    let widest_id = display_ids
        .iter()
        .map(|d| d.chars().count())
        .max()
        .unwrap_or(0);
    let id_w = widest_id.max(if options.show_origin { 12 } else { 14 });
    let id_extra = id_w - if options.show_origin { 12 } else { 14 };
    let header_prefix = if options.show_origin {
        format!(
            "{}{:<id_w$} {:<14} {:<12} {:<status_w$} ",
            options.flow_header, "ID", "Origin ID", "Type", "Status"
        )
    } else {
        format!(
            "{}{:<id_w$} {:<12} {:<status_w$} {:<10} ",
            options.flow_header, "ID", "Type", "Status", "Priority"
        )
    };
    let prefix_width = header_prefix.chars().count();
    let desired_tag_width = if options.show_tags {
        reqs.iter()
            .map(|r| format_tags_inline(&r.tags, 3).chars().count())
            .chain(std::iter::once("Tags".len()))
            .max()
            .unwrap_or(4)
    } else {
        0
    };
    let (title_width, tag_width) = match options.terminal_width {
        // Preserve the pre-BUG-1207 piped-output contract. Only an attached
        // terminal opts into width-aware geometry.
        None => (if options.show_tags { 50 } else { usize::MAX }, usize::MAX),
        Some(total) if options.show_tags => {
            let available = total.saturating_sub(prefix_width);
            let tag_width = desired_tag_width.min(available.saturating_sub(25)).max(4);
            let tag_width = tag_width.min(available.saturating_sub(1));
            (available.saturating_sub(tag_width + 1), tag_width)
        }
        Some(total) => (total.saturating_sub(prefix_width), 0),
    };
    let title_column_width = if title_width == usize::MAX {
        24
    } else {
        title_width
    };
    let table_width = options.terminal_width.unwrap_or_else(|| {
        flow_width
            + match (options.show_origin, options.show_tags) {
                (true, true) => 113,
                (true, false) => 81,
                (false, true) => 111,
                (false, false) => 77,
            }
            + drv_w
            + id_extra
    });

    let render_status = |r: &aida_core::RequirementSummary| -> String {
        let held = r
            .tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case("operator-held"));
        let label = if held {
            "Held".to_string()
        } else {
            status_display::display_status_for_type(&r.req_type, &r.status).to_string()
        };
        // trace:STORY-1653 | ai:claude — `◐ InProgress · relay:lib-split`:
        // the label is padded AFTER the driver suffix so the two read as one
        // phrase, and the column stays `13 + drv_w` wide.
        let suffix = options
            .drivers
            .get(&r.id)
            .map(|l| format!(" · {l}"))
            .unwrap_or_default();
        let label_w = if suffix.is_empty() {
            13
        } else {
            label.chars().count() + if options.no_glyph { 0 } else { 2 }
        };
        let cell = if options.no_glyph {
            status_display::status_cell_no_glyph(&label, label_w)
        } else {
            status_display::status_cell(&label, label_w.saturating_sub(2))
        };
        if drv_w == 0 {
            return cell;
        }
        let pad = (13 + drv_w).saturating_sub(label_w + suffix.chars().count());
        format!("{cell}{}{}", suffix.dimmed(), " ".repeat(pad))
    };
    let flow_prefix = |r: &aida_core::RequirementSummary| -> String {
        if flow_width == 0 {
            return String::new();
        }
        let (in_flight, blocked, queued) = row_routing(r);
        let supervised = r.execution_mode.as_deref().is_some_and(|m| {
            matches!(
                m.to_ascii_lowercase().as_str(),
                "guided" | "operator" | "decide"
            )
        });
        let glyph = status_display::flow_glyph(in_flight, blocked, queued, supervised);
        format!("{:<width$}", format!("{glyph} "), width = flow_width)
    };
    let title_cell = |r: &aida_core::RequirementSummary| -> String {
        if options.show_tags {
            return truncate(&r.title, title_width);
        }
        match r.assignee.as_deref() {
            Some(a) if !a.is_empty() => {
                let suffix = format!("@{a}");
                let available = title_width.saturating_sub(suffix.chars().count() + 1);
                format!("{} {}", truncate(&r.title, available), suffix.cyan())
            }
            _ => truncate(&r.title, title_width),
        }
    };

    let mut lines = Vec::with_capacity(reqs.len() + 2);
    if options.show_tags {
        if options.terminal_width.is_none() {
            lines.push(format!("{}{:<50} Tags", header_prefix, "Title"));
        } else {
            lines.push(format!(
                "{}{:<width$} {:<tag_width$}",
                header_prefix,
                "Title",
                truncate("Tags", tag_width),
                width = title_column_width,
            ));
        }
    } else {
        lines.push(format!("{}Title", header_prefix));
    }
    lines.push("─".repeat(table_width));

    for (req, display_id) in reqs.iter().zip(&display_ids) {
        // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
        let display_id = display_id.as_str();
        let status = render_status(req);
        let prefix = if options.show_origin {
            let origin = req.spec_id.as_deref().unwrap_or("-");
            let origin_padded = format!("{:<14}", origin);
            let origin_cell = if origin == display_id {
                origin_padded.dimmed().to_string()
            } else {
                origin_padded
            };
            format!(
                "{}{:<id_w$} {} {:<12} {} ",
                flow_prefix(req),
                display_id,
                origin_cell,
                req.req_type,
                status
            )
        } else {
            format!(
                "{}{:<id_w$} {:<12} {} {:<10} ",
                flow_prefix(req),
                display_id,
                req.req_type,
                status,
                req.priority
            )
        };
        if options.show_tags {
            let tags = if options.terminal_width.is_none() {
                format_tags_inline(&req.tags, 3)
            } else {
                truncate(&format_tags_inline(&req.tags, 3), tag_width)
            };
            lines.push(format!(
                "{}{:<width$} {}",
                prefix,
                title_cell(req),
                tags.dimmed(),
                width = title_column_width,
            ));
        } else {
            lines.push(format!("{}{}", prefix, title_cell(req)));
        }
    }
    lines
}

#[cfg(test)]
mod list_title_width_tests {
    use super::*;

    fn summary() -> aida_core::RequirementSummary {
        aida_core::RequirementSummary {
            id: Uuid::nil(),
            spec_id: Some("BUG-1207".into()),
            agreed_id: Some("BUG-1207".into()),
            title: "A deliberately very long title that must be clipped before it can wrap the human table".into(),
            description: String::new(),
            status: "in-progress".into(),
            priority: "medium".into(),
            owner: String::new(),
            assignee: None,
            feature: String::new(),
            req_type: "bug".into(),
            tags: vec![
                "aida:list".into(),
                "severity:cosmetic".into(),
                "a-very-long-third-tag".into(),
                "extra".into(),
            ],
            created_at: String::new(),
            modified_at: String::new(),
            archived: false,
            archived_at: None,
            deferred: false,
            deferred_at: None,
            deferred_until: None,
            deferred_reason: None,
            in_degree: 0,
            out_degree: 0,
            heft: 0,
            blocked: false,
            has_pending_decision: false,
            execution_mode: None,
            weight: None,
            origin: None,
            completed_at: None, // trace:TASK-1474 | ai:claude
            yaml_path: String::new(),
        }
    }

    fn visible_width(line: &str) -> usize {
        strip_ansi_color(line).chars().count()
    }

    #[test]
    fn rendered_tables_fit_default_origin_and_tag_variants() {
        let reqs = vec![summary()];
        for width in [80, 120] {
            for show_origin in [false, true] {
                for show_tags in [false, true] {
                    let options = ListTableOptions {
                        show_origin,
                        show_tags,
                        no_glyph: true,
                        flow_header: String::new(),
                        terminal_width: Some(width),
                        drivers: Default::default(),
                    };
                    let lines = render_list_table(&reqs, &options, |_| (false, false, false));
                    assert_eq!(
                        visible_width(&lines[1]),
                        width,
                        "divider: origin={show_origin} tags={show_tags}"
                    );
                    for line in lines {
                        assert!(
                            visible_width(&line) <= width,
                            "{width}-column table overflowed: origin={show_origin} tags={show_tags}: {line:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn non_tty_show_tags_preserves_legacy_fixed_geometry() {
        let req = summary();
        let options = ListTableOptions {
            show_origin: false,
            show_tags: true,
            no_glyph: true,
            flow_header: String::new(),
            terminal_width: None,
            drivers: Default::default(),
        };
        let lines = render_list_table(std::slice::from_ref(&req), &options, |_| {
            (false, false, false)
        });
        assert_eq!(visible_width(&lines[0]), 108);
        assert_eq!(visible_width(&lines[1]), 111);
        let row = strip_ansi_color(&lines[2]);
        assert!(row.contains(&truncate(&req.title, 50)));
        assert!(!row.contains(&req.title));
        assert!(row.contains(&format_tags_inline(&req.tags, 3)));
    }

    /// AC5e: at `[ids] seq_width` 0 and 4 the ID column fits the widest
    /// RENDERED id, so with mixed-length IDs (a short legacy id, a 4-digit
    /// one, a node-local id longer than the historical 14-char column) every
    /// row's Type, Status and Title cells start where the header's do.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046.acbed48e | ai:claude
    #[test]
    fn list_columns_align_with_mixed_length_ids_at_seq_width_0_and_4() {
        let node_local = "TASK-0123456789abcdef0123456789abcdef-001";
        let row = |id: &str, ty: &str, title: &str| aida_core::RequirementSummary {
            spec_id: Some(id.into()),
            agreed_id: None,
            req_type: ty.into(),
            title: title.into(),
            tags: Vec::new(),
            ..summary()
        };
        let reqs = vec![
            row("BUG-07", "bug", "short id"),
            row("STORY-1207", "story", "four digits"),
            row(node_local, "task", "node local"),
        ];
        let options = ListTableOptions {
            show_origin: false,
            show_tags: false,
            no_glyph: true,
            flow_header: String::new(),
            terminal_width: None,
            drivers: Default::default(),
        };
        for (width, short) in [(0, "BUG-7 "), (4, "BUG-0007 ")] {
            let prev = aida_core::id_identity::install_widths_for_thread(Some(
                aida_core::id_identity::IdWidths {
                    seq_width: Some(width),
                    ..Default::default()
                },
            ));
            let lines: Vec<String> = render_list_table(&reqs, &options, |_| (false, false, false))
                .iter()
                .map(|l| strip_ansi_color(l))
                .collect();
            aida_core::id_identity::install_widths_for_thread(prev);
            let col = |line: &str, needle: &str| {
                line.find(needle)
                    .map(|b| line[..b].chars().count())
                    .unwrap_or_else(|| panic!("{needle:?} missing from {line:?}"))
            };
            let header = &lines[0];
            let (type_at, title_at) = (col(header, "Type"), col(header, "Title"));
            assert!(
                type_at > node_local.len(),
                "ID column fits the long id: {header:?}"
            );
            assert!(lines[2].starts_with(short), "width {width}: {:?}", lines[2]);
            for (line, req) in lines[2..].iter().zip(&reqs) {
                assert_eq!(col(line, &req.req_type), type_at, "width {width}: {line:?}");
                assert_eq!(col(line, &req.title), title_at, "width {width}: {line:?}");
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ProxyApprovalEntry {
    pub(crate) spec_id: String,
    pub(crate) title: String,
    pub(crate) comment_id: String,
    pub(crate) comment_author: String,
    pub(crate) created_at: String,
    pub(crate) decision: String,
    pub(crate) verdict: String,
    pub(crate) proxy: String,
    pub(crate) session: String,
    pub(crate) authorized_by: String,
    pub(crate) independent: String,
    pub(crate) timestamp_local: String,
}

pub(crate) fn add_proxy_approval(
    backend: &aida_core::CachedGitBackend,
    spec: &str,
    decision: &str,
    verdict: &str,
    proxy: Option<&str>,
    session: Option<&str>,
    authorized_by: &str,
    independent: crate::cli::ProxyApprovalIndependent,
) -> Result<()> {
    // trace:STORY-1173 | ai:codex
    let mut req = backend
        .get_requirement_unambiguous(spec)?
        .ok_or_else(|| not_found::requirement_not_found(spec, None))?;
    let decision = required_approval_field("decision", decision)?;
    let verdict = required_approval_field("verdict", verdict)?;
    let authorized_by = required_approval_field("authorized-by", authorized_by)?;
    let proxy = proxy
        .map(|p| required_approval_field("proxy", p))
        .transpose()?
        .unwrap_or_else(get_default_author);
    let session = session
        .map(|s| required_approval_field("session", s))
        .transpose()?
        .or_else(resolve_current_session_id)
        .unwrap_or_else(|| "unknown".to_string());
    let independent = match independent {
        crate::cli::ProxyApprovalIndependent::Yes => "yes",
        crate::cli::ProxyApprovalIndependent::No => "no",
    };
    let now = chrono::Utc::now();
    let timestamp_local = now
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M:%S %Z")
        .to_string();
    let body = format!(
        "{PROXY_APPROVAL_MARKER}\n\
         decision: {decision}\n\
         verdict: {verdict}\n\
         proxy: {proxy}\n\
         session: {session}\n\
         authorized-by: {authorized_by}\n\
         independent: {independent}\n\
         timestamp-local: {timestamp_local}"
    );
    let comment = aida_core::Comment {
        id: Uuid::now_v7(),
        content: body,
        author: proxy.clone(),
        created_at: now,
        modified_at: now,
        parent_id: None,
        replies: Vec::new(),
        reactions: Vec::new(),
        session_id: Some(session.clone()),
        harness_version: None,
        relayed_from: None,
    };
    req.comments.push(comment);
    req.modified_at = now;
    backend.update_requirement(&req)?;
    println!(
        "Proxy approval recorded on {}",
        req.spec_id.as_deref().unwrap_or(spec)
    );
    Ok(())
}

pub(crate) fn list_proxy_approvals(
    backend: &aida_core::CachedGitBackend,
    since: Option<&str>,
    until: Option<&str>,
    json: bool,
) -> Result<()> {
    // trace:STORY-1173 | ai:codex
    // trace:TASK-1509 | ai:claude — label errors with the flag that failed.
    let now = chrono::Utc::now();
    let bound = |raw: &str, flag: &str| {
        crate::queue_cmd::parse_time_bound_at(raw, flag, now, &chrono::Local)
    };
    let since = since.map(|raw| bound(raw, "--since")).transpose()?;
    let until = until.map(|raw| bound(raw, "--until")).transpose()?;
    let store = backend.load()?;
    let mut entries = Vec::new();
    for req in &store.requirements {
        let spec_id = req.display_id();
        collect_proxy_approval_entries(
            &mut entries,
            &spec_id,
            &req.title,
            &req.comments,
            since,
            until,
        );
    }
    entries.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then(a.spec_id.cmp(&b.spec_id))
    });
    if json {
        println!("{}", crate::cache_output::json_pretty(&entries)?);
        return Ok(());
    }
    if entries.is_empty() {
        println!("No proxy approvals found");
        return Ok(());
    }
    print_proxy_approval_entries(&entries, true);
    Ok(())
}

pub(crate) fn print_proxy_approval_entries(entries: &[ProxyApprovalEntry], include_spec: bool) {
    // trace:STORY-1173 | ai:codex
    println!("{}", "Proxy approvals".green().bold());
    for entry in entries {
        if include_spec {
            println!(
                "{} {} {}",
                entry.created_at.dimmed(),
                entry.spec_id.cyan().bold(),
                entry.decision.bold()
            );
        } else {
            println!("{} {}", entry.created_at.dimmed(), entry.decision.bold());
        }
        println!("  verdict: {}", entry.verdict);
        println!(
            "  proxy: {}  session: {}  authorized-by: {}  independent: {}",
            entry.proxy, entry.session, entry.authorized_by, entry.independent
        );
        println!(
            "  comment: {}  timestamp-local: {}",
            entry.comment_id, entry.timestamp_local
        );
    }
}

pub(crate) fn collect_proxy_approval_entries(
    out: &mut Vec<ProxyApprovalEntry>,
    spec_id: &str,
    title: &str,
    comments: &[aida_core::Comment],
    since: Option<chrono::DateTime<chrono::Utc>>,
    until: Option<chrono::DateTime<chrono::Utc>>,
) {
    for comment in comments {
        let in_window = since.map(|s| comment.created_at >= s).unwrap_or(true)
            && until.map(|u| comment.created_at < u).unwrap_or(true);
        if in_window {
            if let Some(entry) = parse_proxy_approval_comment(spec_id, title, comment) {
                out.push(entry);
            }
        }
        collect_proxy_approval_entries(out, spec_id, title, &comment.replies, since, until);
    }
}

fn parse_proxy_approval_comment(
    spec_id: &str,
    title: &str,
    comment: &aida_core::Comment,
) -> Option<ProxyApprovalEntry> {
    if !comment
        .content
        .lines()
        .any(|line| line.trim() == PROXY_APPROVAL_MARKER)
    {
        return None;
    }
    let field = |name: &str| approval_field(&comment.content, name);
    let independent = field("independent")?;
    if !matches!(independent.as_str(), "yes" | "no") {
        return None;
    }
    Some(ProxyApprovalEntry {
        spec_id: spec_id.to_string(),
        title: title.to_string(),
        comment_id: comment.id.to_string(),
        comment_author: comment.author.clone(),
        created_at: comment.created_at.to_rfc3339(),
        decision: field("decision")?,
        verdict: field("verdict").or_else(|| field("reasoning"))?,
        proxy: field("proxy")?,
        session: field("session")?,
        authorized_by: field("authorized-by")?,
        independent,
        timestamp_local: field("timestamp-local")?,
    })
}

fn approval_field(content: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}:");
    content.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix(&prefix)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(ToString::to_string)
    })
}

fn required_approval_field(name: &str, value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{name} is required for proxy approval entries");
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod proxy_approval_tests {
    use super::*;

    fn approval_comment(content: &str) -> aida_core::Comment {
        aida_core::Comment::new("ai:codex".to_string(), content.to_string())
    }

    #[test]
    fn parses_marked_proxy_approval_comment() {
        let comment = approval_comment(
            "[aida:proxy-approval]\n\
             decision: merge\n\
             verdict: approved after independent review\n\
             proxy: ai:codex\n\
             session: sess-123\n\
             authorized-by: joe\n\
             independent: yes\n\
             timestamp-local: 2026-09-16 10:10:00 MST",
        );

        let entry = parse_proxy_approval_comment("STORY-1173", "Proxy ledger", &comment)
            .expect("marked approval should parse");

        assert_eq!(entry.spec_id, "STORY-1173");
        assert_eq!(entry.decision, "merge");
        assert_eq!(entry.independent, "yes");
        assert_eq!(entry.authorized_by, "joe");
    }

    #[test]
    fn rejects_marked_proxy_approval_without_independent_field() {
        let comment = approval_comment(
            "[aida:proxy-approval]\n\
             decision: merge\n\
             verdict: approved\n\
             proxy: ai:codex\n\
             session: sess-123\n\
             authorized-by: joe\n\
             timestamp-local: 2026-09-16 10:10:00 MST",
        );

        assert!(parse_proxy_approval_comment("STORY-1173", "Proxy ledger", &comment).is_none());
    }
}

#[cfg(test)]
mod show_latency_regression_tests {
    use super::*;

    /// THE GUARANTEE IS THE TYPE. `show_cached_context` takes
    /// `&impl CacheOnlyReads`, which exposes `list_summaries` and nothing
    /// else, so a whole-store read on that path is a COMPILE ERROR:
    ///
    ///     error[E0599]: no method named `load` found for reference
    ///                   `&impl CacheOnlyReads` in the current scope
    ///
    /// verified by injecting `backend.load()` into `show_cached_context` and
    /// observing the build fail, then restoring to a clean build.
    ///
    /// THIS TEST DELIBERATELY ASSERTS ALMOST NOTHING. An earlier revision
    /// parsed this file to count the trait's methods, so that widening the
    /// trait would be caught. Mutation proved that check INERT: it extracted
    /// the body with `split_once('}')`, which stops at the first brace, and a
    /// default method body supplies one INSIDE the trait — so adding a bodied
    /// method truncated the extraction and the count still read 1. It could
    /// not fire on the exact change it existed to catch.
    ///
    /// Rather than repair a source-text parser to guard a property the
    /// compiler already enforces, it is gone.
    ///
    /// WHAT THIS ONE ACTUALLY ESTABLISHES (BUG-1559). It used to be described
    /// here as "a witness that the restricted view is what the path is typed
    /// against" — that is stronger than what it does, and BUG-1559 corrects
    /// it: this only asserts that `aida_core::CachedGitBackend` implements
    /// `CacheOnlyReads`. It says nothing about `show_cached_context`'s own
    /// parameter type. Demonstrated: narrow that parameter from
    /// `&impl CacheOnlyReads` to the concrete `&aida_core::CachedGitBackend`
    /// and this test still compiles and still passes, because
    /// `CachedGitBackend` implements the trait either way — the guarantee
    /// that matters is gone and this assertion does not notice. Kept because
    /// it is still a true, narrower fact worth having (this production type
    /// does implement the trait); the actual path-typing guarantee is
    /// `show_cached_context_stays_generic_over_cache_only_reads` below.
    // trace:BUG-1480 | ai:claude
    #[test]
    fn cached_git_backend_implements_cache_only_reads() {
        fn requires_cache_only_view<T: CacheOnlyReads>() {}
        requires_cache_only_view::<aida_core::CachedGitBackend>();
    }

    /// A second, independent implementor of `CacheOnlyReads` — deliberately
    /// not `aida_core::CachedGitBackend` and with no relationship to it. Its
    /// only job is to be a distinct type that satisfies the trait, so a call
    /// through it can only type-check if the callee's parameter is still
    /// generic over the trait rather than pinned to one concrete type.
    struct AnotherCacheOnlyView;

    impl CacheOnlyReads for AnotherCacheOnlyView {
        fn list_summaries(
            &self,
            _filter: &aida_core::ListFilter,
        ) -> anyhow::Result<Vec<aida_core::RequirementSummary>> {
            Ok(Vec::new())
        }
    }

    /// THE WITNESS BUG-1559 ASKED FOR: this asserts `show_cached_context`'s
    /// own parameter, not merely that some type implements the trait it
    /// names. `AnotherCacheOnlyView` has no relationship to
    /// `aida_core::CachedGitBackend`, so passing it here only type-checks
    /// while the parameter is generic (`&impl CacheOnlyReads`). If a later
    /// change narrows the parameter to the concrete
    /// `&aida_core::CachedGitBackend`, this call becomes a type mismatch and
    /// the crate fails to build — see BUG-1559 for the mutation demonstrated
    /// both ways (the sibling test above shown to stay green under it, this
    /// one shown to fail).
    ///
    /// A `trybuild` compile-fail fixture was considered for this and judged
    /// disproportionate: it would add a dev-dependency plus an out-of-crate
    /// `.rs`/`.stderr` fixture pair to assert exactly the fact this in-crate
    /// type mismatch already asserts for free, with a clearer error at the
    /// call site than a separately-maintained stderr snapshot would give.
    // trace:BUG-1559 | ai:claude
    #[test]
    fn show_cached_context_stays_generic_over_cache_only_reads() {
        let req = Requirement::new("BUG-1559 witness".to_string(), String::new());
        let _ = show_cached_context(&AnotherCacheOnlyView, &req);
    }

    /// BUG-1767 (acceptance 1): the `show` status value for a rollup-complete
    /// epic whose STORED status is not Completed must carry BOTH values — the
    /// derived one and the stored one — so no caller can mistake a rolled-up
    /// Completed for a stored one. Dropping the stored value from the line
    /// turns this red.
    // trace:BUG-1767 | ai:claude
    #[test]
    fn toon_show_status_discloses_derived_and_stored() {
        // trace:BUG-1767.acc811ae | ai:claude
        // The spec's fixture shape: stored Draft, rollup-derived Completed.
        let line = toon_show_status("Completed", "Draft");
        assert!(line.contains("completed"), "derived value legible: {line}");
        assert!(
            line.contains("stored: draft"),
            "stored value legible: {line}"
        );

        // The other stored shape observed (EPIC-71): stored InProgress.
        let line = toon_show_status("Completed", "InProgress");
        assert!(line.contains("stored: in-progress"), "{line}");
    }

    /// BUG-1767 (acceptance 3): the unchanged cases — a non-epic, and an epic
    /// whose rollup matches its stored field — render EXACTLY the stored
    /// status token with no derivation annotation, including across spelling
    /// variants of the same state and for custom statuses outside the
    /// lifecycle.
    // trace:BUG-1767 | ai:claude
    #[test]
    fn toon_show_status_unannotated_when_stored_agrees() {
        // trace:BUG-1767.ac336a39 | ai:claude
        // TASK-1565 / STORY-1486 shape: a non-epic's effective status IS the
        // stored one.
        assert_eq!(toon_show_status("Draft", "Draft"), "draft");
        // EPIC-74 shape: an epic that is not rollup-complete derives its
        // stored value back.
        assert_eq!(toon_show_status("Completed", "Completed"), "completed");
        assert_eq!(
            toon_show_status("In Progress", "In Progress"),
            "in-progress"
        );
        // Spelling variants of the SAME state are agreement, not divergence.
        assert_eq!(toon_show_status("InProgress", "In Progress"), "in-progress");
        // A custom status outside the lifecycle never grows an annotation.
        assert_eq!(toon_show_status("frobnicated", "Draft"), "frobnicated");
    }

    // trace:TASK-1569 | ai:codex
    #[test]
    fn toon_show_comment_lines_projects_count_and_full_bodies() {
        use chrono::{TimeZone, Utc};
        let empty = Vec::new();
        assert!(toon_show_comment_lines(&empty, false).is_empty());

        let comments = vec![aida_core::Comment {
            id: uuid::Uuid::nil(),
            author: "reviewer".into(),
            content: "distinctive body phrase survives intact".into(),
            created_at: Utc.timestamp_opt(1_759_500_000, 0).unwrap(),
            modified_at: Utc.timestamp_opt(1_759_500_000, 0).unwrap(),
            parent_id: None,
            replies: Vec::new(),
            reactions: Vec::new(),
            session_id: None,
            harness_version: None,
            relayed_from: None,
        }];
        let hint = toon_show_comment_lines(&comments, false).join("\n");
        assert!(hint.contains("comments: 1 (--comments to print)"), "{hint}");

        let body = toon_show_comment_lines(&comments, true).join("\n");
        assert!(body.contains("comments: 1"), "{body}");
        assert!(body.contains("reviewer"), "{body}");
        assert!(
            body.contains("distinctive body phrase survives intact"),
            "{body}"
        );
        assert!(!body.contains("--comments to print"), "{body}");
    }
}

/// `aida edit --status` — emit the seat-tagged `DispositionChanged` event
/// this invocation's status transition produced (approve/reject/defer-class
/// decisions, including groom's `--apply`, which shells out to `aida edit`).
/// Split out for direct unit testing, same rationale as BUG-1423's
/// `emit_ship_pr_merged`. Best-effort: `events::emit` never fails.
// trace:TASK-1450 | ai:claude
pub(crate) fn emit_edit_disposition_changed(
    project_root: &std::path::Path,
    spec_display: &str,
    before: String,
    after: String,
) {
    let mut ev = crate::events::Event::new(
        Some(spec_display.to_string()),
        "",
        crate::events::EventKind::DispositionChanged { before, after },
    );
    ev.seat = crate::events::active_seat();
    crate::events::emit(project_root, &ev);
}

/// `aida edit --mode` — emit the seat-tagged `ExecutionModeChanged` event
/// this invocation's mode write produced. Split out for direct unit testing.
/// Best-effort: `events::emit` never fails.
// trace:TASK-1450 | ai:claude
pub(crate) fn emit_edit_execution_mode_changed(
    project_root: &std::path::Path,
    spec_display: &str,
    before: Option<String>,
    after: Option<String>,
) {
    let mut ev = crate::events::Event::new(
        Some(spec_display.to_string()),
        "",
        crate::events::EventKind::ExecutionModeChanged { before, after },
    );
    ev.seat = crate::events::active_seat();
    crate::events::emit(project_root, &ev);
}

#[cfg(test)]
mod task_1450_edit_event_tests {
    use super::*;

    /// A disposition (status) change through `aida edit --status` must
    /// record the before/after status labels and the acting seat — the
    /// approve/reject/defer-class decision BUG-1423's follow-up (TASK-1450)
    /// closes.
    // trace:TASK-1450 | ai:claude
    #[test]
    fn disposition_change_emits_seat_tagged_before_after() {
        let tmp = tempfile::tempdir().unwrap();
        let _seat = crate::test_env::AmbientGuard::hermetic_with_seat(tmp.path(), "advisor", &[]);

        emit_edit_disposition_changed(tmp.path(), "TASK-1", "Draft".into(), "Approved".into());

        let events = crate::events::read_all(tmp.path());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].spec.as_deref(), Some("TASK-1"));
        assert_eq!(events[0].seat.as_deref(), Some("advisor"));
        assert!(matches!(
            &events[0].kind,
            crate::events::EventKind::DispositionChanged { before, after }
                if before == "Draft" && after == "Approved"
        ));
    }

    /// An `execution_mode` change through `aida edit --mode` must record the
    /// before/after mode (including the ungroomed `None` case) and the
    /// acting seat.
    // trace:TASK-1450 | ai:claude
    #[test]
    fn execution_mode_change_emits_seat_tagged_before_after() {
        let tmp = tempfile::tempdir().unwrap();
        let _seat = crate::test_env::AmbientGuard::hermetic_with_seat(tmp.path(), "advisor", &[]);

        emit_edit_execution_mode_changed(tmp.path(), "TASK-2", None, Some("drain".into()));

        let events = crate::events::read_all(tmp.path());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].spec.as_deref(), Some("TASK-2"));
        assert_eq!(events[0].seat.as_deref(), Some("advisor"));
        assert!(matches!(
            &events[0].kind,
            crate::events::EventKind::ExecutionModeChanged { before, after }
                if before.is_none() && after.as_deref() == Some("drain")
        ));
    }
}

/// What `aida db sync`'s locked local window produced.
// trace:TASK-1717 | ai:claude
pub(crate) struct DbSyncWindow {
    pub(crate) branch: String,
    /// The store commit to push: HEAD as the window closed.
    pub(crate) head: Option<String>,
    /// Conflicts with what the pull brought in; `None` when nothing was pulled.
    pub(crate) conflicts: Option<Vec<aida_core::conflict::RequirementConflict>>,
}

/// `aida db sync`'s local transaction: commit pending changes, then pull
/// --rebase, then scan the result. The caller holds the store write lock for
/// the whole call, so reads here are canonical and strict (the object files,
/// never the cache; a bad object is an error) and a failed rebase is cleaned
/// up by `pull_rebase` before the lock is released.
// trace:TASK-1717 | ai:claude
pub(crate) fn db_sync_local_window(
    store_path: &std::path::Path,
    pull: bool,
    push: bool,
    message: Option<&str>,
) -> Result<DbSyncWindow> {
    let branch =
        aida_core::git_ops::current_branch(store_path).unwrap_or_else(|_| "main".to_string());

    // ── Order matters: commit local first, THEN pull --rebase ──
    // Rebase requires a clean working tree, so we have to commit
    // any pending edits (typically from `aida edit` paths that
    // didn't auto-commit, or manual file edits) before pulling.
    // Old order was pull → commit, which failed when there were
    // unstaged changes — git rebase refuses, and the follow-up
    // commit ran on a partial-rebase state with a confusing
    // empty error. trace:BUG-1-051 | ai:claude

    // Step 1: stage and commit any pending changes.
    //
    // Stage everything in the worktree (`git add -A .`) instead
    // of cherry-picking specific subdirs like `objects/`. The
    // orphan branch's own .gitignore already excludes runtime
    // artifacts (cache.db, lock files); whatever's left modified
    // is canonical state — including `.aida/dispenser.toml`,
    // which gets dirtied by ID dispensing and would otherwise be
    // skipped by an objects-only stage. Without this, has_changes
    // could report "yes" while nothing gets staged, and the
    // follow-up `git commit` would fail with an empty error.
    // trace:BUG-1-051 | ai:claude
    // trace:TASK-1717 | ai:claude — a failing `git status` is an error,
    // never "clean".
    // trace:BUG-2140 | ai:antigravity — refuse embedded git repos even when
    // `git status` would not report an empty nested `.git` as a dirty tree.
    aida_core::git_ops::refuse_embedded_repos(store_path)?;
    aida_core::git_ops::protect_roster_advisors(store_path)?; // trace:BUG-2209 | ai:claude
    let has_changes = aida_core::git_ops::has_changes_checked(store_path)?;
    if has_changes {
        let msg = message.unwrap_or("chore: sync pending changes");
        aida_core::git_ops::add_all(store_path, ".")?;
        if aida_core::git_ops::commit(store_path, msg)? {
            println!("Committed: {}", msg);
        }
    } else if !pull && !push {
        println!("Nothing to commit.");
    }

    // Step 2: pull --rebase. Bare `git pull` fails on divergent
    // branches when the user has no `pull.rebase` / `pull.ff`
    // config — the orphan-store model wants linear history
    // anyway (replay local commits on top of remote), so
    // rebase is the right default.
    let mut conflicts = None;
    if pull && !aida_core::git_ops::has_remote(store_path, "origin") {
        // BUG-432: local-only / fresh-init projects have no `origin`
        // yet (the aida-demo + pre-remote flow). The pull is a graceful
        // skip, not a fatal — otherwise `aida queue work`'s startup sync
        // (and a standalone `aida db sync --pull`) die before a remote
        // is ever added. Mirrors `aida fetch --code-only`'s tolerance.
        // trace:BUG-432 | ai:claude
        println!("  No `origin` remote — skipping pull (local-only project).");
    } else if pull {
        // Snapshot local state before pull for conflict detection.
        // trace:TASK-1717 | ai:claude — strict: an unreadable or unparsable
        // object is an error, not silently left out of the comparison.
        let local_reqs = aida_core::git_ops::load_requirements_strict(store_path)
            .context("cannot sync: the local store did not read completely")?;

        println!("Pulling from origin/{}...", branch);
        if let Err(e) = aida_core::git_ops::pull_rebase(store_path, "origin", &branch) {
            // A failed rebase leaves the repo in a partial
            // state — but only advise `git rebase --abort`
            // when a rebase is actually in progress; a
            // transient failure (e.g. a network 502) with no
            // rebase in flight is a different situation.
            // Reuses the same check as `aida pull`'s
            // store-leg hint so both emission sites agree.
            // trace:BUG-1500 | ai:claude
            anyhow::bail!(
                "Pull failed: {}",
                crate::store_pull_failure_hint(store_path, &e.to_string())
            );
        }
        println!("  Pull complete.");
        let mut remote = RequirementsStore::default();
        remote.requirements = aida_core::git_ops::load_requirements_strict(store_path)
            .context("the pulled store did not read completely; nothing was pushed")?;
        let collisions = crate::find_spec_id_collisions(&remote);
        if !collisions.is_empty() {
            anyhow::bail!(
                "{}",
                crate::spec_id_collision_recovery_message(&collisions, store_path)
            );
        }
        // Detect conflicts with remote changes
        conflicts = Some(aida_core::conflict::detect_store_conflicts(
            &local_reqs,
            &remote.requirements,
        ));
    }

    Ok(DbSyncWindow {
        branch,
        head: aida_core::git_ops::head_sha(store_path).ok(),
        conflicts,
    })
}

/// A command the git-backend dispatcher does not serve. Shared with
/// registered store-phase commands that have no git-backend form.
// trace:STORY-1728 | ai:claude
pub(crate) fn exit_unsupported_on_git_backend() -> ! {
    eprintln!(
        "Command not yet supported for git backend.\n\
         Supported: list, add, show, edit, del, search, comment add/list,\n\
         queue list/add/remove/move/clear,\n\
         rel add/remove/list, db info/status/sync/merge-gate/export-git/workspace-init"
    );
    std::process::exit(1);
}

/// Arms BUG-681's fast-fail cache ladder for one `show` dispatch and restores
/// the prior mode on drop, early returns included.
// trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
struct ShowFastFail(bool);

impl ShowFastFail {
    fn arm() -> Self {
        Self(aida_core::db::set_fast_fail_cache(true))
    }
}

impl Drop for ShowFastFail {
    fn drop(&mut self) {
        aida_core::db::set_fast_fail_cache(self.0);
    }
}

pub(crate) fn handle_git_backend_command(
    store_path: &std::path::Path,
    command: &Command,
) -> Result<()> {
    // STORY-43: warn loudly when this clone has been hijacked. Runs once
    // per invocation before any command executes. The marker doesn't block
    // operations — issued ids remain valid — but the user needs to know
    // they're operating with a node id that no longer belongs to them.
    let marker_path = aida_core::node::HijackMarker::path_in_store(store_path);
    if let Ok(Some(marker)) = aida_core::node::HijackMarker::load(&marker_path) {
        eprintln!(
            "{} This clone's node id '{}' was reassigned by another clone on \
             {} at {}",
            "HIJACK WARNING:".red().bold(),
            marker.node_id,
            marker.new_owner_hostname,
            marker
                .hijacked_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M %Z"),
        );
        if let Some(p) = &marker.new_owner_clone_path {
            eprintln!("  New owner clone: {}", p.display());
        }
        eprintln!(
            "  Continuing to write requirements here will issue ids attributed to \
             the new owner. Run `aida node acquire` to claim a fresh id, or delete \
             {} once you've migrated.",
            marker_path.display()
        );
        eprintln!();
    }

    // BUG-1993: `show` records per-phase timings into its usage event, so a
    // latency regression can be attributed to cache open, lookup, or git
    // linkage instead of only the total. trace:BUG-1993 | ai:claude
    if command.name() == "show" {
        crate::usage::arm_phases();
    }
    // BUG-b6b1278890774ef993d2ebc327862470-008: every cache wait in `show`
    // (open, lookup, the advisory notice) uses BUG-681's short fast-fail
    // ladder, so behind a concurrent writer each one degrades — committed
    // snapshot, targeted YAML read, skipped notice — instead of spending
    // the 3 s read allowance per wait and stacking past ADR-53's 5 s
    // ceiling. Restored when this handler returns.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
    let _show_fast_fail = (command.name() == "show").then(ShowFastFail::arm);
    let open_phase = crate::usage::phase("open");
    let dispenser = load_dispenser(store_path)?;
    let inner = aida_core::GitBackend::new(store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
    // BUG-681: the per-turn `aida awaiting --notice` line is fired by the
    // UserPromptSubmit hook under Claude Code's 5s timeout. The notice is
    // ADVISORY — under cache-lock contention it must bail instantly (degrade to
    // empty) rather than block a prompt on the full ~25s cache retry ladder. Arm
    // fast-fail cache mode BEFORE opening the backend so BOTH the connection open
    // and the summary reads use the short (~150ms) ladder; on a lock-contended
    // open, print nothing and exit clean instead of erroring. Scoped to this one
    // command — every other command keeps the full resilient ladder.
    let notice_fast_fail = command
        .args::<crate::status_awaiting::AwaitingArgs>()
        .is_some_and(|args| args.notice);
    if notice_fast_fail {
        aida_core::db::set_fast_fail_cache(true);
    }
    // `aida cache rebuild` must not depend on the old cache being readable: the
    // ordinary open refreshes a stale cache first, and page-level damage fails
    // that refresh before the rebuild's own quarantine can run (2026-10-14).
    // Open the bare snapshot; the rebuild replaces it. trace:BUG-1940 | ai:claude
    let cache_rebuild = command
        .args::<crate::cache_cmd::CacheArgs>()
        .is_some_and(|args| matches!(args.inner, CacheCommand::Rebuild { history: false, .. }));
    let backend_result = if notice_fast_fail || cache_rebuild {
        // BUG-1569: even a non-contended normal open may refresh a stale cache
        // by scanning every canonical object before dispatch reaches the
        // targeted notice lookup. The advisory path instead consumes the last
        // committed snapshot and validates cache-located records with targeted
        // authoritative reads.
        aida_core::CachedGitBackend::with_inner_cache_snapshot(inner, &cache_path)
    } else {
        aida_core::CachedGitBackend::with_inner(inner, &cache_path)
    };
    let backend = match backend_result {
        Ok(backend) => backend,
        Err(_) if notice_fast_fail => {
            // Cache momentarily locked — the advisory notice degrades to empty.
            return Ok(());
        }
        // BUG-b6b1278890774ef993d2ebc327862470-008: the open's refresh cannot
        // run inside show's zero read budget — a writer holds the lock it
        // needs, or a pending migration makes the snapshot unservable (which
        // used to end `show` silently with no output). `show` reads the spec
        // from authoritative YAML anyway, so it degrades to the committed
        // snapshot; its reads then take their own degrade paths.
        // trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
        Err(e)
            if _show_fast_fail.is_some()
                && (aida_core::db::is_cache_lock_error(&e)
                    || e.chain().any(|c| {
                        c.is::<aida_core::db::cache_refresh::AdvisoryCacheUnavailable>()
                    })) =>
        {
            let inner =
                aida_core::GitBackend::new(store_path)?.with_dispenser(load_dispenser(store_path)?);
            aida_core::CachedGitBackend::with_inner_cache_snapshot(inner, &cache_path)
                .map_err(|_| e)?
        }
        Err(e) => return Err(e),
    };
    drop(open_phase);
    // BUG-b6b1278890774ef993d2ebc327862470-008: the pre-dispatch hygiene
    // checks (auto-push warning here, team-identity guard below) were time
    // `show` spent outside every named phase. trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
    let guard_phase = crate::usage::phase("guard");
    if let Some(project_root) = store_path.parent() {
        warn_if_periodic_auto_push(project_root);
    }
    drop(guard_phase);
    if !notice_fast_fail
        && !cache_rebuild
        && !command
            .args::<crate::report_cmd::ReportArgs>()
            .is_some_and(|args| args.recheck)
    {
        // BUG-2221: the upstream-recheck notice probes store freshness
        // (head/upstream reads) on every command; give `show` a named phase
        // for it rather than leaving it in the unassigned remainder. The
        // probe itself is unchanged — attribution only.
        // trace:BUG-2221 | ai:claude
        let _notice_phase = crate::usage::phase("notice");
        // BUG-b6b1278890774ef993d2ebc327862470-008: the notice is advisory
        // and skips itself on any cache error; on `show` it reads under the
        // fast-fail ladder armed above, so behind a concurrent writer it
        // degrades to no notice (re-checked next command) instead of blocking.
        // trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
        let storage = Storage::new(store_path);
        report_cmd::maybe_print_upstream_recheck_notice(&storage, &backend);
    }

    // STORY-640: team identity hygiene. In a TEAM context (a roster with >1
    // node, or a node other than this clone), a shared `"default"` identity
    // collides queues + attribution — flag it loudly; a fresh clone that joined
    // an existing roster gets a one-time onboarding nudge. Best-effort: solo /
    // single-own-node context is silent, and an unreadable roster never blocks.
    // Skips the `team` command itself (it surfaces the same facts) and the
    // machine-readable JSON status path. trace:STORY-640 | ai:claude
    let guard_phase = crate::usage::phase("guard");
    maybe_team_identity_guard(store_path, command)?;
    drop(guard_phase);

    // Friendly node labels for spec-ID input and display, read once per
    // process. trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    aida_core::node_alias::install_from_store(store_path);
    let flow = match command {
        // trace:STORY-1728 | ai:claude
        Command::Registered(invocation) => {
            let store = crate::command_registry::StoreCtx::Git {
                store_path,
                backend: &backend,
            };
            invocation.run_store(store, None)?
        }

        _ => exit_unsupported_on_git_backend(),
    };
    // BUG-b6b1278890774ef993d2ebc327862470-008: closing the cached backend
    // (connection and snapshot teardown) is its own `show` phase rather than
    // unmeasured time after the handler returns.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
    let close_phase = crate::usage::phase("close");
    drop(backend);
    drop(close_phase);
    if flow == crate::command_registry::Flow::Return {
        return Ok(());
    }

    if command_triggers_per_write_auto_push(command) {
        maybe_auto_push_store(store_path, StoreAutoPushMode::PerWrite, "per-write");
    }

    Ok(())
}
