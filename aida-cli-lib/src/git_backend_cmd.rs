//! `aida db` git-backend command cluster — the storage/git-backend command
//! handler (`handle_git_backend_command`), extracted from `main.rs` (SPIKE-78
//! / TASK-1151; pure movement, no behavior change). Shared store/git helpers
//! stay in `main.rs` and are reached via `crate::`.

#![allow(clippy::too_many_arguments)]

use anyhow::{Context, Result};
use serde::Serialize;

use crate::*;

// trace:TASK-190 | ai:codex
fn resolve_comment_body(
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
fn print_rework_needed_notes(
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

fn terminal_list_width() -> Option<usize> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        return None;
    }
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
}

struct ListTableOptions {
    show_origin: bool,
    show_tags: bool,
    no_glyph: bool,
    flow_header: String,
    terminal_width: Option<usize>,
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
fn is_spec_id_or_uuid(id: &str) -> bool {
    aida_core::object_store::valid_spec_id_format(id) || Uuid::parse_str(id.trim()).is_ok()
}

/// `aida list` must not emit two rows under one id. The display id prefers
/// `agreed_id`, so a spec whose agreed id collides with another spec's native
/// id rendered as a duplicate. Such rows drop the ambiguous agreed id for this
/// view and render under their native spec_id; a stderr note says which.
/// Best-effort: if the scan fails the rows are left untouched.
// trace:BUG-1535 | ai:claude
fn relabel_ambiguous_list_rows(
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

fn show_cached_context(
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

// trace:BUG-1207 | ai:codex
fn render_list_table<F>(
    reqs: &[aida_core::RequirementSummary],
    options: &ListTableOptions,
    row_routing: F,
) -> Vec<String>
where
    F: Fn(&aida_core::RequirementSummary) -> (bool, bool, bool),
{
    let flow_width = options.flow_header.chars().count();
    let header_prefix = if options.show_origin {
        format!(
            "{}{:<12} {:<14} {:<12} {:<13} ",
            options.flow_header, "ID", "Origin ID", "Type", "Status"
        )
    } else {
        format!(
            "{}{:<14} {:<12} {:<13} {:<10} ",
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
    });

    let render_status = |r: &aida_core::RequirementSummary| -> String {
        let label = status_display::display_status_for_type(&r.req_type, &r.status);
        if options.no_glyph {
            status_display::status_cell_no_glyph(label, 13)
        } else {
            status_display::status_cell(label, 11)
        }
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

    for req in reqs {
        let display_id = req
            .agreed_id
            .as_deref()
            .or(req.spec_id.as_deref())
            .unwrap_or("?");
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
                "{}{:<12} {} {:<12} {} ",
                flow_prefix(req),
                display_id,
                origin_cell,
                req.req_type,
                status
            )
        } else {
            format!(
                "{}{:<14} {:<12} {} {:<10} ",
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
            in_degree: 0,
            out_degree: 0,
            heft: 0,
            blocked: false,
            has_pending_decision: false,
            execution_mode: None,
            weight: None,
            origin: None,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProxyApprovalEntry {
    spec_id: String,
    title: String,
    comment_id: String,
    comment_author: String,
    created_at: String,
    decision: String,
    verdict: String,
    proxy: String,
    session: String,
    authorized_by: String,
    independent: String,
    timestamp_local: String,
}

fn add_proxy_approval(
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

fn list_proxy_approvals(
    backend: &aida_core::CachedGitBackend,
    since: Option<&str>,
    until: Option<&str>,
    json: bool,
) -> Result<()> {
    // trace:STORY-1173 | ai:codex
    let since = since.map(parse_since_arg).transpose()?;
    let until = until.map(parse_since_arg).transpose()?;
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
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    if entries.is_empty() {
        println!("No proxy approvals found");
        return Ok(());
    }
    print_proxy_approval_entries(&entries, true);
    Ok(())
}

fn print_proxy_approval_entries(entries: &[ProxyApprovalEntry], include_spec: bool) {
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

fn collect_proxy_approval_entries(
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
}

/// `aida edit --status` — emit the seat-tagged `DispositionChanged` event
/// this invocation's status transition produced (approve/reject/defer-class
/// decisions, including groom's `--apply`, which shells out to `aida edit`).
/// Split out for direct unit testing, same rationale as BUG-1423's
/// `emit_ship_pr_merged`. Best-effort: `events::emit` never fails.
// trace:TASK-1450 | ai:claude
fn emit_edit_disposition_changed(
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
fn emit_edit_execution_mode_changed(
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
        let _seat = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "advisor");

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
        let _seat = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "advisor");

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
    let notice_fast_fail = matches!(command, Command::Awaiting { notice: true, .. });
    if notice_fast_fail {
        aida_core::db::set_fast_fail_cache(true);
    }
    let backend_result = if notice_fast_fail {
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
        Err(e) => return Err(e),
    };
    if let Some(project_root) = store_path.parent() {
        warn_if_periodic_auto_push(project_root);
    }
    if !notice_fast_fail && !matches!(command, Command::Report { recheck: true, .. }) {
        let storage = Storage::new(store_path);
        report_cmd::maybe_print_upstream_recheck_notice(&storage);
    }

    // STORY-640: team identity hygiene. In a TEAM context (a roster with >1
    // node, or a node other than this clone), a shared `"default"` identity
    // collides queues + attribution — flag it loudly; a fresh clone that joined
    // an existing roster gets a one-time onboarding nudge. Best-effort: solo /
    // single-own-node context is silent, and an unreadable roster never blocks.
    // Skips the `team` command itself (it surfaces the same facts) and the
    // machine-readable JSON status path. trace:STORY-640 | ai:claude
    maybe_team_identity_guard(store_path, command)?;

    match command {
        Command::Cache(cache_cmd) => {
            return cache_cmd::handle_cache_command(cache_cmd, &backend);
        }
        Command::Record(record_cmd) => {
            // STORY-582: inspect / prune the durable processing-record trail.
            return record_cmd::handle_record_command(record_cmd, &backend, store_path);
        }
        Command::Protocol(protocol_cmd) => {
            let storage = Storage::new(store_path);
            return crate::protocol_cmd::handle_protocol_command(protocol_cmd, &storage);
        }
        Command::Schedule(schedule_cmd) => {
            return maintenance_schedule::handle_schedule_command(
                schedule_cmd,
                store_path,
                Some(&backend),
            );
        }
        Command::Mailbox(mailbox_cmd) => {
            // trace:STORY-493 | ai:claude — local layer only; git-canonical
            // digest is a later slice. Needs only the project root.
            return mailbox_cmd::handle_mailbox_command(mailbox_cmd, store_path);
        }
        Command::Supervise(supervise_cmd) => {
            // trace:STORY-1052 | ai:codex
            return supervise_cmd::handle_supervise_command(supervise_cmd, &backend, store_path);
        }
        Command::Node(node_cmd) => {
            return node_cmd::handle_node_command(node_cmd, store_path);
        }
        Command::Docs(docs_cmd) => {
            // trace:FR-1-077 | ai:claude
            let store = backend.load()?;
            return doc_cmd::handle_docs_with_store(docs_cmd, &store);
        }
        Command::Rules(rules_cmd) => {
            // trace:SPIKE-31 | ai:claude
            return rules_cmd::handle_rules_command(rules_cmd, &backend);
        }
        Command::Mcp(mcp_cmd) => {
            // trace:STORY-361 | ai:claude
            return handle_mcp_command(mcp_cmd);
        }
        Command::Agent(agent_cmd) => {
            return handle_agent_command(agent_cmd);
        }
        Command::Report { recheck, command } => {
            let storage = Storage::new(store_path.to_path_buf());
            let db_path_str = store_path.display().to_string();
            return report_cmd::handle_report_command(
                *recheck,
                command.as_ref(),
                &storage,
                &db_path_str,
            );
        }
        Command::Advisor { short, command } => {
            // STORY-262 / STORY-559: two advisor subcommands reach storage init
            // — `schedule` (files TASKs) and the default `advisor status`
            // dashboard (aggregates the store). STORY-618: bare `aida advisor`
            // (command = None) reaches here too — it IS the advisor worklist.
            // The narrow `advisor status --registration` view and
            // register/unregister dispatch before storage init and never reach
            // here. trace:STORY-262 trace:STORY-559 trace:STORY-618 | ai:claude
            match command {
                // STORY-618: bare `aida advisor` → the advisor's actionable
                // worklist (the mirror of bare `aida human`).
                None => {
                    return handle_advisor_worklist(*short, &backend, store_path);
                }
                Some(AdvisorCommand::Schedule(sched_cmd)) => {
                    let project_root = store_path
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    return schedule_cmd::handle_schedule_command(
                        sched_cmd,
                        &project_root,
                        store_path,
                    );
                }
                Some(AdvisorCommand::Status {
                    json,
                    registration: false,
                }) => {
                    return handle_advisor_dashboard(*json, &backend, store_path);
                }
                _ => unreachable!("non-dashboard/schedule advisor subcommands dispatch early"),
            }
        }
        Command::Brief {
            agent,
            spec,
            note,
            depends_on,
            as_deep_link,
            notify,
            authorized_by,
            cmd,
        } => {
            let store = backend.load()?;
            let project_root = store_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;
            return brief_cmd::handle_brief_command(
                agent.as_deref(),
                spec.as_deref(),
                note.as_deref(),
                depends_on.as_deref(),
                *as_deep_link,
                *notify,
                authorized_by.as_deref(),
                cmd,
                &store,
                project_root,
            );
        }
        Command::Status {
            // The `spec` form is dispatched early (before storage init), like
            // `aida why` — see the early-dispatch block. By the time control
            // reaches here `spec` is always `None`. trace:STORY-694
            spec: _,
            idle_minutes: _,
            no_dev_context,
            short,
            json,
            queue,
            ci,
            no_ci,
            cleanup,
            activity,
            since,
            awaiting,
            verbose,
            no_hygiene,
            all,
            stale,
            full,
            no_focus,
        } => {
            // STORY-764: honor the global `--format json` pin the same as the
            // command's own `--json`. trace:STORY-764 | ai:claude
            let effective_json = *json || output_format_is_json();
            // STORY-706: surface the active focus loudly at the top of the
            // orientation snapshot, so a focused worktree never reads `aida
            // status` as the whole-project picture. Text modes only (JSON
            // consumers parse structured sections); `--no-focus` suppresses it.
            //
            if !effective_json && !*no_focus {
                if let Some(focus_ref) = find_project_root()
                    .ok()
                    .and_then(|r| crate::focus::resolve_focus(&r))
                {
                    if let Ok(Some(focus_req)) = backend.get_requirement_by_spec_id(&focus_ref) {
                        let n = backend
                            .descendant_ids(&focus_req.id)
                            .map(|s| s.len())
                            .unwrap_or(0);
                        println!(
                            "{}",
                            format!(
                                "\u{25b8} focused: {} \u{2014} {} item{} in subtree  \
                                 (--no-focus to widen)",
                                focus_req.display_id(),
                                n,
                                if n == 1 { "" } else { "s" },
                            )
                            .cyan()
                            .bold()
                        );
                    }
                }
            }
            return handle_status_command_distributed(
                *no_dev_context,
                *short,
                effective_json,
                *queue,
                *ci,
                *no_ci,
                *cleanup,
                *activity,
                since.as_deref(),
                *awaiting,
                *verbose,
                *no_hygiene,
                *all,
                *stale,
                *full,
                store_path,
                &backend,
            );
        }
        // STORY-741: `aida awaiting` — the unified coordination inbox as a
        // first-class command (the same "Awaiting you" report that leads
        // `aida status`, now including unread mail). `--notice` is the compact,
        // cache/local-backed per-turn line the UserPromptSubmit hook injects.
        // trace:STORY-741 | ai:claude
        Command::Awaiting {
            notice,
            json,
            verbose,
            no_ci,
        } => {
            // trace:BUG-1289 | ai:claude
            return handle_awaiting_command(
                *notice,
                *json || output_format_is_json(),
                *verbose,
                *no_ci,
                &backend,
            );
        }
        // — `aida focus` set/show/clear.
        Command::Focus {
            target,
            clear,
            show,
            cmd,
        } => {
            let (clear, show) = match cmd {
                Some(FocusCommand::Clear) => (true, false),
                Some(FocusCommand::Show) => (false, true),
                None => {
                    if *clear {
                        note_hidden_alias(concat!("aida focus ", "--clear"), "aida focus clear");
                    }
                    if *show {
                        note_hidden_alias(concat!("aida focus ", "--show"), "aida focus show");
                    }
                    (*clear, *show)
                }
            };
            return focus_cmd::handle_focus_command(target.as_deref(), clear, show, &backend);
        }
        Command::Team { json, cmd } => {
            // trace:STORY-640 | ai:claude
            // trace:STORY-646 | ai:claude
            return match cmd {
                None => team_cmd::handle_team_command(store_path, *json),
                Some(TeamCommand::SetRole { user, role }) => {
                    team_cmd::handle_team_set_role(store_path, user, role)
                }
                Some(TeamCommand::MyRole { json }) => {
                    team_cmd::handle_team_my_role(store_path, *json)
                }
                Some(TeamCommand::UnsetRole { user }) => {
                    team_cmd::handle_team_unset_role(store_path, user)
                }
            };
        }
        // trace:TASK-845 | ai:claude — the shared person-alias registry.
        Command::Identity { cmd } => {
            return match cmd {
                IdentityCommand::Link { a, b } => handle_identity_link(store_path, a, b),
                IdentityCommand::List { json } => handle_identity_list(store_path, *json),
                IdentityCommand::Show { id, json } => handle_identity_show(store_path, id, *json),
            };
        }
        Command::Usage {
            since,
            unused,
            errors,
            json,
            limit,
            auto_complete,
            failures,
            pattern,
            health,
            read_write,
            slowest,
            events,
            cmd,
            slower_than,
            action,
        } => {
            // The distributed backend dispatches before the legacy handler in
            // lib.rs, so the token-ledger subcommands must terminate here too.
            // trace:TASK-1427 | ai:codex
            if let Some(action) = action {
                match action {
                    UsageCommand::Rebuild { project, json } => {
                        let root = project.clone().unwrap_or_else(|| {
                            store_path.parent().unwrap_or(store_path).to_path_buf()
                        });
                        return token_ledger::rebuild(&root, *json || output_format_is_json());
                    }
                    UsageCommand::Show {
                        spec,
                        group_by,
                        cost,
                        json,
                        toon,
                    } => {
                        let root = store_path.parent().unwrap_or(store_path);
                        return token_ledger::show(
                            root,
                            spec,
                            group_by,
                            *cost,
                            *json || output_format_is_json(),
                            *toon,
                        );
                    }
                    _ => {}
                }
            }
            // TASK-266: load the store only for the `--auto-complete` view,
            // which resolves drafted-BUG statuses; plain usage stays cheap.
            // STORY-530: the `--health` catalog also needs the store.
            let (unused, errors, auto_complete, failures, pattern, health, slowest, events) =
                normalize_usage_mode(
                    unused.as_deref(),
                    *errors,
                    *auto_complete,
                    *failures,
                    *pattern,
                    *health,
                    *slowest,
                    *events,
                    action.as_ref(),
                );
            let store = if auto_complete || health {
                backend.load().ok()
            } else {
                None
            };
            return usage_cmd::handle_usage_command(
                since,
                unused,
                errors,
                // trace:BUG-1289 | ai:claude
                *json || output_format_is_json(),
                *limit,
                auto_complete,
                failures,
                pattern,
                health,
                *read_write,
                slowest,
                events,
                cmd.as_deref(),
                *slower_than,
                store.as_ref(),
            );
        }
        Command::Metrics { cmd } => {
            // trace:STORY-477 | ai:claude — reporting layer over the local
            // telemetry logs; no store load required.
            return metrics_cmd::handle_metrics_command(cmd);
        }
        Command::FieldStudy { cmd } => {
            // trace:SPIKE-67 | ai:claude — observe-only rule-adherence study
            // over the git log; opt-in, local-only, no store load required.
            return field_study_cmd::handle_field_study_command(cmd);
        }
        Command::Digest {
            since,
            audience,
            digest_format,
            include_next,
            include_process,
            out,
            copy,
            reset,
        } => {
            let store = backend.load()?;
            return digest_cmd::handle_digest_command(
                since.as_deref(),
                *audience,
                *digest_format,
                *include_next,
                *include_process,
                out.clone(),
                *copy,
                *reset,
                &store,
            );
        }
        Command::Push {
            code_only,
            store_only,
            message,
            no_rebase_check,
            dry_run,
            json,
            no_notice,
        } => {
            // TASK-106: AIDA_PUSH_DEFAULT lets a user flip the default
            // scope when neither flag is passed explicitly.
            let (code_only, store_only) = resolve_push_scope(
                *code_only,
                *store_only,
                std::env::var("AIDA_PUSH_DEFAULT").ok().as_deref(),
            );
            // TASK-108: --dry-run (or --json, which implies it) prints
            // the plan and exits without touching origin.
            if *dry_run || *json {
                return handle_push_dry_run(store_path, code_only, store_only, *json);
            }
            return handle_push_command(
                store_path,
                code_only,
                store_only,
                message.as_deref(),
                *no_rebase_check,
                *no_notice,
            );
        }
        Command::Pull {
            code_only,
            store_only,
            quiet,
            no_gate,
            dry_run,
            json,
            auto,
        } => {
            // TASK-108: --dry-run fetches, reports, and exits.
            if *dry_run || *json {
                return handle_pull_dry_run(store_path, *code_only, *store_only, *json);
            }
            return handle_pull_command(
                store_path,
                *code_only,
                *store_only,
                *quiet,
                *no_gate,
                *auto,
            );
        }
        Command::Fetch {
            code_only,
            store_only,
            quiet,
        } => {
            return handle_fetch_command(store_path, *code_only, *store_only, *quiet);
        }
        Command::Rebase {
            auto,
            dry_run,
            no_fetch,
            no_stash,
            json,
            branch,
        } => {
            return rebase_cmd::handle_rebase_command(
                store_path,
                *auto,
                *dry_run,
                *no_fetch,
                *no_stash,
                *json,
                branch.as_deref(),
            );
        }
        Command::Upgrade { .. } => unreachable!("upgrade is dispatched before storage init"),
        Command::Memories(_) => unreachable!("memories is dispatched before storage init"),
        Command::Schema { .. } => unreachable!("schema is dispatched before storage init"),
        Command::Skill(_) => unreachable!("skill is dispatched before storage init"),
        Command::Dev(_) => unreachable!("dev is dispatched before storage init"),
        Command::Release { .. } => unreachable!("release is dispatched before storage init"),
        Command::Burndown(_) => unreachable!("burndown is dispatched before storage init"),
        Command::Health { .. } => unreachable!("health is dispatched before storage init"),
        Command::Groom { .. } => {
            unreachable!("groom (assess/intake) is dispatched before storage init")
        }
        // trace:TASK-1147
        Command::Autopilot(_) => {
            unreachable!("autopilot is dispatched before storage init")
        }
        Command::Why { .. } => unreachable!("why is dispatched before storage init"),
        Command::Intent { .. } => unreachable!("intent is dispatched before storage init"),
        // trace:STORY-696
        Command::Ps { .. } => unreachable!("ps is dispatched before storage init"),
        // trace:TASK-1167
        Command::Tail { .. } => unreachable!("tail is dispatched before storage init"),
        Command::Watch { .. } => unreachable!("watch is dispatched before storage init"),
        Command::Contract { .. } => unreachable!("contract is dispatched before storage init"),
        // trace:TASK-1034
        Command::Integrate { .. } => {
            unreachable!("integrate is dispatched before storage init")
        }
        // trace:TASK-957
        Command::Claim { .. } => unreachable!("claim is dispatched before storage init"),
        Command::Unclaim { .. } => unreachable!("unclaim is dispatched before storage init"),
        Command::Spec(_) => unreachable!("spec subcommands are dispatched before storage init"),
        Command::Worktree(_) => {
            unreachable!("worktree subcommands are dispatched before storage init")
        }
        Command::Doctor { .. } => unreachable!("doctor is dispatched before storage init"),
        Command::Store(_) => unreachable!("store is dispatched before storage init"),
        Command::Remote(_) => unreachable!("remote is dispatched before storage init"),
        Command::Sandbox(_) => unreachable!("sandbox is dispatched before storage init"),
        Command::HelpAll => unreachable!("help-all is dispatched before storage init"),
        Command::Alias { .. } => unreachable!("alias is dispatched before storage init"),
        Command::Plan(_) => unreachable!("plan is dispatched before storage init"),
        Command::Deps(_) => unreachable!("deps is dispatched before storage init"),
        Command::Lint { .. } => unreachable!("lint is dispatched before storage init"),
        Command::Gate(_) => unreachable!("gate is dispatched before storage init"),
        Command::Lifecycle { .. } => {
            unreachable!("lifecycle is dispatched before storage init")
        }
        Command::Changelog(_) => unreachable!("changelog is dispatched before storage init"),
        Command::Manual { .. } => unreachable!("manual is dispatched before storage init"),
        Command::Ultraplan { .. } => unreachable!("ultraplan is dispatched before storage init"),
        Command::Compete { .. } => unreachable!("compete is dispatched before storage init"),
        Command::Goal { .. } => unreachable!("goal is dispatched before storage init"),
        Command::Commit { .. } => unreachable!("commit is dispatched before storage init"),
        Command::Internal { .. } => unreachable!("internal is dispatched before storage init"),
        Command::Tui { .. } => unreachable!("tui is dispatched before storage init"),
        Command::Role(_) => unreachable!("role is dispatched before storage init"),
        Command::Statusline { .. } => unreachable!("statusline is dispatched before storage init"),
        Command::BgFetch { .. } => unreachable!("_bg-fetch is dispatched before storage init"),
        Command::Away | Command::Home | Command::Presence { .. } | Command::Solo { .. } => {
            unreachable!("presence/solo commands are dispatched before storage init")
        }
        // trace:TASK-784
        Command::Whoami => unreachable!("whoami is dispatched before storage init"),
        Command::Session(_) => unreachable!("session is dispatched before storage init"),
        Command::Triage(_) => unreachable!("triage is dispatched before storage init"),
        Command::Lock(_) => unreachable!("lock is dispatched before storage init"),
        Command::Pr(_) => unreachable!("pr is dispatched before storage init"),
        Command::Ship { .. } => unreachable!("ship is dispatched before storage init"),
        Command::Orchestrator(_) => {
            unreachable!("orchestrator is dispatched before storage init")
        }
        // STORY-721: `aida zen <spec>` autonomous implement+ship drive. The
        // introspection subcommands (`zen status` / `finish` / `needs-human`)
        // short-circuit before storage init in `run()`; this arm only sees the
        // spec-drive form (`command: None`), which needs the store to validate
        // the spec's status before driving it. trace:STORY-721 | ai:claude
        Command::Zen {
            spec,
            supervised,
            no_human,
            no_pull,
            force,
            solo,
            into_epic,
            dry_run,
            json,
            vendor,
            // trace:STORY-722 — the --compete form dispatches before storage init.
            compete: _,
            command: _,
        } => {
            let storage = Storage::new(store_path.to_path_buf());
            // STORY-744: the machine-readable gate probe short-circuits the
            // drive — resolve + classify the suitability gate and print the
            // verdict as JSON so a shell-out consumer (the TUI drive verb) reads
            // the SAME gate the drive runs instead of parsing human prose.
            if *json {
                return run_zen_gate_json(&storage, spec.as_deref());
            }
            // TASK-1116: a per-invocation `--vendor`/`--agent` picks the headless
            // implementer for this drive (top precedence). Installed before the
            // self-invoked `aida queue work --auto-complete` child spawns so it
            // inherits the choice via `AIDA_HEADLESS_VENDOR`.
            apply_drive_vendor_override(vendor.as_deref())?;
            let user_id = current_user_id(None);
            return run_zen_drive(
                &storage,
                Some(store_path),
                &user_id,
                spec.as_deref(),
                no_human.as_deref(),
                *supervised,
                *no_pull,
                *force,
                *solo,
                *into_epic,
                *dry_run,
            );
        }
        // BUG-735: TASK-1155 wired `aida do` only into the legacy dispatch, so
        // the git-canonical path fell through to the catch-all refusal. Mirror
        // the Zen arm: same storage, same thin delegate.
        // trace:BUG-735 | ai:claude
        Command::Do { spec, mode, force } => {
            let storage = Storage::new(store_path.to_path_buf());
            return run_do_drive(&storage, spec, mode.as_deref(), *force);
        }
        Command::Drain(_) => unreachable!("drain is dispatched before storage init"),
        Command::Stack(_) => unreachable!("stack is dispatched before storage init"),
        Command::Worker(_) => unreachable!("worker is dispatched before storage init"),
        Command::Headless(_) => unreachable!("headless is dispatched before storage init"),
        Command::List {
            shortcut,
            status,
            r#type,
            priority,
            feature,
            tags,
            machine_drafts,
            no_scope,
            show_origin,
            include_meta,
            parent,
            recursive,
            sync,
            all,
            archived,
            deferred,
            json,
            tree,
            show_tags,
            blocked,
            no_flow,
            no_glyph,
            short,
            human,
            sort,
            // trace:FR-283 | ai:claude
            min_weight,
            max_weight,
            mine,
            assigned,
            user,
            limit,
            no_focus,
            fields,
            ..
        } => {
            // STORY-764: the global `--format json` pin reaches list the same way
            // its own `--json` flag does. `--format human` / `toon` are handled
            // upstream by `agent_output_mode()`, so only the json axis needs to be
            // OR'd in here. trace:STORY-764 | ai:claude
            let effective_json = *json || output_format_is_json();
            // STORY-562: `aida list human` (positional alias) and `aida list
            // --human` both resolve to the "what needs me?" view — every open
            // spec the `burndown explain` classifier flags as needing a human
            // nudge, grouped by reason. Detect it BEFORE the status-shortcut
            // expansion so the positional `human` token isn't rejected as an
            // unknown status. Composes with `--short` (ids-only). trace:STORY-562
            let want_human = *human
                || shortcut
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case("human"))
                    .unwrap_or(false);
            if want_human {
                if let Some(s) = status.as_deref() {
                    anyhow::bail!(
                        "`aida list human` is the human-attention view, not a status \
                         filter — drop `--status {s}`. (Use plain `aida list --status \
                         {s}` for a status listing.)"
                    );
                }
                if *sync {
                    maybe_sync_pull(store_path)?;
                }
                return handle_list_human(*short, &backend);
            }
            // STORY-662: resolve the owner-or-assignee `--user` filter. The
            // positional `me` / `user:<name>` token is an alternate spelling of
            // `--user me` / `--user <name>` — peel it off BEFORE the status
            // shortcut expansion so it isn't rejected as an unknown status. `me`
            // resolves to the shell identity via `current_user_id` (the same
            // resolution the queue keys off — $AIDA_USER / $USER). `aida list`
            // does NOT default to the current user; this is an opt-in filter.
            // trace:STORY-662 | ai:claude
            let positional_user: Option<String> = shortcut.as_deref().and_then(|s| {
                if s.eq_ignore_ascii_case("me") {
                    Some("me".to_string())
                } else {
                    s.strip_prefix("user:")
                        .filter(|rest| !rest.is_empty())
                        .map(|rest| rest.to_string())
                }
            });
            let raw_user: Option<String> = match (positional_user, user.clone()) {
                (Some(_), Some(_)) => {
                    anyhow::bail!(
                        "Pass the user filter once: either the positional \
                         `aida list me` / `aida list user:<name>` or `--user <name>`, \
                         not both."
                    );
                }
                (Some(u), None) | (None, Some(u)) => Some(u),
                (None, None) => None,
            };
            // `me` → the shell identity; any other value is a literal handle.
            let user_filter: Option<String> =
                raw_user.map(|u| resolve_list_user_filter(&u, &current_user_id(None)));
            // The positional `me` / `user:<name>` token was consumed as a user
            // filter, so it must not also flow into the status-shortcut path.
            let shortcut: Option<String> = if user_filter.is_some()
                && shortcut
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case("me") || s.starts_with("user:"))
                    .unwrap_or(false)
            {
                None
            } else {
                shortcut.clone()
            };
            // TASK-0415: resolve the optional positional status shortcut.
            // `aida list approved` == `aida list --status approved`; the
            // positional and `--status` are two spellings of the same axis,
            // so reject using both at once rather than silently picking one.
            // Both forms run through `expand_filter_spec`, which expands the
            // `open` / `closed` aliases and the comma-separated OR set, and
            // errors clearly on an unrecognized token (pointing at the real
            // filters) instead of swallowing it.
            // trace:TASK-0415 | ai:claude
            let raw_status: Option<String> = match (shortcut.as_deref(), status.as_deref()) {
                (Some(_), Some(_)) => {
                    anyhow::bail!(
                        "Pass the status filter once: either the positional \
                         `aida list <status>` or `--status <status>`, not both."
                    );
                }
                (Some(s), None) | (None, Some(s)) => Some(s.to_string()),
                (None, None) => None,
            };
            if *machine_drafts
                && !raw_status
                    .as_deref()
                    .is_some_and(crate::status_spec_is_exact_draft)
            {
                anyhow::bail!(
                    "`--machine-drafts` requires the exact draft view: \
                     `aida list --status draft --machine-drafts`"
                );
            }
            // BUG-788: capture whether this is the explicit `open` shortcut
            // BEFORE `raw_status` is expanded into the canonical status set —
            // once expanded, `open` is indistinguishable from a hand-typed
            // `draft,approved,...` list. The explicit `open` verb must share the
            // bare-list default lens's accepted-decision exclusion (BUG-781), so
            // `aida list` and `aida list open` agree. trace:BUG-788 | ai:claude
            let explicit_open_alias = crate::status_spec_is_open_alias(raw_status.as_deref());
            let exact_draft_view = raw_status
                .as_deref()
                .is_some_and(crate::status_spec_is_exact_draft);
            let status: Option<String> = match raw_status {
                Some(spec) => {
                    let expanded = aida_core::RequirementStatus::expand_filter_spec(&spec)
                        .map_err(|tok| {
                            anyhow::anyhow!(
                                "Unknown status filter '{tok}'. Use a status \
                                 (draft, approved, planned, in-progress, done, \
                                 completed, rejected, needs-attention), an alias \
                                 (open, closed), or a comma-separated set \
                                 (draft,approved). To filter by something else \
                                 try --type, --tags, or `aida search`."
                            )
                        })?;
                    // expand_filter_spec returns canonical cache-keys; join
                    // them back into the comma-OR spec the cache understands.
                    Some(expanded.join(","))
                }
                None => None,
            };
            // STORY-78: opt-in implicit sync-pull before reading. Quiet
            // on no-op (already current), warns + falls back on errors.
            // Must run BEFORE the cache-backed list query because the
            // cache rebuild on next read will pick up the new orphan
            // HEAD. trace:STORY-78 | ai:claude
            if *sync {
                maybe_sync_pull(store_path)?;
            }
            // Cache-backed list (EPIC-1-001 Phase 2). The CachedGitBackend
            // ensures the cache is fresh before querying, so this is one
            // SQLite query instead of ~360 YAML reads.
            // trace:EPIC-1-001 | ai:claude
            //
            // Phase 3 scope filters: when a role is active and has scope set,
            // its tags/status are AND'd into the filter. Explicit --tags or
            // --status on the command line override the role scope; --no-scope
            // bypasses it entirely.
            // trace:TASK-1-021 | ai:claude
            let cli_tags: Vec<String> = tags
                .as_deref()
                .map(|t| {
                    t.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let scope = if *no_scope { None } else { active_role_scope() };
            let (effective_tags, effective_status) = match scope {
                Some((scope_tags, scope_status)) => {
                    let final_tags = if !cli_tags.is_empty() {
                        cli_tags
                    } else {
                        scope_tags
                    };
                    let final_status = status.clone().or(scope_status);
                    (final_tags, final_status)
                }
                None => (cli_tags, status.clone()),
            };
            // STORY-441: three-way archive axis. Default hides archived rows;
            // `--archived` shows only archived; `--all` shows both. The
            // pre-STORY-441 terminal-status hide is gone — Completed/Rejected
            // specs stay visible until archived.
            // trace:STORY-441 | ai:claude
            let archive = if *all {
                aida_core::ArchiveFilter::Both
            } else if *archived {
                aida_core::ArchiveFilter::ArchivedOnly
            } else {
                aida_core::ArchiveFilter::NonArchivedOnly
            };
            // STORY-584: three-way defer axis, parallel to archive. Default
            // hides deferred rows (flag set OR `deferred:*`-tagged); `--deferred`
            // shows only the primed shelf; `--all` shows the union. `--archived`
            // keeps the defer axis open so the archive audit is complete.
            // If the user explicitly filters on a `deferred:` tag, opening the
            // axis is implied — otherwise the default hide would contradict the
            // tag filter and return nothing. trace:STORY-584 | ai:claude
            let asked_for_defer_tag = effective_tags
                .iter()
                .any(|t| t.starts_with("deferred:") || t.starts_with("deferred*"));
            let defer = if *all || *archived || asked_for_defer_tag {
                aida_core::DeferFilter::Both
            } else if *deferred {
                aida_core::DeferFilter::DeferredOnly
            } else {
                aida_core::DeferFilter::NonDeferredOnly
            };
            // STORY-723: bare `aida list` defaults to the OPEN/actionable lens.
            // A newcomer's first list should show LIVE work (draft → approved →
            // planned → in-progress → needs-attention), not a wall of completed/
            // rejected history. The default applies ONLY when no status filter is
            // already in play (neither `--status`/positional nor a role scope
            // status) and the view wasn't deliberately widened — `--all`,
            // `--archived`, and `--deferred` each opt back into closed rows, and
            // an explicit `aida list closed` / `--status completed` is untouched.
            // The closed set stays one flag away (`--all` / `--status closed`).
            // trace:STORY-723 | ai:claude
            let default_open_lens =
                list_default_open_lens(effective_status.is_some(), *all, *archived, *deferred);
            let effective_status = if default_open_lens {
                Some(
                    aida_core::RequirementStatus::open_statuses()
                        .iter()
                        .map(|s| s.cache_key())
                        .collect::<Vec<_>>()
                        .join(","),
                )
            } else {
                effective_status
            };
            // STORY-632: resolve the --sort order. Unknown values fall back to
            // the default (freshest-first) with a stderr note rather than
            // erroring the listing. trace:STORY-632 | ai:claude
            let sort_order = match sort.to_ascii_lowercase().as_str() {
                "heft" | "centrality" => aida_core::SortOrder::HeftDesc,
                // trace:FR-283 | ai:claude — heaviest user-set weight first.
                "weight" => aida_core::SortOrder::WeightDesc,
                "modified" | "" => aida_core::SortOrder::ModifiedDesc,
                other => {
                    eprintln!(
                        "warning: unknown --sort '{other}' (expected 'modified', 'heft', or 'weight'); using 'modified'"
                    );
                    aida_core::SortOrder::ModifiedDesc
                }
            };
            // STORY-639: `--mine` resolves to the shell identity (the same
            // user the queue keys off); `--assigned <user>` filters on that
            // user. The two are mutually exclusive (enforced by clap).
            // trace:STORY-639 | ai:claude
            let assignee_filter: Option<String> = if *mine {
                Some(current_user_id(None))
            } else {
                assigned.clone()
            };
            // trace:TASK-845 | ai:claude — expand the assignee filter to the
            // canonical person's full alias set so `--mine` / `--assigned`
            // surfaces specs assigned under any of this human's cross-host owner
            // strings. Empty by default (no aliases linked) → plain match.
            let assignee_aliases: Vec<String> = match &assignee_filter {
                Some(a) => {
                    let registry = aida_core::alias::AliasRegistry::load(store_path);
                    let folded = aida_core::node::canonical_user_id(a);
                    registry
                        .members_of(a)
                        .into_iter()
                        .filter(|m| *m != folded)
                        .collect()
                }
                None => Vec::new(),
            };
            let filter = aida_core::ListFilter {
                status: effective_status.clone(),
                req_type: r#type.clone(),
                // trace:TASK-1-107 | ai:claude — was missing before;
                // CLI accepted --priority but it never reached the query.
                priority: priority.clone(),
                feature: feature.clone(),
                tags: effective_tags.clone(),
                archive,
                defer,
                sort: sort_order,
                assignee: assignee_filter,
                assignee_aliases,
                // trace:STORY-662 | ai:claude — `--user` / `me` / `user:<name>`.
                owner_or_assignee: user_filter.clone(),
                // trace:FR-283 | ai:claude — numeric weight bounds.
                min_weight: *min_weight,
                max_weight: *max_weight,
                ..Default::default()
            };
            let mut reqs = backend.list_summaries(&filter)?;

            // TASK-1456 (follow-up to BUG-1515): the open lens's status set
            // excludes `Done` (it sits with Completed/Rejected on the closed
            // side), but a Done spec whose PR carries a still-live refusal at
            // its tip is not finished — it needs a rework round, and BUG-1515
            // already taught `aida queue work`/`next` to read it that way.
            // Left alone, it simply vanished from the main work list. Re-run
            // the SAME filter (every other axis unchanged) scoped to `done`
            // and fold in only the rows `select_done_rework_rows` confirms
            // have an outstanding refusal, so the fold-in can never disagree
            // with `queue_fresh_pickup_policy`. Runs before every downstream
            // lens (parent/focus/meta/standing-type/etc.) so these rows are
            // scoped identically to the rest of the view.
            //
            // Perf (review follow-up): `select_done_rework_rows` resolves the
            // primary worktree root ONCE for the whole batch, reads local
            // verdict files only (no git spawn) to narrow to candidates, and
            // pays for the git tip/ancestry check on that small, capped
            // candidate set — see its doc comment. `rework_ids` is the set it
            // computed; every render site below reuses it by plain membership
            // check instead of recomputing. trace:TASK-1456 | ai:claude
            let mut rework_ids: std::collections::HashSet<uuid::Uuid> =
                std::collections::HashSet::new();
            if default_open_lens {
                let mut done_filter = filter.clone();
                done_filter.status =
                    Some(aida_core::RequirementStatus::Done.cache_key().to_string());
                let done_rows = backend.list_summaries(&done_filter)?;
                let (rework_rows, ids) =
                    queue_cmd::select_done_rework_rows(done_rows, store_path.parent());
                reqs.extend(rework_rows);
                rework_ids = ids;
            }
            // BUG-1535: never two rows under one id. A row whose displayed
            // (agreed) id also names another spec is shown by its native id
            // instead, with a note. trace:BUG-1535 | ai:claude
            relabel_ambiguous_list_rows(&backend, &mut reqs);

            // STORY-62: --parent <id> restricts to direct children of <id>.
            // We don't materialize a parent->children index in the cache;
            // for one parent it's a single YAML read to grab the
            // relationships array, which is fast enough for the
            // interactive `aida list` cadence.
            // trace:STORY-62 | ai:claude
            //
            // TASK-955: --recursive widens this to the WHOLE transitive subtree
            // (the parent + every descendant via parent->child edges, any depth)
            // instead of direct children. It reads the materialized hierarchy
            // edges with a single WITH RECURSIVE query over the cache — no
            // backend.load() — so the deep-epic view stays as cache-fast as the
            // direct-children view. clap's `requires = "parent"` guarantees we
            // only land here with a --parent value. trace:TASK-955 | ai:claude
            if let Some(parent_ref) = parent {
                let parent_req =
                    backend
                        .get_requirement_by_spec_id(parent_ref)?
                        .ok_or_else(|| {
                            anyhow::anyhow!("--parent {}: requirement not found", parent_ref)
                        })?;
                if *recursive {
                    // Full transitive subtree closure from the cache's edge graph.
                    // The set includes the root; the existing list filters
                    // (STATUS / --type / --tags) then apply to the whole subtree.
                    // trace:TASK-955 | ai:claude
                    let subtree_ids = backend.descendant_ids(&parent_req.id)?;
                    reqs.retain(|r| subtree_ids.contains(&r.id));
                } else {
                    let child_ids: HashSet<Uuid> = parent_req
                        .relationships
                        .iter()
                        .filter(|r| r.rel_type == RelationshipType::Parent)
                        .map(|r| r.target_id)
                        .collect();
                    reqs.retain(|r| child_ids.contains(&r.id));
                }
            }

            // STORY-706: a persistent focus scopes `aida list` to the focused
            // spec's transitive subtree — the SAME cache-fast closure
            // `--parent <id> --recursive` uses (`backend.descendant_ids`), with
            // the focus as the implicit parent. Skipped when an explicit
            // `--parent` already scopes the view, when `--all` widens to
            // everything (incl. archived/deferred), or via the focus-specific
            // `--no-focus` escape. The header below makes the scoping LOUD so a
            // focused subset is never mistaken for the whole project (the
            // kubectl-namespace footgun).
            let mut focus_banner: Option<String> = None;
            if parent.is_none() && !*all && !*no_focus {
                if let Some(focus_ref) = find_project_root()
                    .ok()
                    .and_then(|root| crate::focus::resolve_focus(&root))
                {
                    match backend.get_requirement_by_spec_id(&focus_ref)? {
                        Some(focus_req) => {
                            let total_pre_focus = reqs.len();
                            let subtree_ids = backend.descendant_ids(&focus_req.id)?;
                            reqs.retain(|r| subtree_ids.contains(&r.id));
                            focus_banner = Some(
                                crate::focus::focus_header(
                                    &focus_req.display_id(),
                                    reqs.len(),
                                    total_pre_focus,
                                )
                                .cyan()
                                .bold()
                                .to_string(),
                            );
                        }
                        None => {
                            // A focus pointing at a now-missing spec must not
                            // silently hide everything — warn and widen.
                            eprintln!(
                                "{} focus `{}` no longer resolves to a spec; showing all. \
                                 Run `aida focus clear` or re-set it.",
                                "Note:".yellow(),
                                focus_ref,
                            );
                        }
                    }
                }
            }

            // Hide META reqs (AI prompt customization seeded by init) from
            // the default view — they're plumbing, not user-authored work.
            // The user can still see them via `--type meta` (which forces
            // them into the result regardless of this filter) or by
            // passing `--include-meta`. trace:BUG-27 | ai:claude
            let user_asked_for_meta = r#type
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("meta"))
                .unwrap_or(false);
            if !*include_meta && !user_asked_for_meta {
                reqs.retain(|r| !r.req_type.eq_ignore_ascii_case("meta"));
            }

            // TASK-773: `aida list` / `aida list open` is a WORK view, so
            // perpetual standing-artifact types — vision, principle, term,
            // constraint, folder, meta — are NOT work and are hidden from the
            // default view (this stops e.g. VIS-1 reading as 'InProgress
            // work'). They still surface via an explicit `--type <T>` filter
            // (which overrides the exclusion) and via `--all`. This is
            // type-based filtering, orthogonal to STORY-584's deferred
            // view-flag. trace:TASK-773
            let user_asked_for_standing_type = r#type
                .as_deref()
                .map(is_standing_artifact_type)
                .unwrap_or(false);
            if !*all && !user_asked_for_standing_type {
                reqs.retain(|r| !is_standing_artifact_type(&r.req_type));
            }

            // BUG-1498: draft grooming is human-first. Auto-complete failure
            // records retain their provenance and remain directly reachable
            // (and batchable) through --machine-drafts or an explicit
            // --tags auto-drafted filter. Older records that predate the tag
            // are recognized by their stable description preamble. Apply
            // after parent/focus/type lenses so the hidden count is local to
            // exactly the view the advisor requested.
            let explicitly_asked_for_machine_tag =
                effective_tags.iter().any(|tag| tag == "auto-drafted");
            let machine_drafts_hidden = crate::apply_machine_draft_lens(
                &mut reqs,
                exact_draft_view,
                *machine_drafts,
                explicitly_asked_for_machine_tag,
            );

            // BUG-781: a decision spec (an ADR) sitting at `Approved` is
            // ACCEPTED — that class's TERMINAL state — so it belongs with the
            // closed rows the default open lens already hides, not in the open
            // backlog forever waiting on a manual archive. Applied only under
            // the same default open lens STORY-723 established: an explicit
            // `--status`, `--all`, `--archived`, `--deferred`, or a `--type
            // decision` ask all keep accepted ADRs visible. Sibling of the
            // TASK-773 standing-artifact pass above; the count feeds the footer
            // hint so the hidden rows stay one flag away. trace:BUG-781
            let user_asked_for_decision_type = r#type
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("decision"))
                .unwrap_or(false);
            // BUG-788: the explicit `open` shortcut sets a status filter, which
            // clears `default_open_lens` — but it is the SAME work view as bare
            // `aida list`, so it must hide accepted (terminal) ADRs identically.
            // Fold the two into one lens so the verbs agree; `--type decision`
            // and `--all` still keep accepted ADRs visible (both bypass this).
            // trace:BUG-788 | ai:claude
            let open_work_lens = crate::list_applies_open_work_lens(
                default_open_lens,
                explicit_open_alias,
                *all,
                *archived,
                *deferred,
            );
            let accepted_decisions_hidden = crate::hide_accepted_decisions(
                &mut reqs,
                open_work_lens,
                user_asked_for_decision_type,
            );

            // TASK-900: cap the result at the first N rows AFTER the sort
            // (list_summaries already ordered by --sort; default is
            // recency-first) and after the parent / meta / standing-type
            // retain passes — so `--limit N` is the N most-recent of the
            // fully-filtered set. Applied here, before the --short early
            // return and the json / table / tree rendering, so every output
            // shape sees the same bounded row set.
            // trace:TASK-900 | ai:claude
            //
            // TASK-970: in AGENT MODE a bare `aida list` (no explicit
            // `--limit`/`--all`) gets a DEFAULT row cap so an agent reading
            // ~900 rows as context doesn't blow its token budget. The cap is
            // scoped to the default human-readable TABLE render only: explicit
            // machine shapes (`--short` / `--json`) and the grouped `--tree`
            // view stay unbounded so existing agent/skill consumers that
            // enumerate every spec keep working, and the human TTY path is
            // untouched. An explicit `--limit`/`--all` always overrides.
            // trace:TASK-970
            let total_after_filters = reqs.len();
            let agent_default_cap = agent_list_default_cap(
                *limit,
                *all,
                *short,
                effective_json,
                *tree,
                agent_output_mode(),
            );
            if let Some(n) = limit.or(agent_default_cap) {
                reqs.truncate(n);
            }

            // TASK-743: --short emits one bare canonical spec ID per line —
            // no header, no count footer, no color, no routing glyphs — so
            // the output is directly pipeable into `$(...)` / xargs. Runs
            // AFTER the shared filter + parent + meta passes (same row set
            // the human table would show) and returns early, before any of
            // the routing-probe / table / tree / json rendering below.
            // trace:TASK-743 | ai:claude
            if *short {
                for r in &reqs {
                    let id = r.agreed_id.as_deref().or(r.spec_id.as_deref());
                    if let Some(id) = id {
                        println!("{id}");
                    }
                }
                return Ok(());
            }

            // TASK-670: compute the leading work-routing overlay (queued /
            // in-flight / blocked markers) once for the visible row set, then map
            // each row to a glyph. Two cheap reads are always on (a queues
            // dir scan + a live-lease probe); the blocked axis needs a graph
            // walk so it's gated behind --blocked. `--no-glyph` short-circuits
            // ALL of it (restores the pure-cache fast path); `--no-flow` keeps
            // the cheap probes off too since the column is dropped anyway.
            // trace:TASK-670 | ai:claude
            let show_flow = !*no_glyph && !*no_flow;
            // The routing sets feed BOTH the flow-glyph column AND the JSON
            // routing fields. The display-suppression flags (`--no-glyph` /
            // `--no-flow`) hide the COLUMN but must not blank the machine
            // fields, so compute the cheap probes whenever we render glyphs OR
            // emit JSON. trace:TASK-670 | ai:claude
            let need_routing = show_flow || effective_json;
            // project root = the parent of the `.aida-store` worktree.
            let routing_root = store_path.parent().map(|p| p.to_path_buf());
            let queued_ids: HashSet<Uuid> = if need_routing {
                routing_root
                    .as_deref()
                    .map(all_queued_requirement_ids)
                    .unwrap_or_default()
            } else {
                HashSet::new()
            };
            let in_flight_scopes: HashSet<String> = if need_routing {
                routing_root
                    .as_deref()
                    .map(in_flight_lease_scopes)
                    .unwrap_or_default()
            } else {
                HashSet::new()
            };
            // Blocked axis: only when --blocked. TASK-902: the blocked flag is
            // now cache-projected ("has an incomplete BlockedBy edge", computed
            // into the SQLite cache at write/rebuild time), so each summary
            // already carries `r.blocked` — no full backend.load() over every
            // object. This was the cockpit board's slowest leg (~1.4s); reading
            // the cache like --status/--archived cuts it to sub-second.
            // trace:TASK-902 | ai:claude
            // Per-row routing classifier. A row is in-flight when a live lease's
            // scope matches its canonical or origin id (case-insensitive); the
            // in-flight set is empty unless a live lease exists.
            let want_blocked = need_routing && *blocked;
            let row_routing = |r: &aida_core::RequirementSummary| -> (bool, bool, bool) {
                let queued = queued_ids.contains(&r.id);
                // Surface the cached blocked flag only when --blocked was passed
                // (parity with the prior overlay; off by default).
                let blocked = want_blocked && r.blocked;
                let in_flight = if in_flight_scopes.is_empty() {
                    false
                } else {
                    [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|id| in_flight_scopes.contains(&id.to_ascii_lowercase()))
                };
                (in_flight, blocked, queued)
            };

            // BUG-783: the archived + deferred view-tier nudges are OFF by
            // default — an explicit open-work request doesn't need reminding on
            // every invocation that the other two tiers exist. `[list]
            // show_hidden_hints = true` in `.aida/config.toml` opts back in.
            // The lens hints (closed / accepted decisions) are unaffected.
            // trace:BUG-783 | ai:claude
            let show_view_tier_hints = find_project_root()
                .ok()
                .map(|root| read_list_show_hidden_hints(&root))
                .unwrap_or(false);
            // The two tier counts are each an extra cache query. With the nudges
            // suppressed the only remaining consumer is the BUG-684 empty-state
            // signpost (which must not tell a filtered-empty project to file its
            // first spec), so on the hot non-empty path we skip both queries.
            let need_tier_counts = show_view_tier_hints || reqs.is_empty();

            // STORY-441: count of archived rows that would have surfaced if
            // `--all` was set. Cheap second query, used only to render the
            // "(N archived hidden — pass --all …)" footer hint. Skipped when
            // the user explicitly asked for archived or all rows.
            // trace:STORY-441 | ai:claude
            let archived_hidden_count = if need_tier_counts
                && matches!(archive, aida_core::ArchiveFilter::NonArchivedOnly)
            {
                let archived_filter = aida_core::ListFilter {
                    archive: aida_core::ArchiveFilter::ArchivedOnly,
                    ..filter.clone()
                };
                backend.list_summaries(&archived_filter)?.len()
            } else {
                0
            };

            // STORY-584: parallel count of deferred rows hidden from the default
            // view, used to render the "(N deferred hidden — pass --deferred …)"
            // footer nudge so the primed shelf stays discoverable.
            // trace:STORY-584 | ai:claude
            let deferred_hidden_count =
                if need_tier_counts && matches!(defer, aida_core::DeferFilter::NonDeferredOnly) {
                    let deferred_filter = aida_core::ListFilter {
                        defer: aida_core::DeferFilter::DeferredOnly,
                        ..filter.clone()
                    };
                    backend.list_summaries(&deferred_filter)?.len()
                } else {
                    0
                };

            // STORY-723: count of closed (done/completed/rejected) rows the
            // default OPEN lens is now hiding, so the discoverability footer can
            // tell the user the rest is one flag away. Only computed when the
            // default-open lens is actually active. trace:STORY-723 | ai:claude
            let closed_hidden_count = if default_open_lens {
                let closed_filter = aida_core::ListFilter {
                    status: Some(
                        aida_core::RequirementStatus::closed_statuses()
                            .iter()
                            .map(|s| s.cache_key())
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                    ..filter.clone()
                };
                backend.list_summaries(&closed_filter)?.len()
            } else {
                0
            };

            // Shared footer nudge for both view axes. Each prints a dimmed,
            // 2-space-indented line only when rows on that axis were hidden.
            // trace:STORY-441 trace:STORY-584 trace:STORY-723 | ai:claude
            let print_hidden_hints = || {
                for line in list_hidden_hint_lines(
                    show_view_tier_hints,
                    closed_hidden_count,
                    archived_hidden_count,
                    deferred_hidden_count,
                    accepted_decisions_hidden,
                ) {
                    println!("{}", line.dimmed());
                }
                if machine_drafts_hidden > 0 {
                    println!(
                        "{}",
                        format!(
                            "  ({machine_drafts_hidden} machine-filed drafts hidden — pass --machine-drafts to groom them)"
                        )
                        .dimmed()
                    );
                }
            };

            // BUG-684: a truly empty listing dead-ends the FIRST command a new
            // user runs — `No requirements found.` with no next step. Teach the
            // create move with a soft, forward-pointing signpost (matches the
            // empty `queue work` tone). Fires only when NOTHING is merely hidden
            // behind a filter (else the hidden-hints above already point at
            // `--all` — the specs exist, they're just filtered out).
            // trace:BUG-684 | ai:claude
            let print_empty_list_hint = || {
                if let Some(line) = empty_list_hint_line(
                    closed_hidden_count,
                    archived_hidden_count,
                    deferred_hidden_count,
                    accepted_decisions_hidden,
                ) {
                    println!(
                        "{} {}",
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                        line.dimmed()
                    );
                }
            };

            // BUG-1520: validate `--fields` BEFORE the format branch, so no
            // format can skip the check. Previously only the human and TOON
            // paths validated; `--format json` returned full rows with exit
            // code 0 for a bogus field, and that is the one surface machine
            // consumers read. One hoisted call makes the three agree by
            // construction rather than by three copies of it.
            // trace:BUG-1520 | ai:claude
            let selected_fields = toon_list_fields(fields.as_deref())?;

            // STORY-244: internal JSON output for the TUI launcher's
            // Backlog / History panes. Hidden from --help; schema is
            // internal and may change. Emits the row set straight as
            // `[{spec_id,title,req_type,status,tags}]` without the
            // human chrome (banner / footer count / colours).
            // trace:STORY-244 | ai:claude
            if effective_json {
                // TASK-670: carry the work-routing axis as machine fields
                // (glyphs are display-only). `blocked` reflects the graph walk
                // only when --blocked was passed; otherwise it's always false.
                // trace:TASK-670 | ai:claude
                #[derive(serde::Serialize)]
                struct ListJsonRow<'a> {
                    spec_id: &'a str,
                    title: &'a str,
                    req_type: &'a str,
                    // BUG-1520: `type` is what `--fields`, the human header and
                    // the TOON header all call this, so a consumer reading the
                    // documented key off a JSON row got null. Emitted ALONGSIDE
                    // `req_type`, never instead of it — STORY-1352 declared this
                    // a versioned read-only monitor contract, and a rename is
                    // the exact silent break that contract exists to prevent.
                    // Retiring `req_type` is a later deprecation, not this fix.
                    // trace:BUG-1520 | ai:claude
                    r#type: &'a str,
                    status: &'a str,
                    // STORY-1023: richer display label for stored
                    // NeedsAttention rows, e.g. "Shelved (ci-red)" or
                    // "Needs Decision (design-fork)". `status` stays the
                    // cache/status token for existing machine consumers.
                    #[serde(skip_serializing_if = "Option::is_none")]
                    status_label: Option<String>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    status_lens: Option<&'static str>,
                    tags: &'a [String],
                    queued: bool,
                    in_flight: bool,
                    blocked: bool,
                    // STORY-639: assignee carried as a machine field; omitted
                    // (None) when unassigned. trace:STORY-639 | ai:claude
                    #[serde(skip_serializing_if = "Option::is_none")]
                    assignee: Option<&'a str>,
                    // STORY-703: the revisit trigger of a deferred spec, so the
                    // cockpit's advisor-backlog panel can surface WHY each parked
                    // item is parked inline. Omitted (None) when not deferred.
                    // trace:STORY-703 | ai:claude
                    #[serde(skip_serializing_if = "Option::is_none")]
                    deferred_until: Option<&'a str>,
                    // TASK-1231: the groomed execution mode, so an unattended
                    // selector (`aida autoprogress`) can fence the supervised
                    // modes (drive/guided/operator/decide) it must never
                    // headless-drive. Omitted (None) when ungroomed/unset —
                    // unset stays drainable. trace:TASK-1231 | ai:claude
                    #[serde(skip_serializing_if = "Option::is_none")]
                    execution_mode: Option<&'a str>,
                }
                let out: Vec<ListJsonRow> = reqs
                    .iter()
                    .map(|r| {
                        let (in_flight, blocked, queued) = row_routing(r);
                        let parked_lens = if r.status.eq_ignore_ascii_case("NeedsAttention") {
                            backend
                                .get_requirement(&r.id)
                                .ok()
                                .flatten()
                                .and_then(|req| status_display::needs_attention_lens(&req))
                        } else {
                            None
                        };
                        // TASK-1456: a Done row folded in by
                        // `select_done_rework_rows` still carries `status:
                        // "Done"` — the machine-consumer contract that field
                        // is (STORY-1352) — so the annotation goes in the
                        // SAME status_label/status_lens channel NeedsAttention
                        // rows already use, not a mutated status. Perf: a
                        // plain `rework_ids` membership check — the set was
                        // already computed once above, no recomputation.
                        // trace:TASK-1456 | ai:claude
                        let rework = rework_ids.contains(&r.id);
                        let (status_label, status_lens) =
                            list_json_status_label_lens(rework, parked_lens.as_ref());
                        ListJsonRow {
                            spec_id: r
                                .agreed_id
                                .as_deref()
                                .or(r.spec_id.as_deref())
                                .unwrap_or(""),
                            title: r.title.as_str(),
                            req_type: r.req_type.as_str(),
                            r#type: r.req_type.as_str(),
                            status: r.status.as_str(),
                            status_label,
                            status_lens,
                            tags: &r.tags,
                            queued,
                            in_flight,
                            blocked,
                            assignee: r.assignee.as_deref(),
                            deferred_until: r.deferred_until.as_deref(),
                            execution_mode: r.execution_mode.as_deref(),
                        }
                    })
                    .collect();
                // BUG-1520 criterion 2: `--fields` NARROWS the JSON payload,
                // selecting the same field set TOON selects.
                //
                // Parity with TOON is about WHICH fields come back, not how
                // they are rendered. The narrowed row reuses the full row's own
                // values, so `--fields` can never silently change data — a
                // consumer that narrows gets a strict SUBSET of what it would
                // have got unnarrowed, with booleans still booleans and `tags`
                // still an array. Rendering through TOON's display tokens
                // instead would have lowercased `req_type` ("Task" -> "task")
                // only when narrowed, which is a sharper version of the very
                // defect this fixes.
                //
                // Fields TOON accepts that the JSON row has no key for
                // (priority, feature, owner, heft, weight, modified_at, origin)
                // fall back to the shared cell renderer, so a valid `--fields`
                // entry is never silently dropped.
                // trace:BUG-1520 | ai:claude
                if fields.is_some() {
                    let narrowed: Vec<serde_json::Value> = out
                        .iter()
                        .zip(reqs.iter())
                        .map(|(full_row, r)| {
                            let routing = row_routing(r);
                            let full =
                                serde_json::to_value(full_row).unwrap_or(serde_json::Value::Null);
                            let mut row = serde_json::Map::new();
                            for f in &selected_fields {
                                // `type` is carried under both spellings so a
                                // narrowed row's keys stay a subset of the full
                                // row's rather than a different vocabulary.
                                let keys: Vec<&str> = match f.as_str() {
                                    "id" => vec!["spec_id"],
                                    "type" => vec!["req_type", "type"],
                                    "mode" => vec!["execution_mode"],
                                    other => vec![other],
                                };
                                let mut hit = false;
                                for key in &keys {
                                    if let Some(v) = full.get(*key) {
                                        row.insert((*key).to_string(), v.clone());
                                        hit = true;
                                    }
                                }
                                if !hit {
                                    row.insert(
                                        f.clone(),
                                        serde_json::Value::String(toon_list_cell(r, routing, f)),
                                    );
                                }
                            }
                            serde_json::Value::Object(row)
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&narrowed)?);
                    return Ok(());
                }
                println!("{}", serde_json::to_string_pretty(&out)?);
                return Ok(());
            }

            // TASK-964: AGENT-MODE token-efficient TOON render. Replaces the
            // emoji/table default for non-interactive agent callers (non-TTY or
            // AIDA_AGENT_OUTPUT). The human TTY path drops through to the
            // byte-identical table below. The `--tree` grouped view keeps its own
            // shape (it's a structured rollup, not a flat row set). A leading
            // `count: N of M` header states the bounded slice; `--fields` widens
            // the minimal id/title/status/type schema. trace:TASK-964
            if agent_output_mode() && !*tree {
                // trace:BUG-1520 | ai:claude — one validation, hoisted above.
                let selected = selected_fields.clone();
                let rows: Vec<Vec<String>> = reqs
                    .iter()
                    .map(|r| {
                        let routing = row_routing(r);
                        selected
                            .iter()
                            .map(|f| toon_list_cell(r, routing, f))
                            .collect()
                    })
                    .collect();
                // STORY-723: label the denominator so it reconciles with the
                // other surfaces. The default `aida list` is now the OPEN lens,
                // so `M` counts open (active, non-archived) specs — distinct from
                // `aida status`'s `total` (every active spec, all statuses). An
                // explicit status filter relabels to plain "matched".
                // trace:STORY-723 | ai:claude
                let denom_label = list_count_denom_label(default_open_lens);
                println!(
                    "count: {} of {} {}",
                    reqs.len(),
                    total_after_filters,
                    denom_label
                );
                let field_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
                println!("{}", crate::toon::table_raw("specs", &field_refs, &rows));
                if agent_default_cap.is_some() && total_after_filters > reqs.len() {
                    println!("note: agent default cap — `aida list --all` or `--limit N` to widen");
                }
                // STORY-723: tell the agent the closed history exists but is
                // hidden behind the default open lens. trace:STORY-723
                if closed_hidden_count > 0 {
                    println!(
                        "note: {closed_hidden_count} closed hidden (open lens) — `aida list --all` or `--status closed`"
                    );
                }
                // BUG-781: same for the terminal (accepted) decisions the open
                // lens drops — an agent must not conclude they were deleted.
                if accepted_decisions_hidden > 0 {
                    println!(
                        "note: {accepted_decisions_hidden} accepted decisions hidden (terminal) — `aida list --type decision`"
                    );
                }
                if machine_drafts_hidden > 0 {
                    println!(
                        "note: {machine_drafts_hidden} machine-filed drafts hidden — `aida list --status draft --machine-drafts`"
                    );
                }
                // TASK-1456: the agent-mode row for a folded-in Done+refusal
                // spec renders `status: Done`, same as an ordinary
                // awaiting-merge row — flag it explicitly so an agent
                // consuming this table doesn't read it as merge-ready. Perf:
                // `rework_ids` membership only — the set was already computed
                // once above, no recomputation. trace:TASK-1456 | ai:claude
                for r in &reqs {
                    if !rework_ids.contains(&r.id) {
                        continue;
                    }
                    let display_id = r
                        .agreed_id
                        .as_deref()
                        .or(r.spec_id.as_deref())
                        .unwrap_or("?");
                    println!(
                        "note: {display_id} needs rework — reviewer requested changes; \
                         `aida queue rework {display_id}`"
                    );
                }
                // TASK-974 (AXI #9): trailing next-step block — drill into a row
                // (placeholder id, so no concrete spec id is echoed twice into
                // the id stream) and carry the active status filter forward into
                // the valid next transition for that state. trace:TASK-974
                let next = crate::help_next::list_next(&crate::help_next::ListContext {
                    status: status.as_deref(),
                    // BUG-684: an empty result routes the next[] block to `aida
                    // add` instead of the nonsensical `aida show <id>`.
                    is_empty: reqs.is_empty(),
                });
                if let Some(block) = crate::help_next::render(&next) {
                    println!("{block}");
                }
                return Ok(());
            }

            // STORY-706: print the loud focus header above the human output
            // (table or --tree). Placed after the `--short` / `--json` early
            // returns so those machine surfaces stay header-free.
            //
            if let Some(banner) = &focus_banner {
                println!("{banner}");
            }

            // TASK-970: in AGENT MODE, when the default cap trimmed the table,
            // lead with a `count: N of M` header + a widen hint so the agent
            // knows it's seeing a bounded slice and how to widen. Only fires on
            // the default table path (agent_default_cap is None for explicit
            // shapes and when nothing was trimmed). trace:TASK-970
            if agent_default_cap.is_some() && total_after_filters > reqs.len() {
                println!(
                    "count: {} of {} (agent default cap — `aida list --all` or `--limit N` to widen)",
                    reqs.len(),
                    total_after_filters
                );
            }

            // TASK-568: --tree groups the (already-filtered) listing by
            // parent EPIC for visual clustering, mirroring the shape of
            // `aida queue list --tree`. Children indent under their EPIC,
            // groups sort by item count desc then name, and requirements
            // with no EPIC parent fall into a final "Unscoped" group.
            // Summaries don't carry relationships (the cache is a flat
            // projection), so we load the full store once to resolve each
            // row's parent epic via derive_parent_epic_label — the same
            // opt-in YAML-read tradeoff `aida show --tree` accepts.
            // trace:TASK-568 | ai:claude
            if *tree {
                if reqs.is_empty() {
                    println!("No requirements found.");
                    print_empty_human_list_truncation_footer(
                        total_after_filters,
                        *limit,
                        default_open_lens,
                    );
                    print_hidden_hints();
                    print_empty_list_hint();
                    return Ok(());
                }
                let store = backend.load()?;
                let by_uuid: std::collections::HashMap<Uuid, &aida_core::Requirement> =
                    store.requirements.iter().map(|r| (r.id, r)).collect();

                use std::collections::BTreeMap;
                let unscoped_key = "~unscoped".to_string();
                let mut groups: BTreeMap<String, Vec<&aida_core::RequirementSummary>> =
                    BTreeMap::new();
                for summary in &reqs {
                    let key = by_uuid
                        .get(&summary.id)
                        .and_then(|req| derive_parent_epic_label(req, &store))
                        .unwrap_or_else(|| unscoped_key.clone());
                    groups.entry(key).or_default().push(summary);
                }

                // BUG-659: a requirement used as a real parent-backed group header
                // (its display id is a group key) must NOT also appear as an
                // ordinary row under Unscoped — it is already represented at the
                // parent/group level. Drop such rows from the Unscoped bucket, and
                // drop the bucket entirely if it empties. trace:BUG-659 | ai:claude
                {
                    let header_ids: std::collections::HashSet<String> = groups
                        .keys()
                        .filter(|k| k.as_str() != unscoped_key.as_str())
                        .cloned()
                        .collect();
                    if let Some(unscoped) = groups.get_mut(&unscoped_key) {
                        unscoped.retain(|s| {
                            let id = s
                                .agreed_id
                                .as_deref()
                                .or(s.spec_id.as_deref())
                                .unwrap_or("");
                            !header_ids.contains(id)
                        });
                    }
                    if groups.get(&unscoped_key).is_some_and(|v| v.is_empty()) {
                        groups.remove(&unscoped_key);
                    }
                }

                // Sort groups: real EPICs by count desc then name; unscoped last.
                let mut ordered: Vec<(String, Vec<&aida_core::RequirementSummary>)> =
                    groups.into_iter().collect();
                ordered.sort_by(|a, b| {
                    let a_unscoped = a.0 == unscoped_key;
                    let b_unscoped = b.0 == unscoped_key;
                    match (a_unscoped, b_unscoped) {
                        (true, false) => std::cmp::Ordering::Greater,
                        (false, true) => std::cmp::Ordering::Less,
                        _ => b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)),
                    }
                });

                // BUG: a real parent-backed group header used to print only the
                // parent's id + child count (`EPIC-56 (5 items)`), making a real
                // scoped parent indistinguishable from a synthetic bucket. Resolve
                // each parent label back to its requirement so the header can carry
                // the parent's own title + status badge; the child count becomes
                // trailing metadata. The label is the parent's canonical display id
                // (agreed_id else spec_id) — the same key derive_parent_epic_label
                // groups on — so look it up by that. trace:TASK-0439 | ai:claude
                let parent_by_label: std::collections::HashMap<&str, &aida_core::Requirement> =
                    store
                        .requirements
                        .iter()
                        .filter_map(|p| {
                            p.agreed_id
                                .as_deref()
                                .or(p.spec_id.as_deref())
                                .map(|id| (id, p))
                        })
                        .collect();
                for (key, group) in &ordered {
                    println!();
                    println!(
                        "{}",
                        tree_group_header(
                            key,
                            &unscoped_key,
                            group.len(),
                            parent_by_label.get(key.as_str()).copied(),
                            &store,
                        )
                    );
                    let id_col_width = group
                        .iter()
                        .map(|r| {
                            r.agreed_id
                                .as_deref()
                                .or(r.spec_id.as_deref())
                                .unwrap_or("???")
                                .len()
                        })
                        .max()
                        .unwrap_or(0);
                    for (i, summary) in group.iter().enumerate() {
                        let is_last = i + 1 == group.len();
                        let display_id = summary
                            .agreed_id
                            .as_deref()
                            .or(summary.spec_id.as_deref())
                            .unwrap_or("???");
                        // BUG-781: an accepted ADR badges as terminal `Accepted`,
                        // not the task-style `Approved`. trace:BUG-781
                        let status_badge =
                            status_display::status_badge(status_display::display_status_for_type(
                                &summary.req_type,
                                &summary.status,
                            ));
                        let glyph = if is_last { "└─" } else { "├─" };
                        let pad = " ".repeat(id_col_width.saturating_sub(display_id.len()));
                        let tag_set: std::collections::HashSet<String> =
                            summary.tags.iter().cloned().collect();
                        let tag_chip = format_tag_chip(&tag_set)
                            .map(|c| format!("  {}", format!("[{}]", c).dimmed()))
                            .unwrap_or_default();
                        println!(
                            "  {} {}{}  {}  [{}]{}",
                            glyph.dimmed(),
                            display_id.bold(),
                            pad,
                            summary.title,
                            status_badge,
                            tag_chip,
                        );
                    }
                }
                println!(
                    "\n{}",
                    list_human_count_footer(
                        reqs.len(),
                        total_after_filters,
                        *limit,
                        default_open_lens,
                    )
                );
                print_hidden_hints();
                print_deferred_triggers(*deferred, &reqs);
                print_rework_needed_notes(&reqs, &rework_ids);
                maybe_print_whats_left_tip(status.as_deref(), &reqs);
                return Ok(());
            }

            // STORY-734: `--fields <csv>` selects AND orders the columns of the
            // HUMAN table too (was agent-mode-only — the operator-reported gap).
            // Validate the field set (an unknown name errors with the valid set),
            // then render the dynamic column table and skip the fixed layouts
            // below. Default (no `--fields`) drops straight through, unchanged.
            // trace:STORY-734 | ai:claude
            if let Some(csv) = fields.as_deref() {
                let selected = toon_list_fields(Some(csv))?;
                if reqs.is_empty() {
                    println!("No requirements found.");
                    print_empty_human_list_truncation_footer(
                        total_after_filters,
                        *limit,
                        default_open_lens,
                    );
                    print_hidden_hints();
                    print_empty_list_hint();
                } else {
                    render_list_fields_table(&reqs, &selected, &row_routing);
                    println!(
                        "\n{}",
                        list_human_count_footer(
                            reqs.len(),
                            total_after_filters,
                            *limit,
                            default_open_lens,
                        )
                    );
                    print_hidden_hints();
                    print_deferred_triggers(*deferred, &reqs);
                    print_rework_needed_notes(&reqs, &rework_ids);
                    maybe_print_whats_left_tip(status.as_deref(), &reqs);
                }
                return Ok(());
            }

            if reqs.is_empty() {
                println!("No requirements found.");
                print_empty_human_list_truncation_footer(
                    total_after_filters,
                    *limit,
                    default_open_lens,
                );
                print_hidden_hints();
                print_empty_list_hint();
            } else {
                // Default rendering: one ID column (canonical = agreed_id
                // when present, else spec_id). Pass --show-origin to
                // surface the original spec_id alongside as "Origin ID".
                // Replaces the older two-column-by-default layout where
                // both columns were FR-NNN-shaped and confusing to grep
                // against. trace:FR-1-070 | ai:claude
                // TASK-569: --show-tags adds a "Tags" column right of
                // Title. Title is truncated to a fixed width so the tag
                // chips have room without breaking word-wrap on narrow
                // terminals; chip set itself is truncated to 3 with a
                // "+N more" suffix. trace:TASK-569 | ai:claude
                // TASK-670: the leading work-routing column. `flow_prefix` is
                // "<glyph> " (2 visible cols) per row when the column is shown,
                // else "". `flow_header` reserves the same 2 cols in the header
                // + divider so the ID column stays aligned. `render_status`
                // drops the status glyph under --no-glyph for plain-text output.
                // trace:TASK-670 | ai:claude
                let flow_header = if show_flow
                    && crate::glyphs::active_profile(crate::find_project_root().ok().as_deref())
                        == crate::glyphs::GlyphProfile::Ascii
                {
                    "   "
                } else if show_flow {
                    "  "
                } else {
                    ""
                };
                let options = ListTableOptions {
                    show_origin: *show_origin,
                    show_tags: *show_tags,
                    no_glyph: *no_glyph,
                    flow_header: flow_header.to_string(),
                    terminal_width: terminal_list_width(),
                };
                for line in render_list_table(&reqs, &options, row_routing) {
                    println!("{line}");
                }
                println!(
                    "\n{}",
                    list_human_count_footer(
                        reqs.len(),
                        total_after_filters,
                        *limit,
                        default_open_lens,
                    )
                );
                print_hidden_hints();
                print_deferred_triggers(*deferred, &reqs);
                print_rework_needed_notes(&reqs, &rework_ids);
                maybe_print_whats_left_tip(status.as_deref(), &reqs);
            }
        }
        // TASK-777: `aida fasttrack <title>` is a thin primitive that owns the
        // fasttrack-lane filing convention in ONE place — Approved + queued +
        // `batch:fasttrack` + `lifecycle:no-review` — so the /aida-fasttrack
        // skill delegates to it instead of hardcoding the tag string in
        // markdown (the same anti-drift DRY discipline as the CLI↔MCP mirror).
        // It rewrites to the equivalent `Command::Add` and recurses, reusing
        // the entire add+queue path. The lane skips human review only; CI still
        // gates merge (lifecycle:no-review, never no-ci-wait). trace:TASK-777
        //
        // TASK-1267: `--express` remains for one compatibility release, but it
        // now delegates to ordinary Approved + queued intake with mode=drain.
        // It creates no batch:express tag. Existing tagged specs remain
        // readable by the drain compatibility path in auto_complete.rs.
        Command::Fasttrack {
            title,
            r#type,
            express,
            command,
        } => {
            // TASK-905: the `status` subcommand is a read-only lane projection,
            // not a filing. Dispatch it before the Add delegation.
            // trace:TASK-905 | ai:claude
            if let Some(crate::cli::FasttrackCommand::Status { json }) = command {
                return handle_fasttrack_status(store_path, &backend, *json);
            }
            // Bare `aida fasttrack <title>` files a small change. With the
            // title now optional (so `status` parses as a subcommand), guard the
            // missing-title case with the same guidance `aida add` gives.
            let title = title.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "title is required — `aida fasttrack \"<one-line change>\"`. \
                     For the lane view, run `aida fasttrack status`."
                )
            })?;
            let (batch_bucket, lane_tags, mode) = fasttrack_lane_filing(*express);
            let add = Command::Add {
                title: Some(title.clone()),
                title_positional: None,
                description: None,
                description_from_file: None,
                description_stdin: false,
                status: Some("approved".to_string()),
                priority: None,
                r#type: Some(r#type.clone()),
                owner: None,
                feature: None,
                tags: lane_tags,
                prefix: None,
                parent: None,
                blocked_by: Vec::new(),
                force_parent: false,
                interactive: false,
                effort: None,
                human_only: false,
                no_human_only: false,
                queue: true,
                batch: batch_bucket,
                // BUG-528: route to the implementer queue by default (the
                // common target for filed work). trace:BUG-528 | ai:claude
                r#for: None,
                // trace:FR-283 | ai:claude
                weight: None,
                mode,
                gates: Vec::new(),
            };
            return handle_git_backend_command(store_path, &add);
        }
        Command::Add {
            title,
            title_positional,
            description,
            description_from_file,
            description_stdin,
            status,
            priority,
            r#type,
            owner,
            tags,
            prefix,
            parent,
            blocked_by,
            force_parent,
            interactive,
            effort,
            human_only,
            no_human_only,
            queue,
            batch,
            r#for,
            // trace:FR-283 | ai:claude
            weight,
            mode,
            gates,
            ..
        } => {
            // TASK-725: newcomer-friendly capture — `aida add "do X"`. The
            // positional title is equivalent to --title; --title wins if both
            // are given. Without this, the init greeting's own first suggestion
            // (`aida add "Add a task from the CLI"`) errors — caught by running
            // the novice's first session. trace:TASK-725 | ai:claude
            let title = title.clone().or_else(|| title_positional.clone());
            let title = &title;
            // BUG-45 + interactive expansion: when the user doesn't pass
            // --title, decide whether to prompt or bail. When prompting,
            // also walk through type / description / priority for any
            // field the user didn't already supply via flags. Title is
            // always required; the rest skip cleanly when their flag is
            // present. trace:BUG-45 | ai:claude
            let interactive_mode = *interactive
                || (title.is_none() && std::io::IsTerminal::is_terminal(&std::io::stdin()));

            // Title — required. Either --title, an interactive prompt, or
            // bail with help.
            let title_resolved: String = if let Some(t) = title.clone() {
                t
            } else if interactive_mode {
                let answer = inquire::Text::new("Title:")
                    .with_help_message("Required. One sentence describing what this is.")
                    .prompt()
                    .context("Title prompt cancelled")?;
                let t = answer.trim().to_string();
                if t.is_empty() {
                    anyhow::bail!("title is required (got empty input)");
                }
                t
            } else {
                anyhow::bail!(
                    "title is required — pass `--title \"...\"` or run with `--interactive` \
                     for prompts. (See `aida add --help` for the full flag list.)"
                );
            };
            // TASK-888: reject an empty / whitespace-only title — a titleless
            // spec is never useful, and the silent accept ("Added: TASK-N - ")
            // hid the mistake. Trim is display-only here; the title is stored
            // verbatim below. trace:TASK-888 | ai:claude
            if title_resolved.trim().is_empty() {
                anyhow::bail!("title cannot be empty — pass a non-blank `--title \"...\"`.");
            }
            // TASK-888: cap the title at a sane length. A 5000-char title was
            // accepted unbounded; truncate (preserving char boundaries) and warn
            // rather than hard-fail, so a paste accident still files something
            // legible. trace:TASK-888 | ai:claude
            const MAX_TITLE_LEN: usize = 200;
            let title_resolved: String = if title_resolved.chars().count() > MAX_TITLE_LEN {
                let truncated: String = title_resolved.chars().take(MAX_TITLE_LEN).collect();
                eprintln!(
                    "{} title exceeded {} characters — truncated. Put the detail in the description instead.",
                    "Warning:".yellow().bold(),
                    MAX_TITLE_LEN,
                );
                truncated
            } else {
                title_resolved
            };
            // trace:BUG-22 | ai:claude
            if let Some(msg) = suspicious_title_signal(&title_resolved) {
                eprintln!("{} {}", "Warning:".yellow().bold(), msg);
            }

            // Type — interactive picker when not provided. TASK-728: lead with
            // the relatable everyday types a newcomer reaches for (task is the
            // default + first); push the specialized docs-layer / organizational
            // types to the end so the picker doesn't open with jargon.
            // trace:TASK-728 | ai:claude
            let interactive_type: Option<String> = if r#type.is_none() && interactive_mode {
                let choices = vec![
                    "task",
                    "bug",
                    "story",
                    "epic",
                    "functional",
                    "spike",
                    "non-functional",
                    "system",
                    "user",
                    "principle",
                    "vision",
                    "decision",
                    "constraint",
                    "term",
                    "sprint",
                    "folder",
                    "meta",
                ];
                let pick = inquire::Select::new("Type:", choices)
                    .with_help_message("task for most things · bug for defects · story/epic to group work. The rest are for larger or docs-heavy projects.")
                    .prompt()
                    .context("Type prompt cancelled")?;
                Some(pick.to_string())
            } else {
                None
            };
            // TASK-725: a newcomer who types `aida add "do X"` means a *task*,
            // not a "Functional Requirement" — default to the relatable,
            // catch-all type when none is given (explicit --type and the
            // interactive picker still win). trace:TASK-725 | ai:claude
            let effective_type: Option<String> = r#type
                .clone()
                .or(interactive_type)
                .or_else(|| Some("task".to_string()));

            // Description — open the user's $EDITOR when not provided.
            // trace:BUG-17 | ai:claude
            let resolved_description = if interactive_mode
                && description.is_none()
                && description_from_file.is_none()
                && !*description_stdin
            {
                let body = inquire::Editor::new("Description")
                    .with_help_message(
                        "Multi-line. Save + close the editor to continue. Leave empty to skip.",
                    )
                    .prompt()
                    .context("Description prompt cancelled")?;
                if body.trim().is_empty() {
                    None
                } else {
                    Some(body)
                }
            } else {
                resolve_description(description, description_from_file, *description_stdin)?
            };

            // Priority — picker when not provided.
            let interactive_priority: Option<String> = if priority.is_none() && interactive_mode {
                let pick = inquire::Select::new("Priority:", vec!["medium", "high", "low"])
                    .with_help_message(
                        "medium covers most things; high for blockers; low for nice-to-haves.",
                    )
                    .prompt()
                    .context("Priority prompt cancelled")?;
                Some(pick.to_string())
            } else {
                None
            };
            let effective_priority: Option<String> = priority.clone().or(interactive_priority);

            // STORY-1427: `--gates` runs named library gates over the draft text
            // before filing. Advisory — prints verdicts, never blocks; absent the
            // flag this is a no-op. trace:STORY-1427 | ai:claude
            crate::gate_cmd::run_intake_gates(
                gates,
                &title_resolved,
                resolved_description.as_deref().unwrap_or_default(),
            )?;

            let mut req =
                Requirement::new(title_resolved, resolved_description.unwrap_or_default());
            if let Some(s) = status {
                let canonical = validate_status_input(s).map_err(|e| anyhow::anyhow!(e))?;
                // STORY-332: a freshly-added spec cannot be born paused —
                // NeedsAttention is reached only by punting In-Progress work.
                if let Some(msg) =
                    forbidden_attention_transition(&req.status, &parse_status(canonical)?)
                {
                    anyhow::bail!(msg);
                }
                req.set_status_from_str(canonical);
            }
            // TASK-647 (ADR-3): advisor-gate the production of approved+ specs.
            // A non-advisor, non-TTY caller (headless agent, drain/auto
            // capture) can only file `draft`; a requested approved+ status is
            // downgraded with a one-line triage notice. An interactive human
            // (TTY) or the advisor role is unaffected. trace:TASK-647 | ai:claude
            let intake_downgraded_from =
                if status_requires_advisor_authority(&req.status) && !has_advisor_authority() {
                    let from = req.status;
                    req.status = RequirementStatus::Draft;
                    Some(from)
                } else {
                    // BUG-498: a non-downgraded approved+ intake exercised
                    // advisor authority — nudge the operator to seat the
                    // advisor role if it came from an env prefix.
                    if status_requires_advisor_authority(&req.status) {
                        maybe_hint_advisor_seat();
                    }
                    None
                };
            if let Some(p) = &effective_priority {
                let canonical = validate_priority_input(p).map_err(|e| anyhow::anyhow!(e))?;
                req.set_priority_from_str(canonical);
            }
            if let Some(t) = &effective_type {
                // BUG-48: surface the error instead of dropping silently.
                let rt = parse_requirement_type(t).map_err(|e| {
                    anyhow::anyhow!(
                        "{} — expected one of: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc",
                        e
                    )
                })?;
                // BUG-49: when the new type implies a different prefix
                // than the existing spec_id, the spec_id does NOT auto-
                // renumber (it would orphan trace comments, commit
                // messages, and external references). Warn the user so
                // they don't expect FR-4 to become PRIN-4.
                if let Some(ref sid) = req.spec_id {
                    let new_prefix = rt.default_prefix();
                    let current_prefix = sid.split('-').next().unwrap_or("");
                    if !current_prefix.is_empty() && current_prefix != new_prefix {
                        eprintln!(
                            "{} type changed to `{}` but spec_id stays `{}` — \
                             AIDA spec_ids are stable so existing trace comments \
                             and commit refs keep working. To get a `{}-N` id, \
                             recreate the requirement with the new type.",
                            "Note:".dimmed(),
                            t,
                            sid,
                            new_prefix
                        );
                    }
                }
                req.req_type = rt;
            }
            // TASK-130: a Spike is time-boxed research with operator-judgment
            // decisions (candidate selection, experimental design, rubric
            // evaluation, report writing) — not heads-down implementation. So
            // a Spike is born human-only by default (the orchestrator's
            // pre-pickup gate then skips it instead of an implementer agent
            // mis-treating research as coding). `--no-human-only` opts a Spike
            // back into auto-pickup; `--human-only` sets the marker on any
            // type. Reuses the STORY-333 human_only primitive.
            // trace:TASK-130 | ai:claude
            req.human_only = resolve_human_only(&req.req_type, *human_only, *no_human_only);
            if let Some(o) = owner {
                req.owner = o.clone();
            }
            if let Some(t) = tags {
                for tag in t.split(',') {
                    req.tags.insert(tag.trim().to_string());
                }
            }
            if let Some(effort) = effort {
                effort_calibration::apply_effort_tag(
                    &mut req.tags,
                    effort_calibration::EffortTouchpoint::Open,
                    *effort,
                );
            }
            // FR-283: the optional first-class numeric weight/score — a
            // continuous ranking signal beyond the priority enum.
            // trace:FR-283 | ai:claude
            if let Some(w) = weight {
                if !w.is_finite() {
                    anyhow::bail!("--weight must be a finite number (got {w})");
                }
                req.weight = Some(*w);
            }
            // A creation-time execution mode is the same advisor routing
            // judgment as `aida edit --mode`; validate it before persisting.
            // trace:TASK-1267 | ai:codex
            if let Some(m) = mode {
                if !has_advisor_authority() {
                    anyhow::bail!(
                        "--mode sets the advisor's routing classification and needs advisor authority"
                    );
                }
                req.execution_mode = Some(
                    m.parse::<aida_core::ExecutionMode>()
                        .map_err(|e: String| anyhow::anyhow!(e))?,
                );
            }
            if let Some(p) = prefix {
                req.prefix_override = Some(p.to_uppercase());
            }

            // Try to assign an agreed ID from a pre-allocated block (FR-2-005).
            // Behavior is governed by [id_format] policy in .aida/config.toml:
            //   - node-aware-only:      skip block dispense entirely
            //   - blocks-then-fallback: try block; fall through silently if missing
            //   - blocks-only:          require a block; error if missing
            // trace:EPIC-1-052 Phase 2 | ai:claude

            // TASK-59: pre-flight validation BEFORE the dispenser
            // advances. The original flow allocated an agreed_id from
            // the block, then ran parent / terminal-status / lease
            // checks — when any of those bailed the counter was
            // already advanced and committed to blocks.yaml, leaving
            // a phantom id (BUG-69 etc.) permanently missing from the
            // sequence. Resolve parent + run all three guards FIRST
            // so a failed `aida add --parent <Completed-EPIC>` leaves
            // the dispenser exactly where it was.
            // trace:TASK-59 | ai:claude
            let parent_req: Option<aida_core::models::Requirement> =
                if let Some(parent_str) = parent {
                    let resolved = if let Ok(uuid) = uuid::Uuid::parse_str(parent_str) {
                        backend.get_requirement(&uuid)?
                    } else {
                        backend.get_requirement_unambiguous(parent_str)?
                    };
                    Some(resolved.ok_or_else(|| {
                        anyhow::anyhow!(
                            "parent `{}` not found — refusing to create child requirement \
                         without a valid parent (no child file written)",
                            parent_str
                        )
                    })?)
                } else {
                    None
                };
            if let Some(ref pr) = parent_req {
                if !*force_parent && is_terminal_status(&pr.status) {
                    anyhow::bail!(
                        "parent {} is {} — adding new children to a closed parent is usually a mistake. \
                         Pass `--force-parent` to override, or pick a different parent \
                         (try `aida list --type epic --status approved`).",
                        pr.spec_id.as_deref().unwrap_or("?"),
                        pr.status,
                    );
                }
                if let Ok(project_root) = find_project_root() {
                    if !list_leases(&project_root).is_empty() {
                        let store = backend.load()?;
                        enforce_session_lease(
                            &project_root,
                            pr,
                            &store,
                            "aida add --parent",
                            false,
                            // BUG-637: `aida add --parent` has no --force override; never skip.
                            false,
                        )?;
                    }
                }
            }

            let project_dir = std::env::current_dir().unwrap_or_default();
            // BUG-372: before allocating a human-readable global SPEC-ID,
            // refresh the git-canonical store and refuse known duplicate
            // spec_ids. TASK-281 protects block refill commits, but the
            // creation path also has to start from fresh store state.
            pull_store_before_id_allocation(store_path, &project_dir)?;
            let id_policy = read_id_format_policy(&project_dir);
            if id_policy.uses_blocks() {
                let node_id = load_node_id(store_path);
                let blocks_path = store_path.join("registry").join("blocks.yaml");
                if !blocks_path.exists() && id_policy.requires_block() {
                    anyhow::bail!(
                        "id_format policy is `blocks-only` but no blocks.yaml exists. \
                         Run `aida db block claim --type FR --size 100` to allocate a block."
                    );
                }
                if blocks_path.exists() {
                    // Determine type prefix. Use the canonical
                    // RequirementType::default_prefix() so every type is
                    // covered (no silent fallback to "FR" for new types).
                    // trace:FR-1-074 | ai:claude
                    let type_prefix = match &req.prefix_override {
                        Some(p) => p.clone(),
                        None => req.req_type.default_prefix().to_string(),
                    };

                    // TASK-281: auto-claim a fresh block before dispensing when
                    // aggregate remaining drops below the configured threshold.
                    // Skips when auto-claim is disabled, when there's no active
                    // block yet (bootstrap is owned by `aida node acquire`), and
                    // when we're still above threshold. BUG-372 makes a
                    // refill push failure fatal for add: continuing with a
                    // stale local block can recreate the cross-clone
                    // duplicate SPEC-ID race.
                    // trace:TASK-281 BUG-372 | ai:claude
                    match ensure_block_capacity(store_path, &project_dir, &node_id, &type_prefix) {
                        Ok(Some(outcome)) => {
                            eprintln!(
                                "{} Auto-claimed {} (threshold crossed: {} remaining → {})",
                                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                                outcome.label,
                                outcome.previous_remaining,
                                outcome.new_remaining,
                            );
                        }
                        Ok(None) => {}
                        Err(e) => {
                            anyhow::bail!(
                                "auto-claim failed before dispensing a new {} id: {}\n\
                                 Refusing to continue with a potentially stale local block. \
                                 Run `aida db sync --pull` and retry.",
                                type_prefix,
                                e
                            );
                        }
                    }

                    // BUG-474: the agreed-id dispense below is a
                    // read-modify-write (load → dispense → save). Two
                    // concurrent `aida add` runs would both load `next = N`,
                    // both dispense `<TYPE>-N`, and both save → a duplicate
                    // stable id. Serialize the whole sequence under an
                    // exclusive advisory lock on the blocks file, mirroring
                    // the FileDispenser pattern (TASK-331). The in-loop
                    // collision-skip stays; the lock is the real fix.
                    // trace:BUG-474 | ai:claude
                    aida_core::BlockRegistry::with_dispense_lock(&blocks_path, || {
                        if let Ok(mut registry) = aida_core::BlockRegistry::load(&blocks_path) {
                            match registry.find_active_block_or_global(&node_id, &type_prefix) {
                                None => {
                                    // No block for this type. Under `blocks-only`
                                    // this is fatal — the project requires every
                                    // id come from an allocated block. Otherwise
                                    // fall through to a node-aware id silently.
                                    if id_policy.requires_block() {
                                        anyhow::bail!(
                                            "id_format policy is `blocks-only` but node {} has no \
                                         {} block. Run `aida db block claim --type {} --size 100` \
                                         to allocate one (requires network).",
                                            node_id,
                                            type_prefix,
                                            type_prefix
                                        );
                                    }
                                    // TASK-36: if the only thing missing is an
                                    // *active* block (exhausted blocks exist for
                                    // this type), the user just rolled off the
                                    // last short id and we're silently switching
                                    // to the node-aware format. Surface that with
                                    // the highest issued short id and a pointer
                                    // to merge-gate. trace:TASK-36 | ai:claude
                                    let prefix_upper = type_prefix.to_uppercase();
                                    let last_short = registry
                                        .blocks
                                        .iter()
                                        .filter(|b| b.type_prefix.to_uppercase() == prefix_upper)
                                        .map(|b| b.range_end)
                                        .max();
                                    if let Some(last) = last_short {
                                        eprintln!(
                                        "{} {} agreed-id block exhausted (last short id: {}-{}).",
                                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                                        prefix_upper,
                                        prefix_upper,
                                        last
                                    );
                                        eprintln!(
                                        "  {} Falling back to node-aware ids ({}-NODE-NN). Run \
                                         `aida db block claim --type {} --size 100` to allocate \
                                         a fresh block.",
                                        "→".dimmed(),
                                        prefix_upper,
                                        prefix_upper
                                    );
                                    }
                                }
                                Some(idx) if registry.blocks[idx].is_exhausted() => {
                                    anyhow::bail!(
                                    "Agreed ID block for {} exhausted on node {}. \
                                     Run `aida db block claim --type {}` to allocate a new block (requires network).",
                                    type_prefix, node_id, type_prefix
                                );
                                }
                                Some(_) => {
                                    // BUG-31: dispense in a loop, skipping any
                                    // candidate id that's already taken in the
                                    // store. Existing reqs (e.g. older sessions
                                    // before this block was claimed, retired-
                                    // legacy-id migrations, imports) can occupy
                                    // ids inside our block range. Block.next is
                                    // monotonic so we just keep advancing until
                                    // we land on a free slot or exhaust.
                                    let mut dispensed: Option<(String, bool)> = None;
                                    let mut skipped: Vec<String> = Vec::new();
                                    while let Some((candidate, is_low)) =
                                        registry.dispense(&node_id, &type_prefix)
                                    {
                                        let taken = backend
                                            .get_requirement_by_spec_id(&candidate)
                                            .map(|opt| opt.is_some())
                                            .unwrap_or(false);
                                        if !taken {
                                            dispensed = Some((candidate, is_low));
                                            break;
                                        }
                                        skipped.push(candidate);
                                    }
                                    if !skipped.is_empty() {
                                        eprintln!(
                                            "{} skipped {} already-taken id(s) in {} block: {}",
                                            "Note:".dimmed(),
                                            skipped.len(),
                                            type_prefix,
                                            skipped.join(", ")
                                        );
                                    }
                                    if let Some((agreed_id, _is_low_per_block)) = dispensed {
                                        // BUG-115: warn on the aggregate remaining
                                        // across all of this node's non-exhausted
                                        // blocks for the type. The pre-fix per-
                                        // block check fired on every `aida add`
                                        // when the lowest-numbered block was near
                                        // empty, even though a higher block with
                                        // full capacity had already been claimed.
                                        // trace:BUG-115 | ai:claude
                                        if registry.aggregate_is_low(&node_id, &type_prefix) {
                                            let aggregate = registry
                                                .aggregate_remaining(&node_id, &type_prefix);
                                            let active =
                                                registry.active_block_count(&node_id, &type_prefix);
                                            let span = if active == 1 {
                                                "in 1 block".to_string()
                                            } else {
                                                format!("across {} blocks", active)
                                            };
                                            eprintln!(
                                            "{} {} block running low ({} remaining {}). Run `aida db block claim --type {}` soon.",
                                            "WARNING:".yellow().bold(),
                                            type_prefix,
                                            aggregate,
                                            span,
                                            type_prefix
                                        );
                                        }
                                        // Persist the updated next pointer
                                        if let Err(e) = registry.save(&blocks_path) {
                                            eprintln!("Warning: could not save blocks.yaml: {}", e);
                                        } else {
                                            // Commit the pointer advance to the store
                                            let _ = aida_core::git_ops::add(
                                                store_path,
                                                &["registry/blocks.yaml"],
                                            );
                                        }
                                        req.agreed_id = Some(agreed_id.clone());
                                        // Use the agreed ID as the spec_id so it is immediately
                                        // visible as the primary identifier.
                                        req.spec_id = Some(agreed_id);
                                    }
                                }
                            }
                        }
                        Ok(())
                    })?;
                }
            }

            // (parent validation moved BEFORE the dispenser allocation
            // above for TASK-59 — preserves BUG-62, BUG-64, STORY-48
            // guards while preventing phantom-id leaks.)

            // TASK-856: file the new spec through the single-row write path
            // rather than `update_atomically`. The old path did a FULL-STORE
            // save on every add — `git add objects` + commit over the entire
            // object tree (~2.3k files in this repo), a serialize-read-compare
            // of every requirement, AND a full cache rebuild (DELETE + reinsert
            // every cache row) — making `aida add` scale O(store size) when the
            // work is O(1): exactly one new object. `backend.add_requirement`
            // assigns the spec_id with the SAME store-configured strategy
            // (`add_requirement_with_id`, via GitBackend::add_requirement),
            // writes ONLY the new object, does a TARGETED commit (new YAML +
            // metadata.yaml), and an incremental cache upsert — git-canonical
            // write + cache write-through stay correct. Measured: full-scale
            // store add dropped from ~1.5s to ~0.4s for this step (≈1.8x
            // faster end-to-end median at full scale). This also removes the
            // redundant second `write_object` (GitBackend::add_requirement
            // already writes it). trace:TASK-856 | ai:claude
            let added = backend.add_requirement(req.clone())?;

            {
                let last = &added;
                println!(
                    "Added: {} - {}",
                    last.spec_id.as_deref().unwrap_or("?"),
                    last.title
                );
                // TASK-647 (ADR-3): note that a requested approved+ status was
                // held for advisor triage. The spec is filed (as draft); this
                // is informational, not an error. trace:TASK-647 | ai:claude
                if let Some(from) = &intake_downgraded_from {
                    eprintln!(
                        "{} filed as {} (requested {} needs advisor authority) — queued for advisor triage.",
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                        "draft".yellow(),
                        from.to_string().to_lowercase().dimmed()
                    );
                }
                if let Some(sid) = last.spec_id.as_deref() {
                    record_role_activity(sid, "add");
                }

                // STORY-446: apply any --blocked-by / --depends-on edges now
                // that the spec exists, atomically with the inverse Blocks
                // edge so the pickability gate holds pickup until the blocker
                // is Completed. trace:STORY-446 | ai:claude
                if !blocked_by.is_empty() {
                    if let Some(sid) = last.spec_id.as_deref() {
                        for blocker in blocked_by {
                            match add_blocked_by_edge(&backend, sid, blocker) {
                                Ok(disp) => println!("  Blocked by: {}", disp.cyan()),
                                Err(e) => eprintln!(
                                    "  {} could not add blocked-by {}: {}",
                                    "Warning:".yellow().bold(),
                                    blocker,
                                    e
                                ),
                            }
                        }
                    }
                }

                // BUG-58: when adding from inside a session worktree
                // WITHOUT --parent, hint that the session's scope is
                // probably the right parent. Surfaced after the add line
                // (rather than rejecting the call) so muscle-memory
                // workflows don't break — the user can ignore it for
                // off-topic reqs. Only fires when:
                //   - the lease scope looks like a spec id (so we can
                //     suggest a concrete `--parent`)
                //   - the parent actually exists in the store
                //   - the new req isn't already that scope's spec
                // trace:BUG-58 | ai:claude
                if parent.is_none() {
                    if let (Ok(project_root), Ok(cwd)) =
                        (find_project_root(), std::env::current_dir())
                    {
                        // BUG-416: the "this session owns scope X" hint is only
                        // trustworthy when the current process is the SOLE live
                        // agent in this worktree. When two `aida agent new`
                        // sessions share a worktree, active_lease_for_cwd may
                        // resolve a PEER agent's lease, so the hint would
                        // misattribute its scope (the quizdom bleed). Stay
                        // silent then. A human session (0 registered agents) or
                        // a lone agent (1) hints exactly as before.
                        // trace:BUG-416 | ai:claude
                        let shared_worktree =
                            agent_registry::live_agents_covering_cwd(&project_root, &cwd) > 1;
                        if let Some(lease) =
                            active_lease_for_cwd(&project_root, &cwd).filter(|_| !shared_worktree)
                        {
                            let scope = lease.scope.clone();
                            // Cheap heuristic for "looks like a spec id":
                            // PREFIX-N format. Free-form scopes like
                            // `feature:auth` or path globs don't match
                            // and we stay quiet.
                            if looks_like_spec_id(&scope)
                                && backend
                                    .get_requirement_by_spec_id(&scope)
                                    .ok()
                                    .flatten()
                                    .is_some()
                                && last.spec_id.as_deref() != Some(scope.as_str())
                            {
                                eprintln!(
                                    "{} this session owns scope {}. \
                                     If {} should be a child of it, link it now: \
                                     `aida rel add --from {} --to {} --type child --bidirectional` \
                                     (or pass `--parent {}` next time).",
                                    "Hint:".dimmed(),
                                    scope.cyan(),
                                    last.spec_id.as_deref().unwrap_or("?").cyan(),
                                    last.spec_id.as_deref().unwrap_or("?"),
                                    scope,
                                    scope,
                                );
                            }
                        }
                    }
                }

                // FR-215: --parent <id> establishes a parent relationship
                // in the same shot. Parent was pre-resolved + lease-checked
                // above (BUG-62) so by this point we know the link is
                // valid; here we just append the bidirectional edges.
                // trace:FR-215, BUG-62 | ai:claude
                if let Some(parent_req) = parent_req {
                    use aida_core::models::{Relationship, RelationshipType};
                    // BUG-615: re-LOAD the freshly-saved child rather than
                    // reusing the `last` snapshot captured at add_requirement
                    // time. The --blocked-by loop above may have already
                    // persisted a BlockedBy edge via add_blocked_by_edge; the
                    // stale `last.clone()` would not carry that edge, so saving
                    // it here clobbered the just-written blocked-by data. Read
                    // the current state (which includes any blocked-by edges),
                    // append the Child edge to THAT, and save once — so both
                    // --parent and --blocked-by survive when combined.
                    // trace:BUG-615 | ai:claude
                    let mut child = last
                        .spec_id
                        .as_deref()
                        .and_then(|sid| backend.get_requirement_by_spec_id(sid).ok().flatten())
                        .unwrap_or_else(|| last.clone());
                    let parent_uuid = parent_req.id;
                    let child_uuid = child.id;
                    // RelationshipType is "I am X to target", so on the
                    // new (child) req we record `Child` pointing at the
                    // parent's uuid, and on the parent we record `Parent`
                    // pointing at the child's uuid. trace:FR-215
                    let now = chrono::Utc::now();
                    child.relationships.push(Relationship {
                        target_id: parent_uuid,
                        rel_type: RelationshipType::Child,
                        created_at: Some(now),
                        created_by: None,
                    });
                    backend.update_requirement(&child)?;
                    let mut parent_mut = parent_req.clone();
                    parent_mut.relationships.push(Relationship {
                        target_id: child_uuid,
                        rel_type: RelationshipType::Parent,
                        created_at: Some(now),
                        created_by: None,
                    });
                    backend.update_requirement(&parent_mut)?;
                    println!(
                        "  Linked: {} → parent of {}",
                        parent_req.spec_id.as_deref().unwrap_or("?"),
                        last.spec_id.as_deref().unwrap_or("?")
                    );
                }

                // TASK-928 (SPIKE-71): when no explicit `--parent` was given,
                // a `parent:<SPEC-ID>` tag must still materialize the real
                // bidirectional edge, or the spec is orphaned from the graph
                // (`aida graph tree` / TUI focus-lens blind). Additive +
                // lenient: the tag stays, an unresolvable target is a no-op.
                // trace:TASK-928 | ai:claude
                if parent.is_none() {
                    if let Some(sid) = last.spec_id.as_deref() {
                        match ensure_parent_edge_from_tag(&backend, sid) {
                            Ok(Some(pdisp)) => {
                                println!(
                                    "  Linked: {} → parent of {} (from parent: tag)",
                                    pdisp, sid
                                )
                            }
                            Ok(None) => {}
                            Err(e) => eprintln!(
                                "  {} could not link parent from tag: {}",
                                "Warning:".yellow().bold(),
                                e
                            ),
                        }
                    }
                }
                if let Some(spec_id) = last.spec_id.as_deref() {
                    let main_project_dir = main_worktree_root_from(&project_dir);
                    if let Err(e) = effort_calibration::upsert_open(
                        &main_project_dir,
                        spec_id,
                        *effort,
                        Some(current_user_id(None)),
                    ) {
                        eprintln!(
                            "  {} could not record open effort for {spec_id}: {e}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                        );
                    }
                    // BUG-372: make newly allocated SPEC-IDs visible to the
                    // remote orphan store immediately when online, so a
                    // sibling clone that files next pulls the allocation
                    // before dispensing from its own block.
                    push_store_after_id_allocation(store_path, &project_dir, spec_id)?;
                }

                // TASK-754: `--queue` files + approves + enqueues in one shot,
                // collapsing the advisor's "file as approved, then groom it"
                // two-step. It reuses the exact `aida backlog groom` enqueue
                // helper (user resolution, AIDA_SESSION_ROLE routing, note,
                // optional batch tag) so the surfaces stay consistent. Two
                // gates are honored, not invented:
                //   * AC5 — advisor authority: the intake above already
                //     downgrades a requested Approved to Draft when the session
                //     lacks advisor authority (`intake_downgraded_from`), so a
                //     non-advisor/non-TTY `--queue` lands a draft and is then
                //     refused by the Approved check below — the same hard gate
                //     `aida queue add` enforces, with no bypass.
                //   * AC2 — only Approved is enqueueable, matching
                //     `aida backlog groom`'s policy; a draft/other status is
                //     refused with guidance rather than silently queued.
                // trace:TASK-754 | ai:claude
                if *queue {
                    if let Some(reason) = queue_at_filing_refusal(
                        &last.status,
                        intake_downgraded_from.is_some(),
                        r#for.as_deref(),
                    ) {
                        let did = last.spec_id.as_deref().unwrap_or("?");
                        match reason {
                            QueueAtFilingRefusal::Downgraded => anyhow::bail!(
                                "{} was filed but NOT queued: queuing work for execution needs \
                                 advisor authority (advisor role or an interactive session). \
                                 The spec landed as a draft for advisor triage. Re-run as the \
                                 advisor, or have the advisor approve + groom it.",
                                did
                            ),
                            QueueAtFilingRefusal::NotApproved => anyhow::bail!(
                                "{} is `{}`, not Approved — `--queue` only enqueues Approved work \
                                 (same policy as `aida backlog groom`). Pass `--status approved` \
                                 when you mean to file-approve-and-queue in one shot.",
                                did,
                                last.status
                            ),
                        }
                    }
                    let storage = Storage::new(store_path);
                    let user_id = current_user_id(None);
                    let batch_norm: Option<&str> = batch.as_deref().map(normalize_batch_name);
                    // BUG-528: route the freshly-filed work explicitly rather
                    // than to the FILER's session role. The common case is an
                    // advisor filing implementation work, so `add --queue`
                    // defaults to the `implementer` queue (the overwhelmingly
                    // common target) — overridable with `--for <role>`, and
                    // `--for any` leaves it unrouted. This mirrors the routing
                    // surface of `aida queue add --for`; without it the
                    // advisor-filed bug/task silently landed in the ADVISOR
                    // queue and `burndown run` (which drains the implementer
                    // queue) would miss it. trace:BUG-528 | ai:claude
                    let route_role: Option<String> = add_queue_route_role(r#for.as_deref());
                    backlog::enqueue_groomed_for(
                        &storage,
                        last,
                        batch_norm,
                        Some("advisor-cleared at filing"),
                        &user_id,
                        route_role.clone(),
                    )?;
                    let did = last.spec_id.as_deref().unwrap_or("?");
                    let route_suffix = match route_role.as_deref() {
                        Some(role) => format!(" → {} queue", role),
                        None => " (unrouted)".to_string(),
                    };
                    if let Some(name) = batch_norm {
                        println!(
                            "{} Queued {}{} (advisor-cleared at filing), tagged `batch:{}`.",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            did.bold(),
                            route_suffix,
                            name.bold()
                        );
                    } else {
                        println!(
                            "{} Queued {}{} (advisor-cleared at filing).",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            did.bold(),
                            route_suffix
                        );
                    }
                }
                // TASK-974 (AXI #9): the lifecycle-aware next-step block — the
                // valid next transition(s) for the freshly-filed spec's status (a
                // draft becomes approve/reject), templated with its id.
                //
                // STORY-737 (delight #2): a brand-new user files spec #1 on the
                // HUMAN TTY and needs this nudge MOST, yet it used to fire only
                // under `agent_output_mode()` — the agent got guided, the human
                // got a bare "Added: …". Render the human `Next:` footer (the same
                // idiom `aida show` uses) plus a trace-link breadcrumb so the
                // newcomer learns the one move that wires their code to the spec.
                // Agent mode keeps its TOON `next` block; the two never both fire.
                // trace:TASK-974 trace:STORY-737 | ai:claude
                if let Some(sid) = last.spec_id.as_deref() {
                    if agent_output_mode() {
                        let next = crate::help_next::spec_next(&last.status.to_string(), sid);
                        if let Some(block) = crate::help_next::render(&next) {
                            println!("{block}");
                        }
                    } else {
                        println!(
                            "{}",
                            crate::help_next::render_human_add_footer(
                                &last.status.to_string(),
                                sid
                            )
                        );
                    }
                }
                // STORY-700: passive first-run chain — advancing past the "file
                // your first spec" step. Only fires when the arc is sitting at
                // exactly that step (the first `add` after a fresh `init`); every
                // later `add` is a silent no-op. trace:STORY-700 | ai:claude
                if let Ok(project_root) = find_project_root() {
                    first_run::after_first_spec(&project_root);
                }
            }
        }
        Command::Graph {
            id: graph_id,
            blocked_by,
            blocks,
            tree,
            impact,
            follow,
            depth,
            json,
            cmd,
        } => {
            let store = backend.load()?;
            let (id, blocked_by, blocks, tree, impact) = match cmd {
                Some(GraphCommand::BlockedBy { id }) => (id.as_str(), true, false, false, false),
                Some(GraphCommand::Blocks { id }) => (id.as_str(), false, true, false, false),
                Some(GraphCommand::Tree { id }) => (id.as_str(), false, false, true, false),
                Some(GraphCommand::Impact { id }) => (id.as_str(), false, false, false, true),
                None => {
                    let id = graph_id.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "aida graph needs a spec id, e.g. `aida graph tree STORY-1`"
                        )
                    })?;
                    if *blocked_by {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--blocked-by"),
                            "aida graph blocked-by <id>",
                        );
                    }
                    if *blocks {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--blocks"),
                            "aida graph blocks <id>",
                        );
                    }
                    if *tree {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--tree"),
                            "aida graph tree <id>",
                        );
                    }
                    if *impact {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--impact"),
                            "aida graph impact <id>",
                        );
                    }
                    (id, *blocked_by, *blocks, *tree, *impact)
                }
            };
            graph_cmd::handle_graph_command(
                &store, id, blocked_by, blocks, tree, impact, follow, *depth, *json,
            )?;
        }
        Command::Criteria { spec, json } => {
            let store = backend.load()?;
            let project_root = find_project_root()?;
            criteria::handle_criteria_command(&project_root, &store, spec, *json)?;
        }
        Command::Reconstitute {
            spec,
            json,
            dry_run,
        } => {
            let store = backend.load()?;
            let project_root = find_project_root()?;
            crate::reconstitute::handle_reconstitute_command(
                &project_root,
                &store,
                spec,
                crate::reconstitute::ReconstituteOptions {
                    json: *json,
                    dry_run: *dry_run,
                },
            )?;
        }
        Command::Harvest {
            spec,
            from,
            pr,
            base,
            yes_all,
            dry_run,
            json,
        } => {
            let store = backend.load()?;
            let project_root = find_project_root()?;
            crate::harvest::handle_harvest_command(
                &project_root,
                &store,
                spec,
                crate::harvest::HarvestOptions {
                    from: from.clone(),
                    pr: *pr,
                    base: base.clone(),
                    yes_all: *yes_all,
                    dry_run: *dry_run,
                    json: *json,
                },
            )?;
        }
        Command::Show {
            id,
            comments,
            tree,
            depth,
            sync,
            no_git,
            verbose,
            card,
            brief,
            full,
            rels,
            json,
        } => {
            // trace:TASK-518 | ai:antigravity
            let mut resolved_id = id.clone();
            if let Some(pr) = parse_pr_arg(id) {
                let store = backend.load()?;
                let project_root = store_path
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;
                // TASK-888: a bare number (no `pr`/`#` prefix) routes to PR
                // lookup, so a user expecting a spec lookup gets a PR-flavored
                // error with no clue why. When the arg is a prefixless digit
                // string, append a hint that spec ids carry a TYPE- prefix.
                // trace:TASK-888 | ai:claude
                let bare_number = id.trim().chars().all(|c| c.is_ascii_digit());
                let resolved = resolve_pr_to_spec(project_root, pr, &store).map_err(|e| {
                    if bare_number {
                        anyhow::anyhow!(
                            "{e}\n  note: spec ids look like TASK-12 — did you mean a spec id? \
                             bare numbers are treated as PR lookups."
                        )
                    } else {
                        e
                    }
                })?;
                println!("showing {} (backs {})", resolved, id);
                resolved_id = resolved;
            }
            let id = &resolved_id;

            // STORY-78: opt-in sync-pull before show. trace:STORY-78 | ai:claude
            if *sync {
                maybe_sync_pull(store_path)?;
            }
            // BUG-68: record activity only AFTER the lookup succeeds, so
            // a typo'd `aida show STORY-99` doesn't leave a phantom
            // STORY-99 in the activity log (which would then surface
            // as @SPEC in statusline). Each lookup branch below
            // records the canonical spec_id from the resolved req.
            // trace:BUG-68 | ai:claude
            // STORY-62: --tree replaces the detail view with an indented
            // hierarchy walk. Children are read via rel_type:Parent edges
            // on the parent's record; recursion descends until depth is
            // exhausted or no more children. Hidden tradeoff: each level
            // hits one more YAML read per visited node, but for typical
            // EPIC trees (~50 nodes max) that's fine, and the user opts
            // in. trace:STORY-62 | ai:claude
            // BUG-97: attach the parse-failure recovery hint to ANY error
            // from the lookup. Previously the swallowed-error path returned
            // None for both "file missing" AND "file failed to parse",
            // sending the user down a wrong-spec-id chase. git_backend's
            // get_requirement_by_spec_id now propagates parse errors;
            // wrap them here with the actionable hint. trace:BUG-97
            // BUG-599: a malformed id the user typed (not TYPE-SEQ /
            // TYPE-NODE-SEQ) is a typo, not an on-disk parse failure — give a
            // friendly format hint instead of the version-mismatch/rebuild wall
            // (which `parse_failure_hint` is reserved for below). trace:BUG-599
            if !is_spec_id_or_uuid(id) {
                return Err(not_found::invalid_spec_id_format(id));
            }
            // The id is well-formed, so any Err here is a genuine on-disk parse
            // failure (binary/version skew) worth the rebuild hint. A
            // well-formed-but-absent id comes back as Ok(None) and is handled
            // as a plain not-found below. trace:BUG-97 trace:BUG-599
            // BUG-1535: an ambiguous id is refused (non-zero exit, every
            // candidate listed) — never answered with one of them — and that
            // refusal passes through unwrapped. trace:BUG-1535 | ai:claude
            let lookup = backend.get_requirement_unambiguous(id).map_err(|e| {
                if e.downcast_ref::<aida_core::id_collisions::AmbiguousIdError>()
                    .is_some()
                {
                    return e;
                }
                anyhow::anyhow!(
                    "Parse failed: {}\n  Detail: {:#}\n{}",
                    id,
                    e,
                    aida_core::object_store::parse_failure_hint(None),
                )
            });
            if *tree {
                match lookup? {
                    Some(root) => {
                        record_role_activity(root.spec_id.as_deref().unwrap_or(id), "show");
                        render_tree(&backend, &root, *depth)?;
                    }
                    None => {
                        // BUG-600: a not-found lookup is a failure — return the
                        // error (exit non-zero) like edit/defer/archive, don't
                        // print-and-exit-0. trace:BUG-600 | ai:claude
                        return Err(not_found::requirement_not_found(id, Some(store_path)));
                    }
                }
                return Ok(());
            }
            match lookup? {
                Some(req) => {
                    record_role_activity(req.spec_id.as_deref().unwrap_or(id), "show");
                    // STORY-632: deterministic local graph-centrality, read from
                    // the cache (recomputed on rebuild from the relationship
                    // graph; never stored in YAML). trace:STORY-632 | ai:claude
                    let degrees = backend.degrees(&req.id).unwrap_or_default();
                    let (cached_epic_status, serialize_cluster_command) =
                        show_cached_context(&backend, &req);
                    // BUG-626: the cache projects an EPIC's read-only child
                    // rollup. Using that projection avoids loading every YAML
                    // object merely to render one spec. trace:BUG-626
                    let effective_status_str =
                        cached_epic_status.unwrap_or_else(|| format!("{}", req.effective_status()));
                    // BUG-781: JSON is the machine projection of the same
                    // human detail view, so use the type-aware display label
                    // (a stored Approved decision is displayed as Accepted).
                    // Keep the stored status separately below; consumers must
                    // not have to infer which value is presentation-only.
                    // trace:BUG-1502 | ai:codex
                    let display_status = status_display::display_status_for_type(
                        &format!("{:?}", req.req_type),
                        &effective_status_str,
                    )
                    .to_string();
                    // STORY-632: `--json` emits the spec as a machine object,
                    // including the centrality fields, then returns early.
                    // trace:STORY-632 | ai:claude
                    // The global format pin and the dedicated flag are two
                    // spellings of the same machine contract. Keeping this
                    // test at the renderer (rather than only in dispatch)
                    // also covers AIDA_OUTPUT_FORMAT=json.
                    // trace:BUG-1502 | ai:codex
                    if *json || output_format_is_json() {
                        // BUG-1558: the machine JSON projection used to omit
                        // relationships entirely — an agent reading `aida show
                        // --json` had no way to see the typed graph at all,
                        // which is a stronger form of the same collapse the
                        // human/TOON surfaces had (a missing field instead of
                        // a mislabeled one). Emit the same stored `rel_type`
                        // label the TOON table uses (`rel_type_label`) so all
                        // three surfaces agree on vocabulary.
                        // trace:BUG-1558 | ai:claude
                        #[derive(serde::Serialize)]
                        struct RelJson {
                            rel_type: String,
                            id: String,
                            title: String,
                        }
                        #[derive(serde::Serialize)]
                        struct GitCommitJson {
                            sha: String,
                            short_sha: String,
                            subject: String,
                        }
                        #[derive(serde::Serialize)]
                        struct GitFileJson {
                            file: String,
                            symbol: Option<String>,
                        }
                        #[derive(serde::Serialize)]
                        struct GitLinkageJson {
                            commits: Vec<GitCommitJson>,
                            files: Vec<GitFileJson>,
                            shipped: bool,
                            branch: Option<String>,
                            worktree: Option<String>,
                            shipped_pr: Option<u64>,
                            repo: Option<String>,
                            // BUG-1594 (PRIN-5): `false` when a bounded scan
                            // stopped early; `incomplete` names which part.
                            // trace:BUG-1594 | ai:claude
                            complete: bool,
                            incomplete: Vec<String>,
                        }
                        let relationships: Vec<RelJson> = req
                            .relationships
                            .iter()
                            .map(|rel| {
                                let (id, title) = match backend.get_requirement(&rel.target_id) {
                                    Ok(Some(t)) => (t.display_id(), t.title.clone()),
                                    _ => ("(unknown)".to_string(), String::new()),
                                };
                                RelJson {
                                    rel_type: rel_type_label(&rel.rel_type),
                                    id,
                                    title,
                                }
                            })
                            .collect();
                        let git_linkage = if *no_git {
                            None
                        } else {
                            let project_root = store_path
                                .parent()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                            let mut ids = vec![req.display_id()];
                            if let Some(agreed) = req.agreed_id.as_deref() {
                                if !ids.iter().any(|id| id == agreed) {
                                    ids.push(agreed.to_string());
                                }
                            }
                            if let Some(origin) = req.spec_id.as_deref() {
                                if !ids.iter().any(|id| id == origin) {
                                    ids.push(origin.to_string());
                                }
                            }
                            let linkage = crate::collect_git_linkage(&project_root, &ids);
                            // trace:BUG-1594 | ai:claude
                            let mut incomplete = Vec::new();
                            if let Some((scanned, total)) = linkage.branch_scan_truncated {
                                incomplete
                                    .push(crate::format_branch_scan_truncated_note(scanned, total));
                            }
                            if linkage.files_scan_incomplete {
                                incomplete
                                    .push(crate::LINKAGE_TRACE_SCAN_INCOMPLETE_NOTE.to_string());
                            }
                            Some(GitLinkageJson {
                                complete: incomplete.is_empty(),
                                incomplete,
                                commits: linkage
                                    .commits
                                    .into_iter()
                                    .map(|(sha, short_sha, subject)| GitCommitJson {
                                        sha,
                                        short_sha,
                                        subject,
                                    })
                                    .collect(),
                                files: linkage
                                    .files
                                    .into_iter()
                                    .map(|(file, symbol)| GitFileJson { file, symbol })
                                    .collect(),
                                shipped: linkage.shipped,
                                branch: linkage.branch,
                                worktree: linkage.worktree,
                                shipped_pr: linkage.shipped_pr,
                                repo: linkage.repo,
                            })
                        };
                        // BUG-527: carry the human-visible queue membership
                        // into JSON as structured role/position pairs. The
                        // helper uses the same per-role rank as the human
                        // `Queued:` line, including the `general` label for
                        // unrouted entries. trace:BUG-1502 | ai:codex
                        let project_root = store_path
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                        let queue_membership: Vec<serde_json::Value> =
                            queue_memberships_for(&project_root, &req.id)
                                .into_iter()
                                .map(|(role, position)| {
                                    serde_json::json!({
                                        "role": role.unwrap_or_else(|| "general".to_string()),
                                        "position": position,
                                    })
                                })
                                .collect();
                        // Start from the complete stored requirement rather
                        // than maintaining a lossy parallel projection. Then
                        // overlay every derived field the human view computes
                        // (effective lifecycle values, graph context, next
                        // actions, and git linkage). New stored human fields
                        // now arrive in JSON automatically instead of silently
                        // disappearing until another bug is filed.
                        // trace:BUG-1502 | ai:codex
                        let mut out = serde_json::to_value(&req)?;
                        let object = out.as_object_mut().ok_or_else(|| {
                            anyhow::anyhow!("serialized requirement was not a JSON object")
                        })?;
                        object.insert(
                            "display_id".to_string(),
                            serde_json::Value::String(req.display_id()),
                        );
                        object.insert(
                            "uuid".to_string(),
                            serde_json::Value::String(req.id.to_string()),
                        );
                        object.insert("opened".to_string(), serde_json::to_value(req.created_at)?);
                        object.insert(
                            "modified".to_string(),
                            serde_json::to_value(req.modified_at)?,
                        );
                        object.insert(
                            "status".to_string(),
                            serde_json::Value::String(display_status.clone()),
                        );
                        object.insert(
                            "stored_status".to_string(),
                            serde_json::Value::String(req.status.to_string()),
                        );
                        object.insert(
                            "priority".to_string(),
                            serde_json::Value::String(format!("{}", req.effective_priority())),
                        );
                        object.insert(
                            "relationships".to_string(),
                            serde_json::to_value(&relationships)?,
                        );
                        object.insert(
                            "in_degree".to_string(),
                            serde_json::to_value(degrees.in_degree)?,
                        );
                        object.insert(
                            "out_degree".to_string(),
                            serde_json::to_value(degrees.out_degree)?,
                        );
                        object.insert("heft".to_string(), serde_json::to_value(degrees.heft)?);

                        let blockers: Vec<serde_json::Value> = req
                            .relationships
                            .iter()
                            .filter(|rel| matches!(rel.rel_type, RelationshipType::BlockedBy))
                            .map(|rel| match backend.get_requirement(&rel.target_id) {
                                Ok(Some(blocker)) => serde_json::json!({
                                    "id": blocker.display_id(),
                                    "status": blocker.status.to_string(),
                                    "satisfied": matches!(blocker.status, RequirementStatus::Completed),
                                }),
                                _ => serde_json::json!({
                                    "id": rel.target_id.to_string(),
                                    "status": "missing",
                                    "satisfied": false,
                                }),
                            })
                            .collect();
                        object.insert(
                            "blocked".to_string(),
                            serde_json::Value::Bool(blockers.iter().any(|v| {
                                v.get("satisfied").and_then(|v| v.as_bool()) == Some(false)
                            })),
                        );
                        object.insert("blockers".to_string(), serde_json::Value::Array(blockers));

                        let mut next =
                            crate::help_next::spec_next(&effective_status_str, &req.display_id());
                        crate::help_next::push_serialize_cluster(
                            &mut next,
                            serialize_cluster_command.clone(),
                        );
                        object.insert(
                            "next".to_string(),
                            serde_json::Value::Array(
                                next.into_iter()
                                    .map(|step| serde_json::json!({"cmd": step.cmd, "to": step.to}))
                                    .collect(),
                            ),
                        );
                        object.insert(
                            "git_linkage".to_string(),
                            serde_json::to_value(git_linkage)?,
                        );
                        object.insert(
                            "queue_membership".to_string(),
                            serde_json::Value::Array(queue_membership),
                        );
                        println!("{}", serde_json::to_string_pretty(&out)?);
                        return Ok(());
                    }
                    // TASK-964: AGENT-MODE token-efficient TOON render for the
                    // single-spec detail. A flat scalar head (the minimal field
                    // set an agent needs) plus a uniform relationships table.
                    // The human TTY detail view drops through unchanged. The
                    // description body is truncated unless `--full`/`--verbose`,
                    // mirroring `--card` density. trace:TASK-964
                    if agent_output_mode() && !*card {
                        let mut lines: Vec<String> = Vec::new();
                        lines.push(crate::toon::scalar("id", &req.display_id()));
                        if let Some(origin) = req.spec_id.as_deref() {
                            if origin != req.display_id() {
                                lines.push(crate::toon::scalar("origin_id", origin));
                            }
                        }
                        lines.push(crate::toon::scalar("title", &req.title));
                        lines.push(crate::toon::scalar(
                            "type",
                            &format!("{:?}", req.req_type).to_ascii_lowercase(),
                        ));
                        lines.push(crate::toon::scalar(
                            "status",
                            &toon_status_token(&effective_status_str),
                        ));
                        lines.push(crate::toon::scalar(
                            "priority",
                            &format!("{}", req.effective_priority()).to_ascii_lowercase(),
                        ));
                        lines.push(crate::toon::scalar("owner", &req.owner));
                        if let Some(a) = req.assignee.as_deref() {
                            lines.push(crate::toon::scalar("assignee", a));
                        }
                        if !req.feature.is_empty() {
                            lines.push(crate::toon::scalar("feature", &req.feature));
                        }
                        lines.push(crate::toon::scalar("heft", &degrees.heft.to_string()));
                        // FR-283: the numeric weight/score, emitted only when
                        // set. trace:FR-283 | ai:claude
                        if let Some(w) = req.weight {
                            lines.push(crate::toon::scalar(
                                "weight",
                                &crate::format_weight(w as f64),
                            ));
                        }
                        if !req.tags.is_empty() {
                            let mut tags: Vec<&str> = req.tags.iter().map(String::as_str).collect();
                            tags.sort_unstable();
                            lines.push(crate::toon::scalar("tags", &tags.join(" ")));
                        }
                        // Description body: truncated by char count unless the
                        // user opted into the full view. ASCII marker only.
                        let want_full = *full || *verbose;
                        let desc = if want_full {
                            req.description.clone()
                        } else {
                            const CAP: usize = 280;
                            if req.description.chars().count() <= CAP {
                                req.description.clone()
                            } else {
                                let head: String = req.description.chars().take(CAP).collect();
                                format!("{head} ... (truncated; --full for the rest)")
                            }
                        };
                        lines.push(crate::toon::scalar("description", &desc));
                        // TASK-1148: the three optional narrative fields, emitted
                        // as scalars only when set. trace:TASK-1148 | ai:claude
                        if let Some(v) = req.implementation_summary.as_deref() {
                            if !v.trim().is_empty() {
                                lines.push(crate::toon::scalar("implementation_summary", v));
                            }
                        }
                        if let Some(v) = req.risk_notes.as_deref() {
                            if !v.trim().is_empty() {
                                lines.push(crate::toon::scalar("risk_notes", v));
                            }
                        }
                        if let Some(v) = req.test_coverage_notes.as_deref() {
                            if !v.trim().is_empty() {
                                lines.push(crate::toon::scalar("test_coverage_notes", v));
                            }
                        }
                        // STORY-776: the advisor's dispatch classification,
                        // emitted only when groomed. trace:STORY-776 | ai:claude
                        if let Some(m) = req.execution_mode {
                            lines.push(crate::toon::scalar("execution_mode", &m.to_string()));
                        }
                        // STORY-634: the multi-repo origin dimension, emitted
                        // only when set. trace:STORY-634 | ai:claude
                        if let Some(o) = &req.origin {
                            lines.push(crate::toon::scalar("origin", &o.to_string()));
                        }
                        // CR-8: filing provenance, one compact scalar, only
                        // when stamped. trace:CR-8 | ai:claude
                        if let Some(p) = req.filed_at.as_ref().filter(|p| !p.is_empty()) {
                            lines.push(crate::toon::scalar("filed_at", &p.summary_line()));
                        }
                        println!("{}", lines.join("\n"));

                        // Relationships as a uniform TOON table (rel,id,title).
                        // Unlike the human card's presentation buckets, the
                        // agent surface must preserve the stored edge type.
                        // trace:BUG-1442 | ai:codex
                        if !req.relationships.is_empty() {
                            let mut rows: Vec<Vec<String>> = Vec::new();
                            for rel in &req.relationships {
                                let label = rel_type_label(&rel.rel_type);
                                let (tid, ttitle) = match backend.get_requirement(&rel.target_id) {
                                    Ok(Some(t)) => (t.display_id(), t.title.clone()),
                                    _ => ("(unknown)".to_string(), String::new()),
                                };
                                rows.push(vec![label, tid, ttitle]);
                            }
                            println!(
                                "{}",
                                crate::toon::table_raw(
                                    "relationships",
                                    &["rel", "id", "title"],
                                    &rows
                                )
                            );
                        }
                        let blocker_targets: Vec<uuid::Uuid> = req
                            .relationships
                            .iter()
                            .filter(|rel| matches!(rel.rel_type, RelationshipType::BlockedBy))
                            .map(|rel| rel.target_id)
                            .collect();
                        if !blocker_targets.is_empty() {
                            let mut rows = Vec::new();
                            let mut unsatisfied = 0usize;
                            for target in blocker_targets {
                                let (id, status, satisfied) = match backend
                                    .get_requirement(&target)?
                                {
                                    Some(blocker) => {
                                        let satisfied =
                                            matches!(blocker.status, RequirementStatus::Completed);
                                        (
                                            blocker.display_id(),
                                            blocker.status.to_string(),
                                            satisfied,
                                        )
                                    }
                                    None => (target.to_string(), "missing".to_string(), false),
                                };
                                if !satisfied {
                                    unsatisfied += 1;
                                }
                                rows.push(vec![id, status, satisfied.to_string()]);
                            }
                            println!(
                                "{}",
                                crate::toon::table_raw(
                                    "blockers",
                                    &["id", "status", "satisfied"],
                                    &rows
                                )
                            );
                            println!(
                                "{}",
                                crate::toon::scalar("blocked", &(unsatisfied > 0).to_string())
                            );
                            if unsatisfied > 0 {
                                println!(
                                    "{}",
                                    crate::toon::scalar(
                                        "pickup",
                                        "refused until all blockers are Completed"
                                    )
                                );
                            }
                        }
                        // TASK-974 (AXI #9): lifecycle-aware next-step block —
                        // the valid next transition(s) for THIS spec's current
                        // state, templated with its id. trace:TASK-974
                        let mut next =
                            crate::help_next::spec_next(&effective_status_str, &req.display_id());
                        crate::help_next::push_serialize_cluster(
                            &mut next,
                            serialize_cluster_command.clone(),
                        );
                        if let Some(block) = crate::help_next::render(&next) {
                            println!("{block}");
                        }
                        return Ok(());
                    }
                    // TASK-265: --card renders a compact boxed spec card
                    // instead of the linear detail view, so /aida-pickup can
                    // drop the spec's contract into terminal scrollback at
                    // session start. The plain `aida show` stays the
                    // canonical detail surface. trace:TASK-265 | ai:claude
                    if *card {
                        let density = if *brief {
                            CardDensity::Brief
                        } else if *full {
                            CardDensity::Full
                        } else {
                            CardDensity::Balanced
                        };
                        // Resolve each relationship target into a `CardRel`
                        // row so the card can name parents, related specs AND
                        // custom edges, not just count them.
                        // trace:BUG-1471 | ai:claude
                        let mut rels: Vec<CardRel> = Vec::new();
                        for rel in &req.relationships {
                            let (rid, rtitle) = match backend.get_requirement(&rel.target_id) {
                                Ok(Some(t)) => (t.display_id(), t.title.clone()),
                                _ => ("(unknown)".to_string(), String::new()),
                            };
                            rels.push(CardRel::new(&rel.rel_type, rid, rtitle));
                        }
                        render_spec_card(&req, &rels, store_path, density, *no_git, *verbose);
                        let mut next = Vec::new();
                        crate::help_next::push_serialize_cluster(
                            &mut next,
                            serialize_cluster_command.clone(),
                        );
                        if let Some(block) = crate::help_next::render_human(&next) {
                            println!("{block}");
                        }
                        return Ok(());
                    }
                    println!("{}: {}", "ID".bold(), req.display_id());
                    // Only show Agreed ID / Origin ID when they actually
                    // differ from the canonical display id — otherwise it's
                    // three lines of the same string. trace:BUG-29
                    let canonical = req.display_id();
                    if let Some(ref agreed) = req.agreed_id {
                        if agreed.as_str() != canonical {
                            println!("{}: {}", "Agreed ID".bold(), agreed);
                        }
                    }
                    if let Some(ref origin) = req.spec_id {
                        if origin.as_str() != canonical {
                            // trace:FR-1-070 | ai:claude
                            println!("{}: {}", "Origin ID".bold(), origin);
                        }
                    }
                    println!("{}: {}", "UUID".bold(), req.id);
                    println!("{}: {}", "Title".bold(), req.title);
                    // TASK-91: surface PR-N for auto-queued review stories
                    // so `aida show STORY-108` reads "About: PR-15" right
                    // next to the title, mirroring what `aida queue list`
                    // promotes to the prominent id. Dual-id discoverable
                    // from a single command. trace:TASK-91 | ai:claude
                    if let Some((primary, _)) =
                        format_review_story_display(req.display_id().as_str(), req.title.as_str())
                    {
                        // primary is "PR-N (STORY-NNN)"; strip the
                        // parenthetical for a cleaner About: line.
                        let about = primary
                            .split_once(' ')
                            .map(|(pr, _)| pr.to_string())
                            .unwrap_or(primary);
                        println!("{}: {}", "About".bold(), about.cyan());
                    }
                    println!("{}: {:?}", "Type".bold(), req.req_type);
                    // TASK-269: colour-code Status inline so a quick scan
                    // catches it; the same value is reprinted at the foot
                    // of the output below. trace:TASK-269 | ai:claude
                    // BUG-626: derived rollup status for epics. trace:BUG-626
                    // BUG-781: for a decision spec, the stored `Approved` IS
                    // ACCEPTED — the terminal state — so display it that way
                    // here and in the reprint at the foot. trace:BUG-781
                    let status = display_status.clone();
                    println!(
                        "{}: {}",
                        "Status".bold(),
                        status_display::status_badge(&status)
                    );
                    println!("{}: {}", "Priority".bold(), req.effective_priority());
                    // FR-283: the numeric weight/score, shown only when set.
                    // trace:FR-283 | ai:claude
                    if let Some(w) = req.weight {
                        println!("{}: {}", "Weight".bold(), crate::format_weight(w as f64));
                    }
                    // STORY-634: where this spec's code lives in a multi-repo
                    // shared-store workspace ("repo" or "repo/component");
                    // absent = single-repo, prints nothing.
                    if let Some(o) = &req.origin {
                        println!("{}: {}", "Origin".bold(), o.to_string().cyan());
                    }
                    // CR-8: where (code state) and with what (tooling) the
                    // spec was filed — one compact line, only when stamped.
                    // trace:CR-8 | ai:claude
                    if let Some(p) = req.filed_at.as_ref().filter(|p| !p.is_empty()) {
                        println!("{}: {}", "Filed at".bold(), p.summary_line().dimmed());
                        if p.build_diverges_from_code() && crate::cwd_is_aida_source_repo() {
                            println!(
                                "  {} filed by a binary built from a different commit than \
                                 the checked-out code",
                                "note:".yellow()
                            );
                        }
                    }
                    // BUG-524: surface when the spec was opened / last touched so
                    // `aida show` reveals its age. Stored UTC, rendered in local
                    // time (feedback_local_time) reusing history.rs's format.
                    // trace:BUG-524 | ai:claude
                    println!(
                        "{}: {}",
                        "Opened".bold(),
                        req.created_at
                            .with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                    );
                    println!(
                        "{}: {}",
                        "Modified".bold(),
                        req.modified_at
                            .with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                    );
                    if !req.owner.is_empty() {
                        println!("{}: {}", "Owner".bold(), req.owner);
                    }
                    // STORY-639: surface the assignee only when set, so an
                    // unassigned spec renders exactly as before. trace:STORY-639
                    if let Some(assignee) = req.assignee.as_deref() {
                        println!("{}: {}", "Assignee".bold(), assignee);
                    }
                    if !req.tags.is_empty() {
                        println!(
                            "{}: {}",
                            "Tags".bold(),
                            req.tags.iter().cloned().collect::<Vec<_>>().join(", ")
                        );
                    }
                    // STORY-476: one-way external issue refs rendered as links
                    // via the `[external_refs]` base URLs in config. A ref with
                    // no resolvable base URL prints its bare `provider:id`.
                    // trace:STORY-476 | ai:claude
                    if !req.external_refs.is_empty() {
                        let base_urls = read_external_ref_base_urls(store_path);
                        println!("{}:", "External refs".bold());
                        for raw in &req.external_refs {
                            match aida_core::external_refs::parse_ref(raw) {
                                Ok(parsed) => {
                                    match aida_core::external_refs::render_ref_url(
                                        &parsed, &base_urls,
                                    ) {
                                        Some(url) => println!(
                                            "  ↗ {} → {}",
                                            parsed.canonical().yellow(),
                                            url.cyan()
                                        ),
                                        None => println!("  ↗ {}", parsed.canonical().yellow()),
                                    }
                                }
                                // Stored ref that no longer parses (e.g. a
                                // provider removed from the known set) — show
                                // it verbatim rather than dropping it.
                                Err(_) => println!("  ↗ {}", raw.yellow()),
                            }
                        }
                    }
                    // STORY-542: user-facing interface changes captured at
                    // close — the deterministic source the operator digest
                    // reads. trace:STORY-542 | ai:claude
                    if let Some(ic) = req.interface_changes.as_ref() {
                        if !ic.is_empty() {
                            println!("{}:", "Interface changes".bold());
                            let surfaces: [(&str, &Vec<String>); 4] = [
                                ("CLI", &ic.cli),
                                ("MCP", &ic.mcp),
                                ("TUI", &ic.tui),
                                ("Other", &ic.other),
                            ];
                            for (label, lines) in surfaces {
                                for line in lines {
                                    println!("  {} {}", format!("[{label}]").cyan(), line);
                                }
                            }
                        }
                    }
                    // TASK-102: enumerate relationships inline when there are
                    // few (≤5) or `--rels` is passed; otherwise print the count
                    // + a pointer to `aida rel list`. Truncate past 10 with a
                    // "…N more" line so highly-connected specs don't flood the
                    // view. Each edge shows direction-phrase + target + title.
                    // trace:TASK-102 | ai:claude
                    if !req.relationships.is_empty() {
                        let total = req.relationships.len();
                        const INLINE_THRESHOLD: usize = 5;
                        const TRUNCATE_AT: usize = 10;
                        let spec_label = req.spec_id.as_deref().unwrap_or(id);
                        if *rels || total <= INLINE_THRESHOLD {
                            println!("{}:", "Relations".bold());
                            for rel in req.relationships.iter().take(TRUNCATE_AT) {
                                let phrase = relationship_phrase(&rel.rel_type);
                                match backend.get_requirement(&rel.target_id)? {
                                    Some(t) => println!(
                                        "  {} {} {} ({})",
                                        crate::glyph(crate::glyphs::Glyph::SubArrow),
                                        phrase.cyan(),
                                        t.spec_id.as_deref().unwrap_or("?").yellow(),
                                        t.title
                                    ),
                                    None => println!(
                                        "  {} {} {} (missing)",
                                        crate::glyph(crate::glyphs::Glyph::SubArrow),
                                        phrase.cyan(),
                                        rel.target_id.to_string().dimmed()
                                    ),
                                }
                            }
                            if total > TRUNCATE_AT {
                                println!(
                                    "  … {} more — see `aida rel list {}`",
                                    total - TRUNCATE_AT,
                                    spec_label
                                );
                            }
                        } else {
                            // TASK-1417: the count alone doesn't tell a reader
                            // whether rows exist and were withheld by policy,
                            // or the spec genuinely has nothing more — say so
                            // explicitly so the two cases can't be confused.
                            // TOON is unchanged by this (it always enumerates,
                            // above); this line is the human-only view limit.
                            // trace:TASK-1417 | ai:claude
                            println!(
                                "{}: {} relationship(s) — withheld by the view limit, not enumerated below (use {} to show every edge, or `aida rel list {}`)",
                                "Relations".bold(),
                                total,
                                "--rels".cyan(),
                                spec_label
                            );
                        }
                    }
                    // STORY-632: deterministic centrality readout — inbound +
                    // outbound degree (tracked separately: high inbound =
                    // load-bearing/foundational, high outbound = coupling) plus
                    // the type-weighted heft score. Shown only when the spec is
                    // connected. trace:STORY-632 | ai:claude
                    if degrees.in_degree > 0 || degrees.out_degree > 0 {
                        println!(
                            "{}: {} in / {} out  (heft {})",
                            "Centrality".bold(),
                            degrees.in_degree,
                            degrees.out_degree,
                            degrees.heft
                        );
                    }
                    // STORY-446: Blockers section — one line per BlockedBy edge
                    // with the blocker's status + a pickability glyph (check when
                    // Completed/satisfied, in-flight when it still blocks pickup), so the
                    // pickability gate's verdict is visible at a glance.
                    // trace:STORY-446 | ai:claude
                    let blocker_targets: Vec<uuid::Uuid> = req
                        .relationships
                        .iter()
                        .filter(|r| matches!(r.rel_type, RelationshipType::BlockedBy))
                        .map(|r| r.target_id)
                        .collect();
                    if !blocker_targets.is_empty() {
                        println!("\n{}:", "Blockers".bold());
                        let mut unsatisfied = 0;
                        for target in &blocker_targets {
                            let (sid, status, satisfied) = match backend.get_requirement(target)? {
                                Some(b) => {
                                    let satisfied =
                                        matches!(b.status, RequirementStatus::Completed);
                                    (
                                        b.spec_id.clone().unwrap_or_else(|| "?".to_string()),
                                        format!("{}", b.status),
                                        satisfied,
                                    )
                                }
                                None => (target.to_string(), "missing".to_string(), false),
                            };
                            if !satisfied {
                                unsatisfied += 1;
                            }
                            let glyph = if satisfied {
                                crate::glyph(crate::glyphs::Glyph::Check)
                                    .green()
                                    .to_string()
                            } else {
                                crate::glyph(crate::glyphs::Glyph::InFlight)
                                    .yellow()
                                    .to_string()
                            };
                            println!("  {} {} ({})", glyph, sid, status);
                        }
                        if unsatisfied > 0 {
                            println!(
                                "  {} blocked — {} blocker(s) not yet Completed; `aida queue work` will refuse pickup",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                unsatisfied
                            );
                        }
                    }
                    if !req.comments.is_empty() {
                        println!("{}: {} comment(s)", "Comments".bold(), req.comments.len());
                    }
                    {
                        // trace:STORY-1173 | ai:codex
                        let mut approval_entries = Vec::new();
                        collect_proxy_approval_entries(
                            &mut approval_entries,
                            &req.display_id(),
                            &req.title,
                            &req.comments,
                            None,
                            None,
                        );
                        if !approval_entries.is_empty() {
                            println!();
                            print_proxy_approval_entries(&approval_entries, false);
                        }
                    }
                    // STORY-81: surface the auto-stamped completion
                    // context (who/when/source-tool/optional summary)
                    // when `aida queue done` populated it. Skips when
                    // `implementation_info` is None or `implemented`
                    // is false — keeps the section out of `aida show`
                    // for reqs that were closed via `aida edit
                    // --status completed` directly (no queue done flow).
                    // trace:STORY-81 | ai:claude
                    if let Some(info) = req.implementation_info.as_ref().filter(|i| i.implemented) {
                        println!("\n{}:", "Implementation".green().bold());
                        if let Some(ts) = info.implemented_at {
                            println!(
                                "  Completed: {}",
                                ts.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M %Z")
                            );
                        }
                        if let Some(ref who) = info.implemented_by {
                            println!("  By:        {}", who);
                        }
                        if let Some(ref tool) = info.source_tool {
                            println!("  Source:    {}", tool);
                        }
                        if let Some(ref summary) = info.summary {
                            println!("  Summary:");
                            for line in summary.lines() {
                                println!("    {}", line);
                            }
                        }
                        // STORY-698: the verification steps the builder ran,
                        // captured at `aida queue done` — the audit trail the
                        // PR body surfaces. trace:STORY-698 | ai:claude
                        if let Some(steps) = info
                            .test_coverage_notes
                            .as_ref()
                            .filter(|s| !s.trim().is_empty())
                        {
                            println!("  Verification:");
                            for line in steps.lines().filter(|l| !l.trim().is_empty()) {
                                println!("    - {}", line);
                            }
                        }
                    }
                    // STORY-332: surface the punt reason when the spec is
                    // paused in NeedsAttention, so triage sees the fork
                    // without grepping the ledger. `attention_reason` is
                    // present only while paused (cleared on triage-out).
                    if let Some(reason) = req.attention_reason.as_ref() {
                        println!("\n{}:", "Needs Attention — punted".magenta().bold());
                        println!("  Category:  {}", reason.category);
                        println!("  Reason:    {}", reason.detail);
                        if let Some(lean) = &reason.lean {
                            println!("  Lean:      {}", lean);
                        }
                        if let Some(who) = &reason.raised_by {
                            println!("  Raised by: {}", who);
                        }
                        println!(
                            "  Raised at: {}",
                            reason
                                .raised_at
                                .with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M %Z")
                        );
                    }
                    // STORY-522: surface a PENDING DecisionRequest so the
                    // human sees the question + choices in `aida show`,
                    // self-contained, and can answer it with
                    // `aida questions answer`. trace:STORY-522 | ai:claude
                    if let Some(dr) = req.decision_request.as_ref().filter(|d| d.is_pending()) {
                        println!("\n{}:", "Decision needed".yellow().bold());
                        println!("  {}", dr.question);
                        for (i, choice) in dr.choices.iter().enumerate() {
                            let marker = if dr.recommended == Some(i) {
                                " (recommended)".green().to_string()
                            } else {
                                String::new()
                            };
                            println!(
                                "    {}{} {} — {}",
                                format!("{}.", i + 1).bold(),
                                marker,
                                choice.label.bold(),
                                choice.consequence.dimmed()
                            );
                        }
                        if let Some(rationale) = &dr.rationale {
                            println!("  {} {}", "Why:".dimmed(), rationale);
                        }
                        println!("  {}", "Answer with `aida questions answer`.".dimmed());
                    }
                    // STORY-776: the advisor's dispatch classification — how
                    // `aida do` will route this spec. Rendered only when
                    // groomed, with the human contract so the routing is
                    // explicit at a glance. trace:STORY-776 | ai:claude
                    if let Some(m) = req.execution_mode {
                        println!(
                            "\n{} {} — {}",
                            "Execution mode:".bold(),
                            m.to_string().cyan(),
                            crate::do_dispatch::human_contract(m).dimmed()
                        );
                    }
                    if !req.description.is_empty() {
                        println!("\n{}", req.description);
                    }
                    // STORY-1434: corrections and carve-outs recorded as
                    // comments must be visible in the DEFAULT view (no
                    // `-c`/`--comments`), right after the description — the
                    // bug this closes is exactly a correction that only
                    // `-c`, or a scroll hundreds of lines into the YAML,
                    // would have shown. Runs unconditionally (not gated on
                    // `*comments`) and duplicates into the full `-c` comment
                    // list below without harm. trace:STORY-1434 | ai:claude
                    {
                        let mut visible_comments: Vec<&aida_core::Comment> = Vec::new();
                        crate::collect_default_visible_comments(
                            &req.comments,
                            &mut visible_comments,
                        );
                        if !visible_comments.is_empty() {
                            println!("\n{}:", "Corrections / carve-outs".yellow().bold());
                            for c in &visible_comments {
                                print_comment(c, 0);
                            }
                        }
                    }
                    // TASK-1148: the three optional narrative fields — the
                    // genuinely-new metadata not derivable from git/status/trace.
                    // Each renders its own labelled block only when set; absent
                    // fields print nothing. trace:TASK-1148 | ai:claude
                    {
                        let narrative_blocks: [(&str, &Option<String>); 3] = [
                            ("Implementation summary", &req.implementation_summary),
                            ("Risk notes", &req.risk_notes),
                            ("Test coverage notes", &req.test_coverage_notes),
                        ];
                        for (label, value) in narrative_blocks {
                            if let Some(text) = value {
                                let text = text.trim();
                                if !text.is_empty() {
                                    println!("\n{}:", label.bold());
                                    for line in text.lines() {
                                        println!("  {line}");
                                    }
                                }
                            }
                        }
                    }
                    if *comments && !req.comments.is_empty() {
                        println!("\n{}:", "Comments".green().bold());
                        for c in &req.comments {
                            print_comment(c, 0);
                        }
                    }
                    // STORY-582: surface the durable processing record — the
                    // committed audit of what was done + why at completion.
                    // Always shown (not gated on --verbose) so the audit trail
                    // is visible on a plain `aida show`. trace:STORY-582
                    if !req.processing_record.is_empty() {
                        print_processing_records(&req.processing_record);
                    }
                    // BUG-527: surface queue membership — for-role + the
                    // 1-based position the operator sees on `aida queue
                    // list`. Mirrors how git-linkage is surfaced below; the
                    // line is OMITTED when the spec sits in no queue so a
                    // not-queued spec's card stays clean. Mirrored onto the
                    // MCP `show_requirement` tool for parity (STORY-82).
                    // trace:BUG-527
                    {
                        let project_root = store_path
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                        let memberships = queue_memberships_for(&project_root, &req.id);
                        if let Some(value) = format_queue_membership(&memberships) {
                            println!("{}: {}", "Queued".green().bold(), value);
                        }
                    }
                    // TASK-241: append the git-linkage section unless
                    // --no-git. Grep against every id form the spec has
                    // worn (canonical / agreed / origin) so commits and
                    // trace comments survive id migration.
                    if !*no_git {
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
                        print_git_linkage(&project_root, &ids, *verbose);
                    }
                    // TASK-269: `aida show` output runs 50-150 lines, so the
                    // top Status field scrolls off-screen. Reprint it after a
                    // rule — "what state is this in?" then sits at the cursor
                    // when the command returns. trace:TASK-269 | ai:claude
                    println!("\n{}", "─".repeat(40).dimmed());
                    println!(
                        "{}: {}",
                        "Status".bold(),
                        status_display::status_badge(&status)
                    );
                    // STORY-727: the per-spec next-command block for HUMANS. The
                    // agent-mode TOON `next` block fires on its own branch above
                    // (and returns); STORY-723 added next-steps to the front door
                    // but not to per-spec inspection, so the human `show` never
                    // got a next command. Render it now, leading with `aida zen
                    // <id>` for an Approved/Planned spec. trace:STORY-727
                    let mut next =
                        crate::help_next::spec_next(&effective_status_str, &req.display_id());
                    crate::help_next::push_serialize_cluster(&mut next, serialize_cluster_command);
                    if let Some(block) = crate::help_next::render_human(&next) {
                        println!("{block}");
                    }
                }
                None => {
                    // BUG-600: not-found is a failure — return the error so the
                    // process exits non-zero, matching edit/defer/archive and
                    // keeping script gating + usage telemetry honest (was a
                    // print-and-exit-0). trace:BUG-600 | ai:claude
                    return Err(not_found::requirement_not_found(id, Some(store_path)));
                }
            }
        }
        Command::Approvals {
            since,
            until,
            json,
            command,
        } => {
            // trace:STORY-1173 | ai:codex
            match command {
                Some(ApprovalCommand::Add {
                    spec,
                    decision,
                    verdict,
                    proxy,
                    session,
                    authorized_by,
                    independent,
                }) => {
                    return add_proxy_approval(
                        &backend,
                        spec,
                        decision,
                        verdict,
                        proxy.as_deref(),
                        session.as_deref(),
                        authorized_by,
                        *independent,
                    );
                }
                None => {
                    return list_proxy_approvals(
                        &backend,
                        since.as_deref(),
                        until.as_deref(),
                        *json || output_format_is_json(),
                    );
                }
            }
        }
        Command::Edit {
            id,
            title,
            description,
            description_from_file,
            description_stdin,
            status,
            priority,
            r#type,
            owner,
            feature,
            tags,
            add_tag,
            remove_tag,
            blocked_by,
            remove_blocked_by,
            // trace:TASK-1176 | ai:claude
            superseded_by,
            // trace:STORY-1434 | ai:claude
            carve_out,
            carve_into,
            carve_reason,
            add_ref,
            remove_ref,
            strict,
            force,
            human_only,
            no_human_only,
            // trace:TASK-1148 | ai:claude
            implementation_summary,
            risk_notes,
            test_coverage_notes,
            // trace:STORY-776 | ai:claude
            mode,
            // trace:STORY-634 | ai:claude
            origin,
            // trace:FR-283 | ai:claude
            weight,
            // trace:TASK-1117 | ai:claude
            interactive,
            ..
        } => {
            // trace:TASK-518 | ai:antigravity
            let mut resolved_id = id.clone();
            if let Some(pr) = parse_pr_arg(id) {
                let store = backend.load()?;
                let project_root = store_path
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;
                let resolved = resolve_pr_to_spec(project_root, pr, &store)?;
                println!("editing {} (backs {})", resolved, id);
                resolved_id = resolved;
            }
            let id = &resolved_id;

            // TASK-1176: naming the successor IS the supersede move — recording
            // a superseded-by edge without also flipping the status would leave
            // the two halves able to disagree (exactly the drift the string-tag
            // workaround suffered). So `--superseded-by <ID>` implies
            // `--status superseded`. An EXPLICIT `--status` still wins, so a
            // recovery edit (`--status approved --superseded-by X`) stays
            // expressible. trace:TASK-1176 | ai:claude
            let implied_status: Option<String> = if superseded_by.is_some() && status.is_none() {
                Some("superseded".to_string())
            } else {
                None
            };
            let status: &Option<String> = if implied_status.is_some() {
                &implied_status
            } else {
                status
            };

            // BUG-599: a malformed id is a typo, not an on-disk parse failure
            // — friendly format hint instead of the bare "Invalid spec_id
            // format" (or, via the lookup, the version-mismatch wall).
            // trace:BUG-599 | ai:claude
            if !is_spec_id_or_uuid(id) {
                return Err(not_found::invalid_spec_id_format(id));
            }
            // BUG-68: record activity AFTER successful lookup AND
            // after the TASK-47 terminal-status guard, so a refused
            // re-open doesn't leave a phantom edit entry.
            // trace:BUG-68 | ai:claude
            // BUG-1535: an ambiguous id REFUSES the edit (non-zero exit,
            // every candidate + its unambiguous handle listed); `aida list`
            // shows another spec under the same id, so a guess would silently
            // modify the wrong one. trace:BUG-1535 | ai:claude
            let mut req = backend
                .get_requirement_unambiguous(id)?
                .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;

            // TASK-47: refuse to re-open a Completed/Rejected req
            // without --force. Closing or idempotent re-flips stay
            // allowed; only Closed → Open transitions trip the guard.
            // Computed BEFORE the field mutations below so we read the
            // on-disk status, not the post-edit one.
            // trace:TASK-47 | ai:claude
            if let Some(new_status) = status {
                if is_terminal_status(&req.status) && !*force {
                    // BUG-751: same type-aware alias as the main status path
                    // below, so a terminal decision spec probed with
                    // `accepted` resolves consistently. trace:BUG-751 | ai:claude
                    let canonical = validate_status_input_for_type(new_status, &req.req_type)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    let mut probe = req.clone();
                    probe.set_status_from_str(canonical);
                    if !is_terminal_status(&probe.status) {
                        let spec_id = req.spec_id.as_deref().unwrap_or("?");
                        // BUG-671: keep the override flag on the FIRST line. In
                        // agent mode only the first line of an error survives as
                        // the TOON `error:` summary, so a `--force` hint on a
                        // continuation line is invisible to the agent that needs
                        // it. trace:BUG-671 | ai:claude
                        anyhow::bail!(
                            "{} is currently {}. Re-opening a closed requirement is \
                             usually a mistake — pass --force to override.\n  Otherwise, \
                             file a new requirement that supersedes {}.",
                            spec_id,
                            req.status,
                            spec_id
                        );
                    }
                }
            }

            // STORY-717: focus-scope drift guard at the In-Progress work-start
            // moment. Flipping a spec to In Progress in a focused worktree is
            // "starting work"; if the spec is outside the focus subtree, apply
            // the [focus] out_of_scope policy (warn nudges + proceeds, block
            // refuses without --force, off is silent). Only the genuine
            // transition INTO In Progress is a work-start — re-saving an
            // already-In-Progress spec or any other status edit is not.
            // trace:STORY-717 | ai:claude
            if let Some(status_str) = status {
                let starts_work = matches!(
                    status_str.to_lowercase().replace('-', "_").as_str(),
                    "in_progress" | "inprogress"
                ) && req.status != aida_core::RequirementStatus::InProgress;
                if starts_work {
                    if let Ok(project_root) = find_project_root() {
                        focus_scope_guard(&project_root, &backend, &req, *force)?;
                    }
                }
            }

            // STORY-48: lease enforcement. Find leases relative to the git
            // project root (parent of the orphan store), collect the target +
            // its ancestor chain for ancestor walking (BUG-634: targeted, not a
            // full-store load), and consult the [session].enforcement knob.
            // Best-effort — if we can't even find a project root, skip
            // enforcement rather than break edit.
            // trace:STORY-48 | ai:claude
            if let Ok(project_root) = find_project_root() {
                if !list_leases(&project_root).is_empty() {
                    // BUG-634: build only the target + its ancestor chain via
                    // targeted lookups — the lease walk needs no more — instead
                    // of a full-store load (a scan of every YAML on the write
                    // path). trace:BUG-634 | ai:claude
                    let store = collect_ancestor_store(&backend, &req);
                    // BUG-637: `--force` is the operator override that always
                    // permits an edit/reject of a live-claimed spec (the
                    // EPIC-54-reject escape hatch); `--strict` still escalates
                    // the live-claim warning into a hard block.
                    // trace:BUG-637 | ai:claude
                    enforce_session_lease(
                        &project_root,
                        &req,
                        &store,
                        "aida edit",
                        *strict,
                        *force,
                    )?;
                }
            }
            // BUG-68: all validation gates passed → safe to record the
            // edit. Uses the canonical spec_id from the resolved req
            // rather than the user's input (which might be a UUID or
            // an agreed_id). trace:BUG-68 | ai:claude
            record_role_activity(req.spec_id.as_deref().unwrap_or(id), "edit");

            // STORY-647 (team RBAC slice 2): a spec carrying any `[team]
            // protected_tags` entry may only be edited/transitioned by the
            // configured `protected_role` (advisor by default). Checked here,
            // before any field mutation, against the spec's CURRENT tags so a
            // non-advisor cannot edit OR un-protect a protected spec. `--force`
            // and advisor authority (TTY / live drain / advisor role) bypass.
            // Best-effort; no `[team] protected_tags` => no-op (slice-1 behavior).
            // trace:STORY-647 | ai:claude
            enforce_protected_spec_gate(req.tags.iter(), *force)?;

            // trace:BUG-1234 | ai:codex — mirror `aida add`'s safe long-form
            // sources after lookup/authorization, but before any mutation.
            let resolved_description =
                resolve_edit_description(description, description_from_file, *description_stdin)?;
            let description = &resolved_description;

            // TASK-1117: `aida edit <spec>` with NO field flags (or with
            // `--interactive`) opens the user's editor (AIDA_EDITOR → VISUAL
            // → EDITOR → platform default) on a git-commit-style markdown
            // buffer of title + description, and writes the parsed result
            // back through this same targeted single-spec path. Engages only
            // at a TTY — a piped/scripted no-flag call keeps the "No changes
            // specified" hint below. An aborted editor (nonzero exit), an
            // emptied buffer, or an unchanged buffer leaves the spec
            // untouched (no spurious commit).
            // trace:TASK-1117 | ai:claude
            let no_field_flags = title.is_none()
                && description.is_none()
                && status.is_none()
                && priority.is_none()
                && r#type.is_none()
                && owner.is_none()
                && feature.is_none()
                && tags.is_none()
                && add_tag.is_empty()
                && remove_tag.is_empty()
                && blocked_by.is_empty()
                && remove_blocked_by.is_empty()
                // trace:TASK-1176 | ai:claude
                && superseded_by.is_none()
                // trace:STORY-1434 | ai:claude
                && carve_out.is_none()
                && add_ref.is_empty()
                && remove_ref.is_empty()
                && !*human_only
                && !*no_human_only
                && implementation_summary.is_none()
                && risk_notes.is_none()
                && test_coverage_notes.is_none()
                && mode.is_none()
                // trace:STORY-634 | ai:claude
                && origin.is_none()
                // trace:FR-283 | ai:claude
                && weight.is_none();
            let mut editor_title: Option<String> = None;
            let mut editor_description: Option<String> = None;
            if (no_field_flags || *interactive)
                && std::io::IsTerminal::is_terminal(&std::io::stdin())
                && std::io::IsTerminal::is_terminal(&std::io::stdout())
            {
                let display_id = req.spec_id.as_deref().unwrap_or(id);
                match edit_buffer::edit_spec_in_editor(display_id, &req.title, &req.description)? {
                    Some((new_title, new_description)) => {
                        if new_title != req.title {
                            editor_title = Some(new_title);
                        }
                        if new_description != req.description {
                            editor_description = Some(new_description);
                        }
                        if no_field_flags && editor_title.is_none() && editor_description.is_none()
                        {
                            println!("No changes — {} left untouched.", display_id);
                            return Ok(());
                        }
                    }
                    None => {
                        println!("Empty buffer — {} left untouched.", display_id);
                        return Ok(());
                    }
                }
            }
            let title = if editor_title.is_some() {
                &editor_title
            } else {
                title
            };
            let description = if editor_description.is_some() {
                &editor_description
            } else {
                description
            };

            let mut changed = false;
            let mut force_dropped_structural_tags: Vec<String> = Vec::new();
            // STORY-1434: (target uuid, target display id) once the
            // carve-out target has been resolved+validated and the edge +
            // comment folded into `req` below — carried past the single
            // save so the inverse edge can be written on the target
            // afterwards. trace:STORY-1434 | ai:claude
            let mut carve_target: Option<(uuid::Uuid, String)> = None;
            // TASK-1450: before/after pairs for the seat-tagged coordination
            // events emitted once the edit is durably written — captured here
            // (not reconstructed after the fact) so the recorded values are
            // exactly what this invocation changed, not a re-derivation from
            // the saved requirement. trace:TASK-1450 | ai:claude
            let mut disposition_event: Option<(String, String)> = None;
            let mut execution_mode_event: Option<(Option<String>, Option<String>)> = None;
            if let Some(t) = title {
                req.title = t.clone();
                changed = true;
            }
            if let Some(d) = description {
                req.description = d.clone();
                changed = true;
            }
            // STORY-1434: carve a named criterion out of the description in
            // the SAME edit that records where it now lives, so the stale
            // text and its correction can never drift apart the way a
            // hand-edit + a separate, easy-to-miss comment did. Applied to
            // whatever description is current at this point, so `--carve-out`
            // composes with a plain `--description` pass in one invocation.
            // Refuses (instead of silently no-op'ing) when the text isn't
            // found verbatim — a criterion carved from the wrong wording is
            // worse than an explicit error.
            //
            // Atomicity: the target is resolved and validated FIRST — via
            // `get_requirement_unambiguous` (BUG-1535), so a `--carve-into`
            // id that names two specs refuses instead of guessing — and the
            // edge + the CARVE-OUT comment are folded into THIS `req` before
            // the single save below, alongside the struck description. One
            // write, not a description change plus a separate follow-up
            // write that could land only half-applied. trace:STORY-1434
            if let Some(criterion) = carve_out {
                let target_id_arg = carve_into
                    .as_deref()
                    .expect("clap requires --carve-into with --carve-out");
                let target = backend
                    .get_requirement_unambiguous(target_id_arg)?
                    .ok_or_else(|| {
                        not_found::requirement_not_found(target_id_arg, Some(store_path))
                    })?;
                if target.id == req.id {
                    anyhow::bail!("a spec cannot carve a criterion into itself ({})", id);
                }
                let target_display = target
                    .spec_id
                    .clone()
                    .unwrap_or_else(|| target_id_arg.to_string());
                let source_display = req.spec_id.as_deref().unwrap_or(id).to_string();
                match carve_out_description(&req.description, criterion, &target_display) {
                    Some(new_description) => {
                        req.description = new_description;
                        apply_carve_out(
                            &mut req,
                            target.id,
                            &source_display,
                            &target_display,
                            criterion,
                            carve_reason.as_deref(),
                        );
                        changed = true;
                        carve_target = Some((target.id, target_display));
                    }
                    None => anyhow::bail!(
                        "--carve-out text not found verbatim in {}'s description — copy the \
                         exact wording (including case and punctuation) from `aida show {}`.",
                        source_display,
                        source_display
                    ),
                }
            }
            // TASK-1148: the three optional narrative fields. An empty string
            // clears the field (stores None so it drops out of the YAML); any
            // other value sets it. trace:TASK-1148 | ai:claude
            let set_narrative = |slot: &mut Option<String>, arg: &Option<String>| -> bool {
                match arg {
                    Some(text) if text.is_empty() => {
                        if slot.is_some() {
                            *slot = None;
                            return true;
                        }
                        false
                    }
                    Some(text) => {
                        *slot = Some(text.clone());
                        true
                    }
                    None => false,
                }
            };
            changed |= set_narrative(&mut req.implementation_summary, implementation_summary);
            changed |= set_narrative(&mut req.risk_notes, risk_notes);
            changed |= set_narrative(&mut req.test_coverage_notes, test_coverage_notes);
            // FR-283: the numeric weight/score. An empty string clears it
            // (drops out of the YAML); any other value must parse as a finite
            // number. trace:FR-283 | ai:claude
            if let Some(w) = weight {
                if w.trim().is_empty() {
                    if req.weight.is_some() {
                        req.weight = None;
                        changed = true;
                    }
                } else {
                    let parsed: f32 = w.trim().parse().map_err(|_| {
                        anyhow::anyhow!(
                            "--weight expects a number (e.g. 0.75 or 42), got `{w}`; \
                             pass an empty string to clear"
                        )
                    })?;
                    if !parsed.is_finite() {
                        anyhow::bail!("--weight must be a finite number (got {w})");
                    }
                    if req.weight != Some(parsed) {
                        req.weight = Some(parsed);
                        changed = true;
                    }
                }
            }
            // STORY-776: execution_mode is the advisor's bless-time routing
            // judgment — writing (or clearing) it is an advisor-authority act,
            // same gate as a status promotion, so the audit trail (the store
            // commit) records who classified it and when. An empty string
            // clears back to ungroomed. trace:STORY-776 | ai:claude
            if let Some(m) = mode {
                if !has_advisor_authority() {
                    anyhow::bail!(
                        "--mode sets the advisor's routing classification and needs advisor \
                         authority (advisor role or an interactive session). Leave it for \
                         advisor grooming, or run as the advisor.{}",
                        team_role_refusal_clause()
                    );
                }
                let mode_before = req.execution_mode.map(|m| m.to_string());
                if m.trim().is_empty() {
                    if req.execution_mode.is_some() {
                        req.execution_mode = None;
                        changed = true;
                        execution_mode_event = Some((mode_before, None));
                    }
                } else {
                    let parsed: aida_core::ExecutionMode =
                        m.parse().map_err(|e: String| anyhow::anyhow!(e))?;
                    if req.execution_mode != Some(parsed) {
                        req.execution_mode = Some(parsed);
                        changed = true;
                        execution_mode_event = Some((mode_before, Some(parsed.to_string())));
                    }
                }
            }
            // STORY-634 (ADR-12): the multi-repo origin dimension. An empty
            // string clears it (drops out of the YAML); any other value must
            // parse as "repo" or "repo/component" and the repo slug must name
            // a repo in the workspace manifest — the canonical join-key
            // vocabulary linkage resolves against.
            if let Some(o) = origin {
                if o.trim().is_empty() {
                    if req.origin.is_some() {
                        req.origin = None;
                        changed = true;
                    }
                } else {
                    let parsed =
                        aida_core::Origin::parse(o).map_err(|e: String| anyhow::anyhow!(e))?;
                    let project_root = store_path.parent().ok_or_else(|| {
                        anyhow::anyhow!("cannot derive project root from store path")
                    })?;
                    match aida_core::workspace::WorkspaceManifest::discover(project_root) {
                        Some((_, manifest)) => {
                            let slugs = manifest.repo_slugs();
                            if !slugs.iter().any(|s| s.eq_ignore_ascii_case(&parsed.repo)) {
                                anyhow::bail!(
                                    "origin repo `{}` is not in the workspace manifest \
                                     (known repos: {})",
                                    parsed.repo,
                                    slugs.join(", ")
                                );
                            }
                        }
                        None => anyhow::bail!(
                            "--origin needs a multi-repo workspace manifest \
                             (.aida-workspace) to validate the repo slug against; \
                             single-repo stores leave origin unset"
                        ),
                    }
                    if req.origin.as_ref() != Some(&parsed) {
                        req.origin = Some(parsed);
                        changed = true;
                    }
                }
            }
            let mut new_status_for_manifest: Option<String> = None;
            // STORY-738: did this edit transition the spec INTO Completed?
            // Captured against the prior status before the set below so the
            // human render path can fire the completion crescendo instead of
            // the flat `Updated:` line. trace:STORY-738 | ai:claude
            let mut into_completed = false;
            // TASK-358: triage out of NeedsAttention — captured here, applied
            // after the backend save below. trace:TASK-358 | ai:claude
            let mut left_needs_attention = false;
            if let Some(s) = status {
                // BUG-751: type-aware — a decision spec (ADR) may be moved to
                // its accepted state with the ADR-native verb `accepted`,
                // which records as `Approved`. trace:BUG-751 | ai:claude
                let canonical = validate_status_input_for_type(s, &req.req_type)
                    .map_err(|e| anyhow::anyhow!(e))?;
                // BUG-626: an EPIC's status is a read-only ROLLUP of its
                // children, not a manually-set field. Reject ALL manual epic
                // status edits (substrate-as-bouncer — this is exactly the
                // hygiene drift a confident setter reintroduces: childless epics
                // left In Progress, shipping-children epics left Draft). The
                // displayed status is derived everywhere (list/show/why/status);
                // `--force` is the recovery escape. This generalizes the
                // pre-existing "Epic specs cannot be promoted to Approved" guard
                // (TASK-761) to every transition. trace:BUG-626 | ai:claude
                if manual_epic_status_edit_forbidden(&req.req_type, *force) {
                    anyhow::bail!(
                        "an epic's status is a read-only rollup of its children, not set by \
                         hand — it moves to In Progress when a child starts, to Done/Completed \
                         when all children finish, and back to Draft when it has no active \
                         children. Change the children's statuses instead (`aida graph tree {}` \
                         shows the rollup). Use --force only for recovery.",
                        req.agreed_id
                            .clone()
                            .or_else(|| req.spec_id.clone())
                            .unwrap_or_else(|| req.id.to_string())
                    );
                }
                // STORY-332: enforce the NeedsAttention transition rules —
                // into it only from In Progress (use `aida punt`), out of it
                // only to Approved / In Progress / Rejected. Every other
                // transition is unconstrained, so existing edits don't regress.
                if let Some(msg) =
                    forbidden_attention_transition(&req.status, &parse_status(canonical)?)
                {
                    anyhow::bail!(msg);
                }
                // TASK-647 (ADR-3): advisor-gate the draft→approved promotion.
                // A non-advisor, non-TTY caller may not lift a draft into the
                // approved+ pipeline — that's the intake-triage decision the
                // advisor (or an interactive human) owns. Execution flips
                // (Approved→InProgress→Done) are NOT gated, so drains are
                // unaffected. trace:TASK-647 | ai:claude
                //
                // BUG-482: gate `NeedsAttention` as a source too. A punted spec
                // sits in NeedsAttention awaiting advisor triage;
                // `forbidden_attention_transition` permits NeedsAttention →
                // Approved as a triage outcome, but *who* may make that triage
                // call is still advisor authority. Without NeedsAttention in
                // the source set, a non-advisor could self-re-approve a spec it
                // (or the orchestrator) just punted — bypassing the human/advisor
                // triage the punt exists to request. trace:BUG-482 | ai:claude
                let new_status = parse_status(canonical)?;
                if status_advance_requires_advisor_authority(&req.status, &new_status) {
                    // trace:TASK-761 | ai:codex
                    if matches!(new_status, RequirementStatus::Approved)
                        && approval_forbidden_for_type(&req.req_type)
                    {
                        anyhow::bail!(
                            "{} specs cannot be promoted to {}. Leave this class outside the \
                             approved execution pipeline.",
                            req.req_type,
                            canonical
                        );
                    }
                    if !has_advisor_authority() {
                        anyhow::bail!(
                            "promoting a {} spec to {} needs advisor authority (advisor role or \
                             an interactive session). Leave it for advisor triage, or run as \
                             the advisor.{}{}",
                            req.status,
                            canonical,
                            team_role_refusal_clause(),
                            roleless_recovery_sentence()
                        );
                    }
                    // BUG-498: a gated promotion that went through is advisor
                    // work — nudge the operator to seat the advisor role if
                    // they're acting via an env prefix. trace:BUG-498 | ai:claude
                    maybe_hint_advisor_seat();
                }
                let was_needs_attention = matches!(req.status, RequirementStatus::NeedsAttention);
                // STORY-738: capture the into-Completed transition against the
                // prior status (before the set). trace:STORY-738 | ai:claude
                into_completed = is_into_completed_transition(&req.status, canonical);
                // TASK-1450: disposition before/after for the coordination
                // event, captured against the on-disk status before the set
                // below (same "before the mutation" rule STORY-738 uses).
                // trace:TASK-1450 | ai:claude
                let status_before = req.status.to_string();
                // STORY-1418: an into-Completed edit stamps through the seam;
                // the ship record is emitted below once the write lands.
                // trace:STORY-1418 | ai:claude
                if canonical == "Completed" {
                    crate::completion::mark_completed(&mut req);
                } else {
                    req.set_status_from_str(canonical);
                }
                disposition_event = Some((status_before, req.status.to_string()));
                // TASK-1446: a spec deliberately reopened to Draft after its
                // trailered commit already landed must not be flipped right
                // back to Done by the next pull — `pre_sha=None` scans a
                // `--max-count=50 HEAD` window that can still contain the
                // same old commit. Stamp the code-repo HEAD sha at reopen
                // time in its own field (not `completion_sha`, which the
                // Done→Completed bump owns for BUG-410) so the Draft-landing
                // guard can skip any candidate at or before this sha.
                // trace:TASK-1446 | ai:claude
                if matches!(req.status, RequirementStatus::Draft) {
                    if let Ok(project_root) = find_project_root() {
                        if let Ok(sha) = aida_core::git_ops::head_sha(&project_root) {
                            req.implementation_info
                                .get_or_insert_with(aida_core::ImplementationInfo::default)
                                .reopened_at_sha = Some(sha);
                        }
                    }
                }
                // STORY-332 / EPIC-28: a spec triaged out of NeedsAttention is
                // no longer paused — drop the now-stale punt metadata AND any
                // orchestrator-shelving metadata. The punt ledger
                // (`.aida/punts.jsonl`) keeps the durable history for both.
                // trace:EPIC-28 | ai:claude
                //
                // TASK-1311: the clearing lives in `requeue::clear_shelve_markers`
                // so `aida edit` and `aida queue rework` return a spec to flight
                // identically. It also drops the `needs-human` tag an advisor
                // escalation wrote, which is a burndown parking tag: without
                // that, a triaged spec stayed out of the drain for good. The
                // comment records why the spec came back (auditable re-entry).
                // trace:TASK-1311 | ai:claude
                if was_needs_attention && !matches!(req.status, RequirementStatus::NeedsAttention) {
                    // The escalation tag is a hand-off to a human; only a
                    // human at a terminal may clear it (not a non-TTY advisor
                    // agent, not an orchestrated phase).
                    let cleared = crate::requeue::clear_shelve_markers(
                        &mut req,
                        crate::requeue::caller_may_clear_escalation(),
                    );
                    if cleared != crate::requeue::ClearedMarkers::default() {
                        let note = cleared.audit_note(
                            "`aida edit --status`",
                            &req.status.to_string(),
                            None,
                        );
                        req.add_comment(aida_core::Comment::new(get_default_author(), note));
                    }
                    if !cleared.removed_tags.is_empty() {
                        eprintln!(
                            "  {} cleared stale parking tag(s) {} so the drain can pick it up again",
                            "·".dimmed(),
                            cleared.removed_tags.join(", ")
                        );
                    }
                    if let Some(w) = crate::requeue::kept_escalation_warning(
                        req.spec_id.as_deref().unwrap_or(id),
                        &cleared,
                    ) {
                        eprintln!("  {} {w}", "Warning:".yellow().bold());
                    }
                    left_needs_attention = true;
                }
                changed = true;
                new_status_for_manifest = Some(canonical.to_string());
            }
            if let Some(p) = priority {
                let canonical = validate_priority_input(p).map_err(|e| anyhow::anyhow!(e))?;
                req.set_priority_from_str(canonical);
                changed = true;
            }
            if let Some(t) = r#type {
                // BUG-48: an invalid --type used to be silently dropped,
                // making the user think they passed no flags ("No changes
                // specified" — misleading). Propagate the error so they
                // see "Unknown requirement type: <input>" with the same
                // valid-list hint as --status / --priority.
                let rt = parse_requirement_type(t).map_err(|e| {
                    anyhow::anyhow!(
                        "{} — expected one of: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc",
                        e
                    )
                })?;
                // BUG-49: spec_id is stable; type changes don't auto-
                // renumber. Warn when the new type implies a different
                // prefix so the user isn't surprised by FR-4 still saying
                // FR-4 after `--type principle`.
                if let Some(ref sid) = req.spec_id {
                    let new_prefix = rt.default_prefix();
                    let current_prefix = sid.split('-').next().unwrap_or("");
                    if !current_prefix.is_empty() && current_prefix != new_prefix {
                        eprintln!(
                            "{} type changed to `{}` but spec_id stays `{}` — \
                             AIDA spec_ids are stable so existing trace comments \
                             and commit refs keep working. To get a `{}-N` id, \
                             recreate the requirement with the new type.",
                            "Note:".dimmed(),
                            t,
                            sid,
                            new_prefix
                        );
                    }
                }
                req.req_type = rt;
                changed = true;
            }
            if let Some(o) = owner {
                req.owner = o.clone();
                changed = true;
            }
            if let Some(f) = feature {
                req.feature = f.clone();
                changed = true;
            }
            if let Some(t) = tags {
                // BUG-545: `--tags` REPLACES the whole set — a frequent footgun
                // when someone means "add one tag". Warn loudly (stderr) showing
                // old→new so a silent clobber of provenance/routing tags is
                // visible. `--add-tag` / `--remove-tag` are the incremental forms
                // (kept mutually exclusive with `--tags` by clap's conflicts_with).
                // trace:BUG-545 | ai:claude
                let old_tags = req.tags.clone();
                req.tags.clear();
                for tag in t.split(',') {
                    let trimmed = tag.trim();
                    if !trimmed.is_empty() {
                        req.tags.insert(trimmed.to_string());
                    }
                }
                // trace:BUG-1252 | ai:codex
                force_dropped_structural_tags =
                    enforce_structural_tag_replacement(&old_tags, &req.tags, *force)?;
                if let Some(msg) = tags_replace_warning(&old_tags, &req.tags) {
                    eprintln!(
                        "  {} {}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                        msg
                    );
                }
                changed = true;
            }
            // TASK-351: partial tag edits that don't clobber the rest.
            // `--tags` is full-replace; `--add-tag` / `--remove-tag` are
            // additive/subtractive. Clap's `conflicts_with` keeps the two
            // forms mutually exclusive. Adding a present tag or removing
            // an absent one is a graceful no-op.
            // trace:TASK-351 | ai:claude
            if apply_tag_deltas(&mut req.tags, add_tag, remove_tag) {
                changed = true;
            }
            // TASK-524: typo guard — a `lifecycle:*` tag that isn't a recognized
            // short-circuit tag silently no-ops at drain time (it's just an
            // unread string). Warn on any misspelling the user passed via
            // `--tags` or `--add-tag` so it's caught at edit time, not when a
            // drain quietly fails to skip the phase. trace:TASK-524 | ai:claude
            {
                let passed: Vec<String> = tags
                    .iter()
                    .flat_map(|t| t.split(',').map(|s| s.trim().to_string()))
                    .chain(add_tag.iter().cloned())
                    .filter(|s| !s.is_empty())
                    .collect();
                for tag in &passed {
                    if auto_complete::is_unrecognized_lifecycle_tag(tag) {
                        eprintln!(
                            "  {} unrecognized lifecycle tag `{}` — it will NOT short-circuit \
                             any phase. Valid: {}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                            tag,
                            auto_complete::RECOGNIZED_LIFECYCLE_TAGS.join(", ")
                        );
                    }
                }
            }
            // STORY-333: `--human-only` / `--no-human-only` flip the typed
            // marker that the pickability gate consults. Clap's
            // `conflicts_with` keeps the two flags mutually exclusive, so
            // at most one branch fires per invocation.
            // trace:STORY-333 | ai:claude
            if *human_only && !req.human_only {
                req.human_only = true;
                changed = true;
            }
            if *no_human_only && req.human_only {
                req.human_only = false;
                changed = true;
            }

            // STORY-476: one-way external issue refs. `--add-ref` validates
            // each `provider:id` (linear/jira/github) before storing the
            // canonical form; an invalid ref is a hard error so a typo'd ref
            // never lands silently. `--remove-ref` drops a stored ref by its
            // canonical form. Both are repeatable and idempotent.
            // trace:STORY-476 | ai:claude
            for raw in add_ref {
                let parsed =
                    aida_core::external_refs::parse_ref(raw).map_err(|e| anyhow::anyhow!(e))?;
                let canonical = parsed.canonical();
                if !req.external_refs.contains(&canonical) {
                    req.external_refs.push(canonical);
                    changed = true;
                }
            }
            for raw in remove_ref {
                // Accept either the canonical or a loosely-typed form by
                // normalizing through the parser when possible; fall back to a
                // trimmed literal match so a stored-but-now-unknown provider
                // can still be removed.
                let target = aida_core::external_refs::parse_ref(raw)
                    .map(|p| p.canonical())
                    .unwrap_or_else(|_| raw.trim().to_string());
                let before = req.external_refs.len();
                req.external_refs.retain(|r| r != &target);
                if req.external_refs.len() != before {
                    changed = true;
                }
            }

            if changed {
                req.modified_at = chrono::Utc::now();
                if force_dropped_structural_tags.is_empty() {
                    backend.update_requirement(&req)?;
                } else {
                    backend.bulk_update(
                        std::slice::from_ref(&req),
                        &format!(
                            "update {}: replaced tags, dropped: {}",
                            req.spec_id.as_deref().unwrap_or(id),
                            force_dropped_structural_tags.join(", ")
                        ),
                    )?;
                }
                // TASK-1450: a disposition (status) or execution_mode change
                // made through `aida edit` is a coordination-seat decision —
                // approve/reject/defer and groom's `--mode` writes all land
                // here (groom/approve/reject shell out to `aida edit`
                // themselves). Emit AFTER the write above lands, mirroring
                // BUG-1423's PrMerged placement, so a failed write never
                // produces a phantom event. Best-effort — `events::emit`
                // itself never fails the edit. trace:TASK-1450 | ai:claude
                if let Some(project_root) = store_path.parent() {
                    let display_id = req.spec_id.as_deref().unwrap_or(id);
                    if let Some((before, after)) = disposition_event.take() {
                        emit_edit_disposition_changed(project_root, display_id, before, after);
                    }
                    if let Some((before, after)) = execution_mode_event.take() {
                        emit_edit_execution_mode_changed(project_root, display_id, before, after);
                    }
                }
                // STORY-738: a transition INTO Completed is the payoff state —
                // render the felt completion crescendo instead of the flat
                // generic `Updated:` line (reused for any tag edit). Only the
                // human surface gets it; the agent/TOON path and every
                // non-completion edit keep `Updated:`. The workflow-hint line
                // below is unaffected. trace:STORY-738 | ai:claude
                // BUG-1286 F1: `aida edit --status completed` is the third
                // into-Completed path and emitted nothing. `into_completed` was
                // already computed here for the STORY-738 render against the
                // PRIOR status, so the transition test is reused rather than
                // recomputed. trace:BUG-1286 | ai:claude
                if into_completed {
                    if let Some(project_root) = store_path.parent() {
                        crate::completion::emit_spec_completed(
                            project_root,
                            req.spec_id.as_deref().unwrap_or(id),
                            "",
                            None,
                            "edit",
                        );
                    }
                }
                match edit_completion_render(into_completed, agent_output_mode()) {
                    EditCompletionRender::Crescendo => {
                        let display_id = req.spec_id.as_deref().unwrap_or(id);
                        render_completion_crescendo(display_id, &req.title);
                    }
                    EditCompletionRender::Updated => println!("Updated: {}", id),
                }

                // TASK-928 (SPIKE-71): a tag edit that introduces a
                // `parent:<SPEC-ID>` tag must materialize the real bidirectional
                // edge too (same gap as `aida add`). Only fires when tags were
                // touched. Additive + lenient: tag stays, bad target = no-op.
                // trace:TASK-928 | ai:claude
                if tags.is_some() || !add_tag.is_empty() {
                    if let Some(sid) = req.spec_id.as_deref() {
                        match ensure_parent_edge_from_tag(&backend, sid) {
                            Ok(Some(pdisp)) => {
                                println!(
                                    "  Linked: {} → parent of {} (from parent: tag)",
                                    pdisp, sid
                                )
                            }
                            Ok(None) => {}
                            Err(e) => eprintln!(
                                "  {} could not link parent from tag: {}",
                                "Warning:".yellow().bold(),
                                e
                            ),
                        }
                    }
                }

                // STORY-98: if this edit changed the status AND a session
                // manifest covers the active session, flip the manifest
                // row so `aida session show --plan` and the
                // `[planned:by-X]` chip stay coherent with the store.
                // Best-effort — manifest errors don't fail the edit.
                // trace:STORY-98 | ai:claude
                if let Some(status_str) = &new_status_for_manifest {
                    if let Some(spec_id) = req.spec_id.as_deref() {
                        update_manifest_for_status(spec_id, status_str);
                    }
                }

                // STORY-106: workflow hint when a status flip to Completed
                // emptied the queue for the active role+scope. Mirrors the
                // queue-done hint so direct `aida edit --status completed`
                // surfaces the same Next-step. Best-effort.
                // trace:STORY-106 | ai:claude
                if new_status_for_manifest.as_deref() == Some("Completed") {
                    let storage = Storage::new(store_path);
                    let user_id = current_user_id(None);
                    maybe_hint_after_queue_drain(&storage, &user_id);
                }

                // BUG-378: substrate-as-bouncer — direct `aida edit --status
                // done|completed` is the other path an agent uses to declare
                // work shipped, so the pending-brief banner must fire here
                // too. Mirrors the queue-done hookup. trace:BUG-378 | ai:claude
                if matches!(
                    new_status_for_manifest.as_deref(),
                    Some("Done") | Some("Completed")
                ) {
                    if let Ok(project_root) = find_project_root() {
                        warn_pending_briefs_for_running_agent(&project_root);
                    }
                }

                // BUG-238: file plan `## Followups` when `aida edit` directly
                // transitions a spec into Done/Completed. `aida queue done`
                // and the STORY-86 auto-bump already trigger the parse; the
                // direct-edit path (used by `/aida-review` step 10) was the
                // hole — followups were silently lost. Idempotent via the
                // [aida:followups] marker, so whichever path runs first wins.
                // Interactive only when stdin is a TTY; non-TTY (skill /
                // script context) files all bullets so the silent-loss case
                // that motivated this fix actually surfaces them.
                // trace:BUG-238 | ai:claude
                if matches!(
                    new_status_for_manifest.as_deref(),
                    Some("Done") | Some("Completed")
                ) {
                    if let (Ok(project_root), Some(spec_id)) =
                        (find_project_root(), req.spec_id.as_deref())
                    {
                        let storage = Storage::new(store_path);
                        let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
                        if let Err(e) = extract_plan_followups(
                            &storage,
                            &project_root,
                            spec_id,
                            spec_id,
                            interactive,
                        ) {
                            eprintln!(
                                "{} followup extraction skipped: {}",
                                "Warning:".yellow().bold(),
                                e
                            );
                        }
                    }
                }

                // TASK-358: triage out of NeedsAttention — clean up any
                // orchestrator-escalated worktree for this spec. The lease's
                // `escalated_to_human` marker (stamped by the
                // `--escalate-blocks` path) is the safety gate: an
                // interactive user session on the same spec, or an
                // `--escalate-defaults` advisor-resume, has the marker
                // absent and is left alone. trace:TASK-358 | ai:claude
                if left_needs_attention {
                    if let Ok(project_root) = find_project_root() {
                        let spec_id = req.spec_id.as_deref().unwrap_or(id);
                        cleanup_escalated_leases_for_spec(&project_root, spec_id);
                        // BUG-674: a spec triaged out of NeedsAttention no
                        // longer has an open punt awaiting triage — close its
                        // ledger record so the punt ledger reflects reality
                        // instead of a permanent "awaiting triage" row.
                        // trace:BUG-674 | ai:claude
                        let _ = punt::close_open_records(
                            &project_root,
                            spec_id,
                            punt::RESOLUTION_HUMAN_RESOLVED,
                            "resolved",
                            None,
                            Some("resume"),
                        );
                    }
                }
            } else if blocked_by.is_empty()
                && remove_blocked_by.is_empty()
                // trace:TASK-1176 | ai:claude
                && superseded_by.is_none()
                // trace:STORY-1434 | ai:claude
                && carve_out.is_none()
            {
                println!("No changes specified. Use --title, --status, --priority, etc.");
            }

            // TASK-1176: record the supersede lineage AFTER the scalar save,
            // for the same reason the blocked-by block below does — the helper
            // re-reads the persisted spec, so writing the edge first would be
            // clobbered by the status save. trace:TASK-1176 | ai:claude
            if let Some(successor) = superseded_by {
                let edit_spec = req.spec_id.as_deref().unwrap_or(id).to_string();
                match crate::add_superseded_by_edge(&backend, &edit_spec, successor) {
                    Ok(disp) => println!("  Superseded by: {}", disp.cyan()),
                    Err(e) => eprintln!(
                        "  {} could not record superseded-by {}: {}",
                        "Warning:".yellow().bold(),
                        successor,
                        e
                    ),
                }
            } else if matches!(req.status, RequirementStatus::Superseded)
                && !req.relationships.iter().any(|r| {
                    matches!(
                        r.rel_type,
                        aida_core::models::RelationshipType::SupersededBy
                    )
                })
            {
                // TASK-1176: "superseded" without a named successor is a
                // half-recorded move — the state says something replaced this,
                // but nothing says WHAT. Nudge rather than refuse: the successor
                // may not be filed yet. trace:TASK-1176 | ai:claude
                eprintln!(
                    "{}",
                    format!(
                        "  Note: no successor recorded — add one with `aida edit {} --superseded-by <SPEC-ID>`.",
                        req.spec_id.as_deref().unwrap_or(id)
                    )
                    .dimmed()
                );
            }

            // STORY-1434: the carved-out-to edge + CARVE-OUT comment already
            // landed on `req` and were saved atomically above (with the
            // struck description); only the INVERSE carved-from edge on the
            // TARGET remains, written after the source's save the same way
            // the blocked-by/supersede edges below are. `target` is
            // re-fetched by UUID — never re-resolved from the user-typed
            // `--carve-into` id — so this cannot rediscover an ambiguity;
            // a failure here (backend error, target vanished) is a real
            // error, not a warning: a one-sided carve-out (source struck, no
            // reciprocal edge) is exactly the half-applied state this spec
            // exists to prevent. trace:STORY-1434 | ai:claude
            if let Some((target_id, target_display)) = carve_target {
                let source_display = req.spec_id.as_deref().unwrap_or(id).to_string();
                crate::add_carved_from_edge(
                    &backend,
                    target_id,
                    &target_display,
                    req.id,
                    &source_display,
                )?;
                println!("  Carved out → {}", target_display.cyan());
            }

            // STORY-446: apply blocked-by edge add/remove AFTER any scalar edit
            // has been saved, so the helper operates on the persisted spec
            // (avoiding a save that would clobber the new edge). Runs whether or
            // not a scalar field changed. trace:STORY-446 | ai:claude
            if !blocked_by.is_empty() || !remove_blocked_by.is_empty() {
                let edit_spec = req.spec_id.as_deref().unwrap_or(id).to_string();
                for blocker in blocked_by {
                    match add_blocked_by_edge(&backend, &edit_spec, blocker) {
                        Ok(disp) => println!("  Blocked by: {}", disp.cyan()),
                        Err(e) => eprintln!(
                            "  {} could not add blocked-by {}: {}",
                            "Warning:".yellow().bold(),
                            blocker,
                            e
                        ),
                    }
                }
                for blocker in remove_blocked_by {
                    match remove_blocked_by_edge(&backend, &edit_spec, blocker) {
                        Ok(disp) => println!("  Removed blocked-by: {}", disp.cyan()),
                        Err(e) => eprintln!(
                            "  {} could not remove blocked-by {}: {}",
                            "Warning:".yellow().bold(),
                            blocker,
                            e
                        ),
                    }
                }
            }

            // TASK-974 (AXI #9): agent-mode next-step block — the valid next
            // transition(s) for the spec's (now-current) status, templated with
            // its id. e.g. after `aida edit X --status approved` -> suggest
            // `aida queue work X`. The human TTY path is unchanged. trace:TASK-974
            if agent_output_mode() {
                let sid = req.spec_id.as_deref().unwrap_or(id);
                let next = crate::help_next::spec_next(&req.status.to_string(), sid);
                if let Some(block) = crate::help_next::render(&next) {
                    println!("{block}");
                }
            }
        }
        Command::Del { id, yes } => {
            let req = backend
                .get_requirement_unambiguous(id)?
                .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;

            if !yes {
                println!("Delete {} — {}?", id, req.title);
                print!("Confirm (y/N): ");
                use std::io::Write;
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Cancelled.");
                    return Ok(());
                }
            }

            backend.delete_requirement(&req.id)?;
            println!("Deleted: {}", id);
        }
        Command::Findings { cmd: findings_cmd } => {
            // STORY-732 (FIX 8): bare `aida findings` defaults to `findings list`
            // — the command every failure message tells the user to run, so it
            // must not be a hard clap error. Mirrors the `aida questions` pattern.
            // trace:STORY-732 | ai:claude
            let default_list = FindingsCommand::List {
                pr: None,
                source: None,
                kind: None,
                count: false,
                json: false,
            };
            let cmd = findings_cmd.as_ref().unwrap_or(&default_list);
            handle_findings_command(cmd, &backend, store_path)?;
        }
        Command::ImportPlan {
            file,
            spec,
            request_review,
        } => {
            import_plan_cmd::handle_import_plan_command(
                &backend,
                store_path,
                file,
                spec.as_deref(),
                *request_review,
            )?;
        }
        Command::Questions { cmd } => {
            handle_questions_command(cmd.as_ref(), &backend, store_path)?;
        }
        // trace:TASK-779 | ai:claude
        Command::Decide { spec } => {
            decide_cmd::handle_decide_command(&backend, store_path, spec)?;
        }
        Command::Research {
            id,
            dry_run,
            artifact_dir,
        } => {
            handle_research_command(&backend, store_path, id, *dry_run, artifact_dir)?;
        }
        // Bare `aida human [--short]` lists the three pending-for-human
        // buckets. The presence/unblock subcommands already returned via the
        // early pre-storage dispatch; the STORY-611 action aliases
        // (`answer`/`review`/`decide`) reach here because they need the
        // backend, and delegate to their canonical verbs.
        // trace:TASK-746 trace:STORY-563 trace:STORY-611
        Command::Human { short, command } => match command {
            // STORY-611: `aida human answer|decide <spec> <choice>` →
            // `aida questions answer` (pure pass-through).
            Some(cli::HumanCommand::Answer { spec, choice, note })
            | Some(cli::HumanCommand::Decide { spec, choice, note }) => {
                questions_answer_one(&backend, store_path, spec, choice, note.as_deref())?;
            }
            // STORY-611: `aida human review <spec>` → `aida review <spec>`.
            Some(cli::HumanCommand::Review {
                spec,
                no_agent,
                allow_stale_base,
                guided,
            }) => {
                if *guided {
                    handle_guided_human_review(spec)?;
                } else {
                    handle_review_spec(&backend, store_path, spec, *no_agent, *allow_stale_base)?;
                }
            }
            _ => {
                human_cmd::handle_human_command(*short, &backend)?;
            }
        },
        Command::Punt {
            id,
            category,
            reason,
            lean,
        } => {
            handle_punt_command(id, category, reason, lean.as_deref(), &backend, store_path)?;
        }
        Command::Punts(punts_cmd) => {
            handle_punts_command(punts_cmd.clone())?;
        }
        Command::Autonomy { command } => {
            autonomy_cmd::handle_autonomy_command(command.as_ref())?;
        }
        Command::Archive {
            id,
            older_than,
            status,
            dry_run,
            force,
            verbose,
        } => {
            // STORY-441: archive a single spec OR bulk-sweep over closed
            // work matching the duration + status filter. trace:STORY-441
            // BUG-492: `force` opts past the non-terminal/queued guard.
            archive_cmd::handle_archive_command(
                id.as_deref(),
                older_than.as_deref(),
                status.as_deref(),
                *dry_run,
                *force,
                *verbose,
                &backend,
                store_path,
            )?;
        }
        Command::Unarchive { id } => {
            // STORY-441: inverse of `aida archive`. trace:STORY-441 | ai:claude
            archive_cmd::handle_unarchive_command(id, &backend, store_path)?;
        }
        Command::Defer { id, until } => {
            // STORY-584: park a spec on the primed/conditional shelf, hidden
            // from the default open-work view. trace:STORY-584 | ai:claude
            defer_cmd::defer_single(id, until.as_deref(), &backend, store_path)?;
        }
        Command::Undefer { id } => {
            // STORY-584: inverse of `aida defer`. trace:STORY-584 | ai:claude
            defer_cmd::handle_undefer_command(id, &backend, store_path)?;
        }
        Command::Assign { id, to } => {
            // STORY-639: set the durable assignee + route into the target
            // user's work queue. trace:STORY-639 | ai:claude
            assign_cmd::handle_assign_command(id, to, &backend, store_path)?;
        }
        Command::Unassign { id, from_queue } => {
            // STORY-639: clear the assignee (and optionally dequeue).
            // trace:STORY-639 | ai:claude
            assign_cmd::handle_unassign_command(id, *from_queue, &backend, store_path)?;
        }
        Command::Done { spec } => {
            // TASK-728: the newcomer's "I finished it" verb. trace:TASK-727
            handle_done_command(spec, &backend, store_path)?;
        }
        Command::Search {
            query,
            status,
            limit,
            sync,
            all,
            archived,
            deferred,
            include_meta,
            short,
            json,
            fields,
            ..
        } => {
            // STORY-764: the global `--format json` pin reaches search the same
            // way its own `--json` does. trace:STORY-764 | ai:claude
            let effective_json = *json || output_format_is_json();
            // STORY-78: opt-in sync-pull before search. trace:STORY-78 | ai:claude
            if *sync {
                maybe_sync_pull(store_path)?;
            }
            // STORY-441: same three-way archive axis as `aida list`.
            // trace:STORY-441 | ai:claude
            let archive = if *all {
                aida_core::ArchiveFilter::Both
            } else if *archived {
                aida_core::ArchiveFilter::ArchivedOnly
            } else {
                aida_core::ArchiveFilter::NonArchivedOnly
            };
            // STORY-584: same three-way defer axis as `aida list`. `--archived`
            // keeps the defer axis open so the archive audit is complete.
            // trace:STORY-584 | ai:claude
            let defer = if *all || *archived {
                aida_core::DeferFilter::Both
            } else if *deferred {
                aida_core::DeferFilter::DeferredOnly
            } else {
                aida_core::DeferFilter::NonDeferredOnly
            };
            // Cache-backed FTS5 search (EPIC-1-001 Phase 2). Replaces a
            // full-store load + in-memory substring scan.
            // trace:EPIC-1-001 | ai:claude
            let mut results = backend.search(query, *limit, archive, defer)?;
            if let Some(s) = status {
                let needle = s.clone();
                results.retain(|r| r.status.eq_ignore_ascii_case(&needle));
            }
            // BUG-488: hide the seeded META AI-prompt specs by default, same as
            // `aida list` (BUG-27) — a newcomer's search shouldn't surface
            // internal prompt machinery like "Generate Children". Opt in with
            // --include-meta. trace:BUG-488 | ai:claude
            if !*include_meta {
                results.retain(|r| !r.req_type.eq_ignore_ascii_case("meta"));
            }

            // BUG-531: mirror `aida list`'s output-mode flags onto search.
            // `--short`/`-q`/`--ids-only`/`--quiet` emits one bare canonical
            // spec ID per line — no header, no count footer, no color — so the
            // output is directly pipeable into `$(...)` / xargs. Runs AFTER the
            // shared filter + meta passes (same row set the human table shows)
            // and returns early, before the table or JSON rendering. Mutually
            // exclusive with --json (enforced by clap). Empty result = no
            // output (and a zero exit), matching `aida list --short`.
            // trace:BUG-531 | ai:claude
            if *short {
                for req in &results {
                    let id = req.agreed_id.as_deref().or(req.spec_id.as_deref());
                    if let Some(id) = id {
                        println!("{id}");
                    }
                }
                return Ok(());
            }

            // BUG-531: `--json` emits the row set as a JSON array without the
            // human chrome (header / count footer / colour), mirroring
            // `aida list --json`. trace:BUG-531 | ai:claude
            if effective_json {
                #[derive(serde::Serialize)]
                struct SearchJsonRow<'a> {
                    spec_id: &'a str,
                    title: &'a str,
                    req_type: &'a str,
                    status: &'a str,
                    tags: &'a [String],
                }
                let out: Vec<SearchJsonRow> = results
                    .iter()
                    .map(|req| SearchJsonRow {
                        spec_id: req
                            .agreed_id
                            .as_deref()
                            .or(req.spec_id.as_deref())
                            .unwrap_or(""),
                        title: req.title.as_str(),
                        req_type: req.req_type.as_str(),
                        status: req.status.as_str(),
                        tags: &req.tags,
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
                return Ok(());
            }

            // STORY-734: `--fields <csv>` selects + orders the columns of the
            // search table too, mirroring `aida list --fields`. Search rows carry
            // no work-routing axis, so the queued/in_flight/blocked fields render
            // false here. In AGENT mode the row set is emitted as the lean TOON
            // `specs[N]{...}` projection (the ~2x token win the agent surface
            // exists for) exactly like `aida list --fields`, not the verbose
            // human box-table; `render_search_fields` branches on the same
            // `agent_output_mode()` the list handler uses so the two surfaces
            // stay identical. trace:STORY-734 trace:BUG-668 | ai:claude
            if let Some(csv) = fields.as_deref() {
                let selected = toon_list_fields(Some(csv))?;
                if results.is_empty() {
                    println!("No results for: {}", query);
                } else {
                    println!(
                        "{}",
                        render_search_fields(&results, &selected, agent_output_mode())
                    );
                }
                return Ok(());
            }

            // BUG-672: the DEFAULT search path (no `--fields`) was a hardcoded
            // human box-table even in agent mode — the most-called discovery
            // verb leaked human chrome (`─` rule, padded columns, count footer)
            // to agents instead of the lean TOON projection STORY-734 gave the
            // `--fields` path. Route the default through the SAME minimal
            // `specs[N]{id,title,status,type}` schema `aida list` emits (reuse
            // `toon_list_fields(None)` + `render_search_fields`, do NOT reinvent),
            // so the no-`--fields` case finally reaches STORY-734 parity. The
            // human TTY path keeps the box-table unchanged. trace:BUG-672
            if agent_output_mode() {
                let selected = toon_list_fields(None)?;
                println!("{}", render_search_fields(&results, &selected, true));
                // BUG-672 (Finding #4): trailing drill-in next-step block —
                // `aida show <id>` is the move an agent reaching for a search
                // result is missing. Placeholder id (multi-row surface), same
                // rule `list` follows. trace:BUG-672
                let next = crate::help_next::search_next(!results.is_empty());
                if let Some(block) = crate::help_next::render(&next) {
                    println!("{block}");
                }
                return Ok(());
            }

            if results.is_empty() {
                println!("No results for: {}", query);
            } else {
                println!("{:<14} {:<12} {:<10} Title", "ID", "Type", "Status");
                println!("{}", "─".repeat(65));
                for req in &results {
                    let display_id = req
                        .agreed_id
                        .as_deref()
                        .or(req.spec_id.as_deref())
                        .unwrap_or("?");
                    // BUG-781: same type-aware label the list table uses, so a
                    // ratified ADR reads `Accepted` here too. trace:BUG-781
                    println!(
                        "{:<14} {:<12} {:<10} {}",
                        display_id,
                        req.req_type,
                        status_display::display_status_for_type(&req.req_type, &req.status),
                        req.title,
                    );
                }
                println!("\n{} results", results.len());
            }
            // BUG-672 (Finding #4): the HUMAN search surface gets the same
            // drill-in nudge via the `Next:` block (the idiom `aida show` uses),
            // so the operator sees the next move too. trace:BUG-672
            let next = crate::help_next::search_next(!results.is_empty());
            if let Some(block) = crate::help_next::render_human(&next) {
                println!("{block}");
            }
        }
        Command::Comment(CommentCommand::Add {
            id: req_id,
            content,
            content_positional,
            body_file,
            stdin,
            author,
            relayed_from,
            ..
        }) => {
            // BUG-68: lookup first, record activity after the body
            // sanity check below also passes. trace:BUG-68 | ai:claude
            // BUG-1535: refuse an ambiguous id. trace:BUG-1535 | ai:claude
            let mut req = backend
                .get_requirement_unambiguous(req_id)?
                .ok_or_else(|| not_found::requirement_not_found(req_id, Some(store_path)))?;

            // `aida comment add <REQ> "text"` — the text comes through as
            // `content_positional`. Earlier the git-backend dispatch only
            // looked at `--content`, so positional invocations silently
            // wrote empty comments. trace:BUG-28 | ai:claude
            let body = resolve_comment_body(
                content.clone().or_else(|| content_positional.clone()),
                body_file.as_deref(),
                *stdin,
            )?;
            if body.trim().is_empty() {
                anyhow::bail!(
                    "comment body required: pass it positionally `aida comment add {} \"...\"` \
                     or via `--content`",
                    req_id
                );
            }

            let now = chrono::Utc::now();
            let comment_author = author.clone().unwrap_or_else(get_default_author);
            // STORY-644: capture @mentions before `body` is moved into the
            // comment, so we can notify each mentioned user after the write
            // lands. trace:STORY-644 | ai:claude
            let mentions = extract_mentions(&body);
            let mention_body = body.clone();
            let comment = aida_core::Comment {
                id: Uuid::now_v7(),
                content: body,
                author: comment_author.clone(),
                created_at: now,
                modified_at: now,
                parent_id: None,
                replies: Vec::new(),
                reactions: Vec::new(),
                // trace:TASK-330 | ai:claude — stamp the producing session
                session_id: resolve_current_session_id(),
                relayed_from: None,
            }
            // trace:BUG-1534 | ai:claude — record whose claim this relays.
            .with_relayed_from(relayed_from.as_deref());

            req.comments.push(comment);
            req.modified_at = now;
            backend.update_requirement(&req)?;
            // BUG-68: record after successful write, with the canonical
            // spec_id from the resolved req. trace:BUG-68 | ai:claude
            record_role_activity(req.spec_id.as_deref().unwrap_or(req_id), "comment");
            println!("Comment added to {}", req_id);

            // STORY-644: notify each @mentioned user via the mailbox
            // (best-effort; STORY-643 auto-sync delivers on their next pull).
            // The named identity is routed to directly — matching how the
            // mailbox addresses any agent/user id — and a self-mention is
            // skipped. trace:STORY-644 | ai:claude
            if !mentions.is_empty() {
                let display = req.spec_id.as_deref().unwrap_or(req_id);
                let snippet = mention_snippet(&mention_body, 80);
                for handle in &mentions {
                    if handle == &comment_author {
                        continue;
                    }
                    send_notification(
                        store_path,
                        &comment_author,
                        handle,
                        format!("You were mentioned on {display} by {comment_author}: {snippet}"),
                    );
                }
            }
        }
        Command::Comment(CommentCommand::List { id }) => {
            // trace:TASK-1-020 | ai:claude
            // BUG-68: record after successful lookup. trace:BUG-68 | ai:claude
            let req = backend
                .get_requirement_unambiguous(id)?
                .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
            record_role_activity(req.spec_id.as_deref().unwrap_or(id), "show");
            println!("{}: {}", "Requirement".cyan(), req.title);
            println!();
            if req.comments.is_empty() {
                println!("{}", "No comments yet".dimmed());
            } else {
                println!("{}:", "Comments".green().bold());
                for c in &req.comments {
                    print_comment(c, 0);
                }
            }
        }
        // SPIKE-2: edit/delete an existing comment on a git-backed store.
        // Uses prefix-friendly resolution so users can pass the leading 8
        // chars of the UUID instead of the full 36-char form.
        // trace:SPIKE-2 | ai:claude
        Command::Comment(CommentCommand::Edit {
            req_id,
            comment_id,
            content,
            body_file,
            stdin,
            interactive,
        }) => {
            // BUG-68: record after successful lookup. trace:BUG-68 | ai:claude
            let mut req = backend
                .get_requirement_unambiguous(req_id)?
                .ok_or_else(|| not_found::requirement_not_found(req_id, Some(store_path)))?;
            record_role_activity(req.spec_id.as_deref().unwrap_or(req_id), "comment");
            let comment_uuid = resolve_comment_uuid(&req, comment_id)?;

            let new_content =
                if *interactive || (content.is_none() && body_file.is_none() && !stdin) {
                    let existing = req
                        .find_comment_mut(&comment_uuid)
                        .map(|c| c.content.clone())
                        .unwrap_or_default();
                    inquire::Editor::new("Edit comment")
                        .with_predefined_text(&existing)
                        .prompt()
                        .context("Editor cancelled")?
                } else {
                    resolve_comment_body(content.clone(), body_file.as_deref(), *stdin)?
                };

            let comment = req
                .find_comment_mut(&comment_uuid)
                .context("Comment not found (vanished between resolve and edit?)")?;
            comment.content = new_content;
            comment.touch();
            req.modified_at = chrono::Utc::now();
            backend.update_requirement(&req)?;
            println!(
                "{} comment {} on {}",
                "Updated".green(),
                comment_uuid,
                req_id
            );
        }
        Command::Comment(CommentCommand::Delete { req_id, comment_id }) => {
            // BUG-68: record after successful lookup. trace:BUG-68 | ai:claude
            let mut req = backend
                .get_requirement_unambiguous(req_id)?
                .ok_or_else(|| not_found::requirement_not_found(req_id, Some(store_path)))?;
            record_role_activity(req.spec_id.as_deref().unwrap_or(req_id), "comment");
            let comment_uuid = resolve_comment_uuid(&req, comment_id)?;
            req.delete_comment(&comment_uuid)?;
            backend.update_requirement(&req)?;
            println!(
                "{} comment {} from {}",
                "Deleted".green(),
                comment_uuid,
                req_id
            );
        }
        Command::Db(DbCommand::Path) => {
            // trace:FR-1-076 | ai:claude
            println!("{}", store_path.display());
        }
        Command::Db(DbCommand::Info) => {
            let store = backend.load()?;
            println!("{}: Git (sharded YAML)", "Backend".bold());
            println!("{}: {}", "Path".bold(), store_path.display());
            println!("{}: {}", "Requirements".bold(), store.requirements.len());
            println!("{}: {}", "Users".bold(), store.users.len());

            let file_count = aida_core::object_store::list_objects(&store_path.join("objects"))
                .map(|l| l.len())
                .unwrap_or(0);
            println!("{}: {}", "Object files".bold(), file_count);

            // Show git status
            if aida_core::git_ops::is_git_repo(store_path) {
                let has_changes = aida_core::git_ops::has_changes(store_path).unwrap_or(false);
                let status_str = if has_changes {
                    "uncommitted changes"
                } else {
                    "clean"
                };
                println!("{}: {}", "Git".bold(), status_str);
                if let Ok(sha) = aida_core::git_ops::head_sha(store_path) {
                    println!("{}: {}", "HEAD".bold(), sha);
                }
            }

            // TASK-36: surface agreed-id block utilization so users spot
            // imminent exhaustion before it bites at `aida add` time.
            // Aggregates across nodes (sum within each type prefix).
            //
            // BUG-115: remaining sums across every non-exhausted block
            // of the type. The pre-fix code reported only the highest-
            // numbered block's `remaining`, so a near-empty lower block
            // looked invisible and a near-empty higher block masked
            // healthy lower ones.
            // trace:TASK-36 | trace:BUG-115 | ai:claude
            let blocks_path = store_path.join("registry").join("blocks.yaml");
            if let Ok(registry) = aida_core::BlockRegistry::load(&blocks_path) {
                if !registry.blocks.is_empty() {
                    println!();
                    println!("{}", "Agreed-id blocks".bold());
                    use std::collections::BTreeMap;
                    let mut by_prefix: BTreeMap<String, Vec<&aida_core::AgreedIdBlock>> =
                        BTreeMap::new();
                    for b in &registry.blocks {
                        by_prefix
                            .entry(b.type_prefix.to_uppercase())
                            .or_default()
                            .push(b);
                    }
                    for (prefix, blocks) in &by_prefix {
                        let pad = format!("{:<8}", format!("{}:", prefix));
                        let total: u32 =
                            blocks.iter().map(|b| b.range_end - b.range_start + 1).sum();
                        let remaining: u32 = blocks
                            .iter()
                            .filter(|b| !b.is_exhausted())
                            .map(|b| b.remaining())
                            .sum();
                        let active_count = blocks.iter().filter(|b| !b.is_exhausted()).count();
                        if active_count == 0 {
                            let last_end = blocks.iter().map(|b| b.range_end).max().unwrap_or(0);
                            println!(
                                "  {} {}  (last: {}-{}; falling back to node-aware)",
                                pad,
                                "EXHAUSTED".red().bold(),
                                prefix,
                                last_end
                            );
                        } else {
                            let issued = total - remaining;
                            println!(
                                "  {} {}/{}  ({} remaining, {} block{})",
                                pad,
                                issued,
                                total,
                                remaining,
                                blocks.len(),
                                if blocks.len() == 1 { "" } else { "s" }
                            );
                        }
                    }
                }
            }
        }
        // Phase 2: Relationship commands
        Command::Rel(RelationshipCommand::Add {
            from_pos,
            to_pos,
            from_flag,
            to_flag,
            r#type,
            bidirectional,
            force_parent,
        }) => {
            let from = from_pos
                .as_deref()
                .or(from_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing FROM (positional or --from)"))?;
            let to = to_pos
                .as_deref()
                .or(to_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing TO (positional or --to)"))?;
            // trace:BUG-790 | ai:codex
            reject_cross_store_spec_ref(from)?;
            reject_cross_store_spec_ref(to)?;
            let mut from_req = backend
                .get_requirement_unambiguous(from)?
                .ok_or_else(|| not_found::requirement_not_found(from, Some(store_path)))?;

            let to_req = backend
                .get_requirement_unambiguous(to)?
                .ok_or_else(|| not_found::requirement_not_found(to, Some(store_path)))?;

            // BUG-1471: normalize the natural `related` spelling to the
            // traversable standard References edge before anything is written.
            // Other aliases and custom names retain the core parser's behavior.
            // trace:BUG-1471 | ai:codex
            let rel_type = cli_relationship_type(r#type);

            // TASK-887: a `--type` that isn't a standard relationship type lands
            // as a `Custom` edge, which the graph traversals (blocked-by,
            // blocks, tree, impact) silently won't follow. Don't hard-
            // reject (custom types can be intentional), but surface a note —
            // with a did-you-mean when it looks like a typo of a standard type —
            // so a fat-fingered `blockedby`/`depends` doesn't create an
            // invisible edge. trace:TASK-887 | ai:claude
            if let RelationshipType::Custom(ref custom) = rel_type {
                match nearest_standard_rel_type(custom) {
                    Some(near) => eprintln!(
                        "{} '{}' isn't a standard relationship type (did you mean '{}'?); \
                         created as a custom edge — graph traversals won't follow it.",
                        "note:".yellow().bold(),
                        custom,
                        near,
                    ),
                    None => eprintln!(
                        "{} '{}' isn't a standard relationship type ({}); \
                         created as a custom edge — graph traversals won't follow it.",
                        "note:".yellow().bold(),
                        custom,
                        STANDARD_REL_TYPES.join(", "),
                    ),
                }
            }

            // BUG-64: same terminal-status guard as `aida add --parent`,
            // applied here when the user is hand-rolling a parent edge
            // via `aida rel add X Y --type child` (X is child of Y, so Y
            // is the parent being checked) or its inverse `--type parent`
            // (X is parent of Y). `--force-parent` bypasses for backfill
            // cases. trace:BUG-64 | ai:claude
            if !*force_parent {
                let parent_for_guard = match &rel_type {
                    RelationshipType::Child => Some(&to_req),
                    RelationshipType::Parent => Some(&from_req),
                    _ => None,
                };
                if let Some(p) = parent_for_guard {
                    if is_terminal_status(&p.status) {
                        anyhow::bail!(
                            "parent {} is {} — adding new children to a closed parent is usually a mistake. \
                             Pass `--force-parent` to override.",
                            p.spec_id.as_deref().unwrap_or("?"),
                            p.status,
                        );
                    }
                }
            }
            if let Some(warning) =
                relationship_terminal_ambiguity_warning(&rel_type, &from_req, &to_req)
            {
                // trace:BUG-790 | ai:codex
                eprintln!("{} {}", "warning:".yellow().bold(), warning);
            }

            // TASK-679: a parent/child edge is canonically BIDIRECTIONAL so it
            // matches `aida add --parent` (which writes the parent's `Parent`
            // edge AND the child's reciprocal `Child` edge). The old `rel add`
            // wrote only the source-side edge, leaving the other spec's `show`
            // blind to the link (spec point #5). Force the reciprocal for
            // parent/child even without an explicit `--bidirectional`; other
            // edge types keep the opt-in flag. trace:TASK-679 | ai:claude
            let write_inverse = rel_should_write_inverse(&rel_type, *bidirectional);

            // TASK-679: dedup. The old git-canonical path pushed blindly, so a
            // repeated `rel add` (EPIC-37 had the same target twice) accumulated
            // duplicate edges. Skip the push when an identical (rel_type,
            // target) edge already exists on the source. trace:TASK-679 | ai:claude
            let edge_exists = from_req
                .relationships
                .iter()
                .any(|r| r.rel_type == rel_type && r.target_id == to_req.id);
            if edge_exists {
                println!(
                    "Relationship already exists: {} --[{:?}]--> {} (no change)",
                    from, rel_type, to
                );
            } else {
                let rel = aida_core::models::Relationship {
                    rel_type: rel_type.clone(),
                    target_id: to_req.id,
                    created_at: Some(chrono::Utc::now()),
                    created_by: Some(get_default_author()),
                };

                from_req.relationships.push(rel);
                from_req.modified_at = chrono::Utc::now();
                backend.update_requirement(&from_req)?;
                println!("Added relationship: {} --[{:?}]--> {}", from, rel_type, to);
            }

            if write_inverse {
                let mut to_req = backend.get_requirement_unambiguous(to)?.unwrap();
                // STORY-333: use the canonical inverse helper so new typed
                // variants (BlockedBy ↔ Blocks) compose automatically.
                // trace:STORY-333 | ai:claude
                let inverse_type = rel_type.inverse().unwrap_or_else(|| rel_type.clone());
                // TASK-679: dedup the inverse end too. trace:TASK-679 | ai:claude
                let inverse_exists = to_req
                    .relationships
                    .iter()
                    .any(|r| r.rel_type == inverse_type && r.target_id == from_req.id);
                if inverse_exists {
                    println!(
                        "Inverse already exists: {} --[{:?}]--> {} (no change)",
                        to, inverse_type, from
                    );
                } else {
                    let inv_rel = aida_core::models::Relationship {
                        rel_type: inverse_type.clone(),
                        target_id: from_req.id,
                        created_at: Some(chrono::Utc::now()),
                        created_by: Some(get_default_author()),
                    };
                    to_req.relationships.push(inv_rel);
                    to_req.modified_at = chrono::Utc::now();
                    backend.update_requirement(&to_req)?;
                    println!("Added inverse: {} --[{:?}]--> {}", to, inverse_type, from);
                }
            }
        }
        Command::Rel(RelationshipCommand::Remove {
            from_pos,
            to_pos,
            from_flag,
            to_flag,
            ..
        }) => {
            let from = from_pos
                .as_deref()
                .or(from_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing FROM (positional or --from)"))?;
            let to = to_pos
                .as_deref()
                .or(to_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing TO (positional or --to)"))?;
            let mut from_req = backend
                .get_requirement_unambiguous(from)?
                .ok_or_else(|| not_found::requirement_not_found(from, Some(store_path)))?;

            // Look up target UUID
            let to_req = backend
                .get_requirement_unambiguous(to)?
                .ok_or_else(|| not_found::requirement_not_found(to, Some(store_path)))?;

            let before = from_req.relationships.len();
            from_req.relationships.retain(|r| r.target_id != to_req.id);
            let removed = before - from_req.relationships.len();

            if removed > 0 {
                from_req.modified_at = chrono::Utc::now();
                backend.update_requirement(&from_req)?;
                println!(
                    "Removed {} relationship(s) from {} to {}",
                    removed, from, to
                );
            } else {
                println!("No relationship found from {} to {}", from, to);
            }
        }
        Command::Rel(RelationshipCommand::List {
            id,
            source,
            target,
            r#type,
            dangling,
            all,
            limit,
        }) => {
            // trace:TASK-65 | ai:claude
            // Three modes:
            //   - target=Some                → incoming edges
            //   - id|source=Some             → outgoing edges (legacy point query)
            //   - none of the above          → global listing
            // --type / --dangling / --all compose across all three modes.
            let source_ref = source.as_deref().or(id.as_deref());
            handle_rel_list_modern(
                &backend,
                store_path,
                source_ref,
                target.as_deref(),
                r#type.as_deref(),
                *dangling,
                *all,
                *limit,
            )?;
        }

        // Phase 1: Sync command
        Command::Db(DbCommand::Sync {
            pull,
            push,
            message,
        }) => {
            if !aida_core::git_ops::is_git_repo(store_path) {
                anyhow::bail!("Not a git repository: {}", store_path.display());
            }

            let branch = aida_core::git_ops::current_branch(store_path)
                .unwrap_or_else(|_| "main".to_string());

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
            let has_changes = aida_core::git_ops::has_changes(store_path)?;
            if has_changes {
                let msg = message.as_deref().unwrap_or("chore: sync pending changes");
                aida_core::git_ops::add_all(store_path, ".")?;
                aida_core::git_ops::commit(store_path, msg)?;
                println!("Committed: {}", msg);
            } else if !*pull && !*push {
                println!("Nothing to commit.");
            }

            // Step 2: pull --rebase. Bare `git pull` fails on divergent
            // branches when the user has no `pull.rebase` / `pull.ff`
            // config — the orphan-store model wants linear history
            // anyway (replay local commits on top of remote), so
            // rebase is the right default.
            if *pull && !aida_core::git_ops::has_remote(store_path, "origin") {
                // BUG-432: local-only / fresh-init projects have no `origin`
                // yet (the aida-demo + pre-remote flow). The pull is a graceful
                // skip, not a fatal — otherwise `aida queue work`'s startup sync
                // (and a standalone `aida db sync --pull`) die before a remote
                // is ever added. Mirrors `aida fetch --code-only`'s tolerance.
                // trace:BUG-432 | ai:claude
                println!("  No `origin` remote — skipping pull (local-only project).");
            } else if *pull {
                // Snapshot local state before pull for conflict detection
                let local_reqs = backend.load().map(|s| s.requirements).unwrap_or_default();

                println!("Pulling from origin/{}...", branch);
                match aida_core::git_ops::pull_rebase(store_path, "origin", &branch) {
                    Ok(()) => {
                        println!("  Pull complete.");
                        ensure_no_spec_id_collisions(store_path)?;

                        // Detect conflicts with remote changes
                        let remote_reqs =
                            backend.load().map(|s| s.requirements).unwrap_or_default();

                        let conflicts =
                            aida_core::conflict::detect_store_conflicts(&local_reqs, &remote_reqs);

                        if !conflicts.is_empty() {
                            println!();
                            println!("{}", "Conflicts detected:".red().bold());
                            for conflict in &conflicts {
                                println!();
                                print!("{}", conflict);
                            }
                            println!();
                            println!(
                                "Resolve with: aida edit <ID> --title/--status/... to pick the version you want."
                            );
                        }

                        // STORY-86: also scan the code repo's default
                        // branch for any spec-referencing commits that
                        // arrived (possibly via a separate `git pull`)
                        // and bump matching Done specs. `db sync --pull`
                        // is decoupled from `git pull`, so we don't know
                        // a pre_sha here — fall back to HEAD~50. The
                        // `status == Done` guard inside the helper keeps
                        // this idempotent. trace:STORY-86 | ai:claude
                        if auto_bump_enabled() {
                            if let Some(project_root) = store_path.parent() {
                                let storage = Storage::new(store_path);
                                match auto_bump_done_to_completed(
                                    project_root,
                                    store_path,
                                    None,
                                    &storage,
                                ) {
                                    Ok(flips) => print_auto_bump_summary(&flips),
                                    Err(e) => {
                                        eprintln!(
                                            "  {} auto-bump failed: {}",
                                            "Warning:".yellow().bold(),
                                            e
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
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
                }
            }

            if *push {
                println!("Pushing to origin/{}...", branch);
                let origin_ok = match aida_core::git_ops::push(store_path, "origin", &branch) {
                    Ok(true) => {
                        println!("  Push complete.");
                        true
                    }
                    Ok(false) => {
                        println!("  Push rejected. Pulling and retrying...");
                        aida_core::git_ops::pull_rebase(store_path, "origin", &branch)?;
                        aida_core::git_ops::push(store_path, "origin", &branch)?;
                        println!("  Push complete after rebase.");
                        true
                    }
                    Err(e) => {
                        eprintln!("  Push failed: {}", e);
                        false
                    }
                };

                // TASK-1096: fan out the store push to every configured mirror
                // remote so a clone can't silently leave one hub behind — the
                // drift-prevention leg. trace:TASK-1096 | ai:claude
                if origin_ok {
                    if let Some(project_root) = store_path.parent() {
                        fan_out_mirror_push(store_path, &branch, project_root);
                    }
                }
            }

            if !pull && !push {
                println!("Use --pull and/or --push to sync with remote.");
                println!("  aida db sync --pull --push");
            }

            // TASK-1033: opportunistic store maintenance after a sync that
            // touched the store — ensure the lowered gc.auto is set, then
            // `git gc --auto` (a cheap no-op unless the threshold is exceeded).
            // Best-effort; never fails the sync.
            if *pull || *push {
                aida_core::git_ops::opportunistic_store_gc(store_path);
            }
        }

        // Status
        Command::Db(DbCommand::Status) => {
            let store = backend.load()?;

            let total = store.requirements.len();
            let with_agreed = store
                .requirements
                .iter()
                .filter(|r| r.agreed_id.is_some())
                .count();
            let without_agreed = total - with_agreed;

            println!("{}", "Distributed Store Status".bold());
            println!("{}", "─".repeat(45));
            println!("{:<20} {}", "Path:", store_path.display());
            println!("{:<20} {}", "Requirements:", total);
            println!("{:<20} {}", "With agreed ID:", with_agreed);
            println!("{:<20} {}", "Pending merge-gate:", without_agreed);

            if aida_core::git_ops::is_git_repo(store_path) {
                let has_changes = aida_core::git_ops::has_changes(store_path).unwrap_or(false);
                let head = aida_core::git_ops::head_sha(store_path).unwrap_or_else(|_| "?".into());
                let branch =
                    aida_core::git_ops::current_branch(store_path).unwrap_or_else(|_| "?".into());

                println!("{:<20} {}", "Branch:", branch);
                println!("{:<20} {}", "HEAD:", head);
                println!(
                    "{:<20} {}",
                    "Working tree:",
                    if has_changes {
                        "uncommitted changes"
                    } else {
                        "clean"
                    }
                );

                let remote_ok = aida_core::git_ops::is_remote_reachable(store_path, "origin");
                println!(
                    "{:<20} {}",
                    "Remote:",
                    if remote_ok {
                        "reachable"
                    } else {
                        "not configured or unreachable"
                    }
                );
            }

            // Show dispenser state
            if let Ok(disp) = load_dispenser(store_path) {
                if let Ok(state) = disp.state() {
                    let mode_str = match &state.mode {
                        aida_core::IdMode::Centralized => "centralized".to_string(),
                        aida_core::IdMode::Distributed { node_id } => {
                            format!("distributed (node {})", node_id)
                        }
                    };
                    println!("{:<20} {}", "ID mode:", mode_str);
                    let total_dispensed: u32 = state.sequences.values().sum();
                    println!("{:<20} {}", "IDs dispensed:", total_dispensed);
                }
            }
        }

        // Merge gate: assign agreed IDs
        Command::Db(DbCommand::MergeGate) => {
            if !aida_core::git_ops::is_git_repo(store_path) {
                anyhow::bail!("Not a git repository: {}", store_path.display());
            }

            // STORY-647: team RBAC guardrail — the merge gate assigns the agreed
            // short IDs (an advisor act by default, tunable via
            // `[team.permissions] merge_gate`). Advisor authority (TTY / live
            // drain / advisor role) bypasses; `merge-gate` carries no `--force`
            // flag, so a non-advisor seats the role to proceed. trace:STORY-647
            enforce_team_gate(permissions::GatedOp::MergeGate, false)?;

            let assignments = aida_core::git_ops::merge_gate(store_path)?;

            if assignments.is_empty() {
                println!("All objects already have agreed IDs.");
            } else {
                println!("Assigned {} agreed ID(s):", assignments.len());
                for (node_id, agreed_id) in &assignments {
                    println!("  {} → {}", node_id, agreed_id.green().bold());
                }
            }
        }

        // trace:FR-2-005 | ai:claude
        Command::Db(DbCommand::Block { subcommand }) => {
            handle_block_command(subcommand, store_path)?;
        }

        // trace:FR-1-071 | ai:claude
        Command::Db(DbCommand::RetireLegacyIds { dry_run }) => {
            handle_retire_legacy_ids(&backend, store_path, *dry_run)?;
        }

        // trace:TASK-226 | ai:claude
        Command::Db(DbCommand::ReconcileStatus {
            since,
            spec,
            dry_run,
        }) => {
            handle_db_reconcile_status(store_path, since.as_deref(), spec.as_deref(), *dry_run)?;
        }

        // trace:TASK-80 | ai:claude
        Command::Db(DbCommand::Check { collisions, repair }) => {
            if !*collisions {
                anyhow::bail!(
                    "`aida db check` requires a check flag. Try `--collisions` (currently the only supported audit)."
                );
            }
            handle_db_check_collisions(&backend, store_path, *repair)?;
        }

        // Phase 3: Export to git backend
        Command::Db(DbCommand::ExportGit { output }) => {
            let output_path = std::path::PathBuf::from(output);

            // Load from whatever backend we're using (this handler is already git,
            // but the export command is also available from the main CLI path)
            let store = backend.load()?;

            // Create the target git backend
            let target = aida_core::GitBackend::new(&output_path)?;

            // Initialize git repo if not already
            if !aida_core::git_ops::is_git_repo(&output_path) {
                aida_core::git_ops::init(&output_path)?;
                let git_name = aida_core::git_ops::git_config_get("user.name")
                    .unwrap_or_else(|_| "AIDA".to_string());
                let git_email = aida_core::git_ops::git_config_get("user.email")
                    .unwrap_or_else(|_| "aida@localhost".to_string());
                aida_core::git_ops::configure_user(&output_path, &git_name, &git_email)?;
            }

            target.save(&store)?;
            println!(
                "Exported {} requirements to {}",
                store.requirements.len(),
                output_path.display()
            );
        }

        Command::Queue(queue_cmd) => {
            // STORY-78: `aida queue list --sync` pulls before listing.
            // Done here at dispatch (not inside handle_queue_command) so
            // the helper has direct store_path access. Other queue
            // subcommands have no --sync. trace:STORY-78 | ai:claude
            if let QueueCommand::List { sync: true, .. } = queue_cmd {
                maybe_sync_pull(store_path)?;
            }
            // Reuse the legacy Storage handler — it works against any
            // DatabaseBackend via Storage trait shims, and our GitBackend
            // already implements queue_list/add/remove/reorder/clear.
            // Wrap our backend in a Storage façade pointing at the store path.
            let storage = Storage::new(store_path);
            handle_queue_command(queue_cmd, &storage, store_path)?;
        }
        Command::Load(load_cmd) => {
            let storage = Storage::new(store_path);
            load_cmd::handle_load_command(load_cmd, &storage)?;
        }
        // FR-267: trace commands must resolve spec ids against the resolved
        // git store — including a SIBLING store in an `aida init --sibling`
        // workspace. Previously `Command::Trace` fell through to the
        // "not yet supported for git backend" catch-all, so `aida trace
        // scan` / `aida trace list` only worked when the store lived inside
        // the current repo. `store_path` here is whatever
        // `detect_distributed_store` resolved (the sibling `../aida-store`
        // for sibling-mode configs), and `Storage::new(<dir>)` delegates
        // load/save to GitBackend — so the same store-resolution that
        // already backs `aida add` / `aida show` now backs trace-id
        // resolution too, regardless of which code repo's CWD invoked it.
        // (MCP `show_requirement` already routed through the resolved
        // store_path, so it needed no change.)
        // trace:FR-267 | ai:claude
        Command::Trace(trace_cmd) => {
            let storage = Storage::new(store_path);
            crate::trace_cmd::handle_trace_command(trace_cmd, &storage)?;
        }
        // STORY-444 + STORY-451: `aida backlog` owns grooming plus the
        // `load` alias for quantitative effort summaries.
        Command::Backlog(backlog_cmd) => {
            let storage = Storage::new(store_path);
            match backlog_cmd {
                BacklogCommand::Load => {
                    load_cmd::handle_load_command(&LoadCommand::Backlog, &storage)?
                }
                _ => backlog::handle_backlog_command(backlog_cmd, &storage)?,
            }
        }
        // TASK-218: top-level `aida rework SPEC` → forwards to the same
        // handler as `aida queue rework SPEC`. trace:TASK-218 | ai:claude
        Command::Rework {
            id,
            work,
            r#for,
            tail,
            status,
            reason,
            resume,
            force,
            steal,
            permission_mode,
            no_pull,
            user,
        } => {
            let storage = Storage::new(store_path);
            handle_queue_rework(
                &storage,
                id,
                *work,
                r#for.as_deref(),
                *tail,
                status.as_deref(),
                reason.as_deref(),
                *resume,
                *force,
                *steal,
                permission_mode.as_deref(),
                *no_pull,
                user.as_deref(),
            )?;
        }
        Command::Review {
            spec,
            no_agent,
            allow_stale_base,
            cmd,
        } => {
            // trace:STORY-553 | ai:claude — `aida review <SPEC>` drives a
            // human-decision review of a held spec (the dual of `aida queue
            // work`). The `prompt` / `assemble` subcommands stay the
            // review-prompt helpers, read via the same Storage façade.
            match (spec, cmd) {
                (Some(spec_id), _) => {
                    handle_review_spec(
                        &backend,
                        store_path,
                        spec_id,
                        *no_agent,
                        *allow_stale_base,
                    )?;
                }
                (None, Some(review_cmd)) => {
                    let storage = Storage::new(store_path);
                    handle_review_command(review_cmd, &storage)?;
                }
                (None, None) => {
                    anyhow::bail!("pass a spec id (`aida review <SPEC>`) or a subcommand (`prompt` / `assemble`)");
                }
            }
        }
        // STORY-44: `aida config user` is a global op against
        // ~/.aida/preferences.toml — no store needed. Route it through the
        // git-backend path so it works in modern projects too.
        Command::Config(ConfigCommand::User {
            node_id,
            email,
            toml: emit_toml,
        }) => {
            config_cmd::handle_config_user(node_id.as_deref(), email.as_deref(), *emit_toml)?;
        }
        // STORY-106: `aida config hints` reads/writes `.aida/config.toml`
        // — project-level, no store mutation needed. Route through the
        // git-backend path so it works in modern projects.
        // trace:STORY-106 | ai:claude
        Command::Config(ConfigCommand::Hints { enabled }) => {
            let storage = Storage::new(store_path);
            config_cmd::handle_config_hints(enabled.as_deref(), &storage)?;
        }
        // STORY-633: `aida config glyph ...` — CLI surface over the glyph
        // registry, themes, and per-symbol override table. Writes to
        // .aida/config.toml (or ~/.aida/config.toml with --user), preserving
        // the rest of the file. trace:STORY-633 | ai:claude
        Command::Config(ConfigCommand::Glyph(glyph_cmd)) => {
            config_cmd::handle_config_glyph(glyph_cmd)?;
        }
        Command::Config(config_cmd) => {
            let storage = Storage::new(store_path);
            config_cmd::handle_config_command(config_cmd, &storage)?;
        }
        Command::Scaffold(scaffold_cmd) => {
            // Scaffold apply / status / preview / extract — same pattern.
            // Storage façade now handles directory paths via GitBackend.load().
            let storage = Storage::new(store_path);
            scaffold_cmd::handle_scaffold_command(scaffold_cmd, &storage, store_path)?;
        }
        Command::Doc(doc_cmd) => {
            // trace:STORY-104 | ai:claude
            doc_cmd::handle_doc_command(doc_cmd, store_path, &backend)?;
        }
        Command::History {
            limit,
            max_commits,
            events,
            id,
            r#type,
            author,
            since,
            until,
            status_changes,
            kind,
            shipped,
            comments,
            oneline,
            all,
            archived,
            deferred,
            include_meta,
            cmd,
        } => {
            // STORY-1436: `--kind` reads the local event feed (non-actions
            // included) rather than the spec git log. trace:STORY-1436 | ai:claude
            if let Some(kind) = kind {
                return crate::history_kind_report(
                    kind,
                    since.as_deref(),
                    until.as_deref(),
                    author.as_deref(),
                    *limit,
                );
            }
            let events = match cmd {
                Some(HistoryCommand::Events) => true,
                None => {
                    if *events {
                        note_hidden_alias(
                            concat!("aida history ", "--events"),
                            "aida history events",
                        );
                    }
                    *events
                }
            };
            // trace:FR-1-037 | ai:claude
            // Default max_commits scales differently per mode: digest only
            // touches each commit once (cheap, scan deeper), events shells
            // to git per file per commit (expensive, scan shallow).
            let default_max = if events { (*limit * 5).max(50) } else { 250 };
            let max = max_commits.unwrap_or(default_max);
            // STORY-441: archive axis replaces TASK-64's terminal-status
            // hide. Default surfaces non-archived rows; `--all` widens to
            // the union; `--archived` narrows to archived-only. When
            // `--id <ID>` was passed, the user named a single spec so we
            // bypass archive filtering for that spec's timeline.
            // trace:STORY-441 | ai:claude
            let archive = if id.is_some() || *all {
                aida_core::ArchiveFilter::Both
            } else if *archived {
                aida_core::ArchiveFilter::ArchivedOnly
            } else {
                aida_core::ArchiveFilter::NonArchivedOnly
            };
            // STORY-584: parallel defer axis. `--id` bypasses (single-spec
            // timeline); `--all` and `--archived` keep it open so those audits
            // are complete; `--deferred` narrows to the shelf; default hides it.
            // trace:STORY-584 | ai:claude
            let defer = if id.is_some() || *all || *archived {
                aida_core::DeferFilter::Both
            } else if *deferred {
                aida_core::DeferFilter::DeferredOnly
            } else {
                aida_core::DeferFilter::NonDeferredOnly
            };
            // Build the set of archived spec_ids the caller's archive filter
            // should hide. Cheap: one indexed SELECT. Empty when archive is
            // Both (nothing to hide) or ArchivedOnly (handled by the
            // include-set below). trace:STORY-441 | ai:claude
            let archived_specs: std::collections::HashSet<String> = match archive {
                aida_core::ArchiveFilter::NonArchivedOnly => backend
                    .list_summaries(&aida_core::ListFilter {
                        archive: aida_core::ArchiveFilter::ArchivedOnly,
                        ..Default::default()
                    })?
                    .into_iter()
                    .filter_map(|s| s.spec_id)
                    .collect(),
                _ => std::collections::HashSet::new(),
            };
            let archived_only_specs: Option<std::collections::HashSet<String>> = match archive {
                aida_core::ArchiveFilter::ArchivedOnly => Some(
                    backend
                        .list_summaries(&aida_core::ListFilter {
                            archive: aida_core::ArchiveFilter::ArchivedOnly,
                            ..Default::default()
                        })?
                        .into_iter()
                        .filter_map(|s| s.spec_id)
                        .collect(),
                ),
                _ => None,
            };
            // STORY-584: the same two sets on the defer axis. The DeferredOnly
            // ListFilter honors both the flag and legacy `deferred:*` tags.
            // trace:STORY-584 | ai:claude
            let deferred_specs: std::collections::HashSet<String> = match defer {
                aida_core::DeferFilter::NonDeferredOnly => backend
                    .list_summaries(&aida_core::ListFilter {
                        defer: aida_core::DeferFilter::DeferredOnly,
                        archive: aida_core::ArchiveFilter::Both,
                        ..Default::default()
                    })?
                    .into_iter()
                    .filter_map(|s| s.spec_id)
                    .collect(),
                _ => std::collections::HashSet::new(),
            };
            let deferred_only_specs: Option<std::collections::HashSet<String>> = match defer {
                aida_core::DeferFilter::DeferredOnly => Some(
                    backend
                        .list_summaries(&aida_core::ListFilter {
                            defer: aida_core::DeferFilter::DeferredOnly,
                            archive: aida_core::ArchiveFilter::Both,
                            ..Default::default()
                        })?
                        .into_iter()
                        .filter_map(|s| s.spec_id)
                        .collect(),
                ),
                _ => None,
            };
            // BUG-588: `aida history` filters by spec_id (the orphan-branch git
            // log is the source-of-truth for spec-state time series, and each
            // event is keyed by the YAML's spec_id). But `aida show` prints the
            // raw UUID, and a user who copies that into `--id <uuid>` got an
            // empty "(no recent activity)" — the filter never matched because it
            // only ever compared against spec_id. Resolve a UUID (or agreed_id)
            // to its canonical spec_id here so the documented invocation works.
            // trace:BUG-588 | ai:claude
            let id_filter = id
                .as_ref()
                .map(|raw| resolve_history_id_filter(&backend, raw));
            let opts = history::HistoryOpts {
                limit: *limit,
                max_commits: max.max(*limit),
                // TASK-507: --shipped is an events-mode filter; imply it.
                events_mode: events || *shipped,
                id_filter,
                type_filter: r#type.clone(),
                author_filter: author.clone(),
                since: since.clone(),
                until: until.clone(),
                status_changes_only: *status_changes,
                shipped_only: *shipped,
                comments_only: *comments,
                oneline: *oneline,
                archived_specs,
                archived_only_specs,
                deferred_specs,
                deferred_only_specs,
                // STORY-737 (delight #4): hide META by default. An explicit
                // `--type meta` (or `--include-meta`) keeps them visible — the
                // type filter is honored above, so we only need to suppress the
                // default-view drowning. trace:STORY-737 | ai:claude
                exclude_meta: history_should_exclude_meta(*include_meta, r#type.as_deref()),
            };
            history::run(store_path, &opts)?;
        }
        Command::StateSnapshot {
            spec,
            tests,
            fmt,
            json,
        } => {
            handle_state_snapshot_command(&backend, store_path, spec, tests, fmt, *json)?;
        }
        _ => {
            eprintln!(
                "Command not yet supported for git backend.\n\
                 Supported: list, add, show, edit, del, search, comment add/list,\n\
                 queue list/add/remove/move/clear,\n\
                 rel add/remove/list, db info/status/sync/merge-gate/export-git/workspace-init"
            );
            std::process::exit(1);
        }
    }

    if command_triggers_per_write_auto_push(command) {
        maybe_auto_push_store(store_path, StoreAutoPushMode::PerWrite, "per-write");
    }

    Ok(())
}
