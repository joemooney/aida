//! `aida import-plan` command cluster (TASK-114 / TASK-516).
//!
//! `/aida-import-plan <FILE>` lands any saved plan under
//! `docs/plans/YYYY-MM-DD-<slug>.md`, pins it to its SPEC with a comment,
//! and optionally requests master review. Extracted
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

use crate::init_cmd::ensure_plan_template_scaffold;
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
        .get_requirement_unambiguous(&spec_id)? // trace:TASK-1468 | ai:claude
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
    ensure_plan_template_scaffold(&plans_dir, false)?;
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
        harness_version: None,
        relayed_from: None,
    });

    if request_review {
        req.tags.insert(PLAN_REVIEW_PENDING_TAG.to_string());
    }
    req.modified_at = now;
    backend.update_requirement(&req)?;

    // STORY-1480: the import IS the planning-phase record — the timeline's
    // planning marker for specs planned outside a drain. Unverified here;
    // `aida plan verify` records the verified companion.
    // trace:STORY-1480 | ai:claude
    crate::events::emit_interactive_lifecycle(
        &main_root,
        &[spec_display.clone()],
        &crate::events::EventKind::PlanRecorded { verified: false },
    );

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

/// Import a saved plan file into AIDA conventions: archive it under
/// `docs/plans/YYYY-MM-DD-<slug>.md` and pin it to its SPEC with a
/// comment. `--request-review` additionally lands a master-review
/// handshake — it tags the spec `plan-review:pending` and posts a
/// "plan landed for master review" comment, so the plan is NOT yet
/// treated as canonical: `aida queue work <SPEC>` warns before
/// picking up a spec whose plan is still awaiting review.
// trace:TASK-516 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "import-plan")]
pub(crate) struct ImportPlanArgs {
    /// The saved plan markdown file to import.
    pub file: String,

    /// SPEC-ID this plan targets. If omitted, AIDA tries to detect it
    /// from the filename (a `TYPE-N` pattern like `task-42`).
    #[clap(long, value_name = "SPEC-ID")]
    pub spec: Option<String>,

    /// Land the plan as awaiting-master-review rather than canonical:
    /// tag the spec `plan-review:pending` + post a review-requested
    /// comment. `aida queue work <SPEC>` then warns before pickup.
    // trace:TASK-516 | ai:claude
    #[clap(long)]
    pub request_review: bool,
}

fn run_import_plan(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <ImportPlanArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                ImportPlanArgs {
                    file,
                    spec,
                    request_review,
                } => {
                    self::handle_import_plan_command(
                        backend,
                        store_path,
                        file,
                        spec.as_deref(),
                        *request_review,
                    )?;
                }
                #[allow(unreachable_patterns)]
                _ => crate::git_backend_cmd::exit_unsupported_on_git_backend(),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                ImportPlanArgs { .. } => {
                    // The plan-review handshake tags + comments on the spec via the
                    // git-canonical write path. trace:TASK-516 | ai:claude
                    anyhow::bail!(
                        "aida import-plan requires the distributed git-canonical store \
                     (run `aida init` to migrate, or this project is on the \
                     deprecated --centralized backend)"
                    );
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!("`aida import-plan` finished before storage init"),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static IMPORT_PLAN: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "import-plan",
        help_after: "ultraplan",
        command: <ImportPlanArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_import_plan),
    };
