//! `aida import-plan` command cluster (TASK-114 / TASK-516).
//!
//! `/aida-import-plan <FILE>` lands a saved plan (e.g. from `/ultraplan`'s
//! teleport-back) under `docs/plans/YYYY-MM-DD-<slug>.md`, pins it to its
//! SPEC with a comment, and optionally requests master review. Extracted
//! verbatim from `main.rs` (SPIKE-78); no behavior change. Shared helpers
//! (`find_main_worktree_root`, `get_default_author`, `slugify_str`,
//! `resolve_current_session_id`, the `plan_review_warning` decision + the
//! `PLAN_REVIEW_PENDING_TAG` const it shares with `aida queue work`) stay in
//! `main.rs` and are reached via `crate::`.

use anyhow::Result;
use colored::Colorize;

use aida_core::Comment;
use aida_core::DatabaseBackend;
use uuid::Uuid;

use crate::not_found;
use crate::PLAN_REVIEW_PENDING_TAG;
use crate::{find_main_worktree_root, get_default_author, resolve_current_session_id, slugify_str};

/// TASK-516: detect a `TYPE-N` spec id embedded in a plan filename (e.g.
/// `task-516`, `STORY-86`, `2026-06-06-task-516-foo.md`). Returns the
/// uppercased `TYPE-N` token, or None. Fully isolated for unit testing.
// trace:TASK-516 | ai:claude
fn detect_spec_id_from_filename(filename: &str) -> Option<String> {
    let re = regex::Regex::new(
        r"(?i)\b(functional|non-functional|system|user|bug|epic|story|task|spike|sprint|folder|meta|doc|fr|nfr)-(\d+)\b",
    )
    .ok()?;
    re.captures(filename)
        .map(|c| format!("{}-{}", c[1].to_uppercase(), &c[2]))
}

/// `aida import-plan <file> [--spec <SPEC>] [--request-review]` (TASK-516).
///
/// Archives a saved plan markdown file under
/// `docs/plans/YYYY-MM-DD-<slug>.md` and pins it to its SPEC with a
/// comment. With `--request-review`, lands a minimal master-review
/// handshake: tags the spec `plan-review:pending` and posts a "plan landed
/// for master review" comment so `aida queue work <SPEC>` warns before
/// pickup, keeping an unreviewed plan from being treated as canonical.
///
/// Minimal Phase-1 slice of TASK-516: no dedicated approve/revise/decline
/// verbs and no `aida status` pane — the master clears the tag/edits the
/// comment by hand.
// trace:TASK-516 | ai:claude
pub(crate) fn handle_import_plan_command(
    backend: &aida_core::CachedGitBackend,
    store_path: &std::path::Path,
    file: &str,
    spec: Option<&str>,
    request_review: bool,
) -> Result<()> {
    let src = std::path::Path::new(file);
    if !src.exists() {
        anyhow::bail!("Plan file not found: {file}");
    }

    // Resolve the target spec: explicit --spec wins, else detect from the
    // filename. Never guess silently beyond the filename heuristic.
    let detected = spec
        .map(|s| s.to_string())
        .or_else(|| detect_spec_id_from_filename(file));
    let spec_id = detected.ok_or_else(|| {
        anyhow::anyhow!(
            "Could not determine the target SPEC for this plan. Pass `--spec <SPEC-ID>` \
             (the filename had no recognizable TYPE-N pattern)."
        )
    })?;

    let mut req = backend
        .get_requirement_by_spec_id(&spec_id)?
        .ok_or_else(|| not_found::requirement_not_found(&spec_id, Some(store_path)))?;
    let spec_display = req
        .agreed_id
        .as_deref()
        .or(req.spec_id.as_deref())
        .unwrap_or(&spec_id)
        .to_string();

    // Archive the plan under docs/plans/YYYY-MM-DD-<slug>.md (AIDA
    // convention). Slug derives from the spec title; fall back to the
    // source filename stem. The plan dir lives at the main worktree root.
    let main_root = find_main_worktree_root().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let plans_dir = main_root.join("docs/plans");
    std::fs::create_dir_all(&plans_dir)?;
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let slug_source = if req.title.trim().is_empty() {
        src.file_stem().and_then(|s| s.to_str()).unwrap_or("plan")
    } else {
        req.title.as_str()
    };
    let slug: String = slugify_str(slug_source)
        .split('-')
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() {
        "plan".to_string()
    } else {
        slug
    };
    let dest = plans_dir.join(format!("{date}-{slug}.md"));
    // Don't overwrite an existing archived plan silently — disambiguate.
    let dest = if dest.exists() && dest != src {
        plans_dir.join(format!(
            "{date}-{slug}-{}.md",
            chrono::Local::now().format("%H%M%S")
        ))
    } else {
        dest
    };
    if dest != src {
        std::fs::copy(src, &dest)?;
    }
    let dest_rel = dest
        .strip_prefix(&main_root)
        .unwrap_or(&dest)
        .display()
        .to_string();

    let now = chrono::Utc::now();
    let author = get_default_author();

    let comment_body = if request_review {
        format!(
            "Plan landed for master review: {dest_rel}. Master verdict awaited before \
             implementer pickup (tagged `{PLAN_REVIEW_PENDING_TAG}`)."
        )
    } else {
        format!("Plan imported to {dest_rel} (via aida import-plan).")
    };
    req.comments.push(Comment {
        id: Uuid::now_v7(),
        content: comment_body,
        author,
        created_at: now,
        modified_at: now,
        parent_id: None,
        replies: Vec::new(),
        reactions: Vec::new(),
        session_id: resolve_current_session_id(), // trace:TASK-330
    });

    if request_review {
        req.tags.insert(PLAN_REVIEW_PENDING_TAG.to_string());
    }
    req.modified_at = now;
    backend.update_requirement(&req)?;

    println!(
        "Imported plan to {} (pinned to {}).",
        dest_rel, spec_display
    );
    if request_review {
        println!(
            "  {} {} tagged `{}`. `aida queue work {}` will warn before pickup until the \
             master clears the tag with `aida edit {} --remove-tag {}`.",
            "Master review requested:".cyan().bold(),
            spec_display,
            PLAN_REVIEW_PENDING_TAG,
            spec_display,
            spec_display,
            PLAN_REVIEW_PENDING_TAG,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TASK-516: filename spec-detection picks up `TYPE-N` patterns and
    /// uppercases the type; returns None when no pattern is present.
    // trace:TASK-516 | ai:claude
    #[test]
    fn detect_spec_id_from_filename_finds_type_n() {
        assert_eq!(
            detect_spec_id_from_filename("2026-06-06-task-516-handshake.md"),
            Some("TASK-516".to_string())
        );
        assert_eq!(
            detect_spec_id_from_filename("STORY-86.md"),
            Some("STORY-86".to_string())
        );
        assert_eq!(detect_spec_id_from_filename("plan.md"), None);
        assert_eq!(detect_spec_id_from_filename("no-numbers-here.md"), None);
    }
}
