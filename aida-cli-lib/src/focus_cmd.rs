//! `aida focus` command cluster (focus-scoping: set / clear / show the active
//! worktree focus).
//!
//! Thin dispatch over the shared focus-state machinery in `crate::focus`
//! (marker read/write/resolve stays there, reached via `crate::focus::*`).
//! Only the command handler and its focus-exclusive rollup/tally helpers live
//! here. Extracted verbatim from `main.rs` (SPIKE-78); no behavior change.

use anyhow::Result;
use colored::Colorize;

use aida_core::DatabaseBackend;

use crate::find_project_root;

pub(crate) fn handle_focus_command(
    target: Option<&str>,
    clear: bool,
    _show: bool,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    let project_root = find_project_root()?;

    if clear {
        if crate::focus::clear_focus_marker(&project_root) {
            println!(
                "{} focus cleared.",
                crate::glyph(crate::glyphs::Glyph::Check)
            );
        } else {
            println!("No focus was set.");
        }
        return Ok(());
    }

    if let Some(raw) = target {
        // Set: validate the target resolves to a real spec, then persist its
        // canonical display id so later reads resolve unambiguously.
        let req = backend.get_requirement_by_spec_id(raw)?.ok_or_else(|| {
            anyhow::anyhow!(
                "focus target `{}` not found. Pass an existing epic or spec id (try `aida list`).",
                raw
            )
        })?;
        let label = req.display_id();
        crate::focus::write_focus_marker(&project_root, &label)?;
        println!(
            "{} focused on {} — {}",
            crate::glyph(crate::glyphs::Glyph::Check),
            label.cyan().bold(),
            req.title,
        );
        print_focus_rollup(backend, &req)?;
        println!(
            "Read commands (list / status / queue list) now scope to this subtree. \
             `aida focus --clear` to drop it; `--all` / `--no-focus` per-command to widen."
        );
        return Ok(());
    }

    // Show (no target, or --show): the current focus + a rollup, or a hint.
    match crate::focus::resolve_focus(&project_root) {
        Some(focus_ref) => match backend.get_requirement_by_spec_id(&focus_ref)? {
            Some(req) => {
                let env_override = std::env::var(crate::focus::FOCUS_ENV)
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .is_some();
                println!(
                    "{} focused on {} — {}",
                    crate::glyph(crate::glyphs::Glyph::Bullet),
                    req.display_id().cyan().bold(),
                    req.title,
                );
                if env_override {
                    println!(
                        "  (from {} env override — unset it to fall back to .aida/focus)",
                        crate::focus::FOCUS_ENV
                    );
                }
                print_focus_rollup(backend, &req)?;
            }
            None => {
                println!(
                    "{} focus `{}` no longer resolves to a spec. \
                     Run `aida focus --clear` or re-set it.",
                    "Note:".yellow(),
                    focus_ref,
                );
            }
        },
        None => {
            println!("No focus set.");
            println!("  Set one with `aida focus <epic-or-spec>` to scope list / status / queue.");
        }
    }
    Ok(())
}

// trace:BUG-678 | ai:claude
/// Bucketed status counts for a focused subtree.
///
/// `total` is completed + in_progress + open and deliberately EXCLUDES
/// terminal `rejected` specs (surfaced separately via `rejected`), so the
/// "open" figure never counts terminal work as outstanding.
#[derive(Default, Debug, PartialEq, Eq)]
struct FocusTally {
    total: usize,
    completed: usize,
    in_progress: usize,
    open: usize,
    rejected: usize,
}

// trace:BUG-678 | ai:claude
/// Tally subtree statuses into buckets. `open` = draft/approved/planned (and any
/// other non-terminal status); `rejected` is terminal and excluded from both
/// `open` and `total`.
fn tally_focus_statuses<'a>(statuses: impl IntoIterator<Item = &'a str>) -> FocusTally {
    let mut t = FocusTally::default();
    for status in statuses {
        match status.to_ascii_lowercase().as_str() {
            "completed" | "done" => {
                t.completed += 1;
                t.total += 1;
            }
            "inprogress" | "in-progress" | "in_progress" => {
                t.in_progress += 1;
                t.total += 1;
            }
            "rejected" => t.rejected += 1,
            _ => {
                t.open += 1;
                t.total += 1;
            }
        }
    }
    t
}

/// Print a one-line status rollup of the focus spec's transitive subtree
/// (cache-fast: one descendant-id closure + one summary read).
fn print_focus_rollup(
    backend: &aida_core::CachedGitBackend,
    focus_req: &aida_core::Requirement,
) -> Result<()> {
    let subtree = backend.descendant_ids(&focus_req.id)?;
    let summaries = backend.list_summaries(&aida_core::ListFilter::default())?;
    let tally = tally_focus_statuses(
        summaries
            .iter()
            .filter(|s| subtree.contains(&s.id) && s.id != focus_req.id)
            .map(|s| s.status.as_str()),
    );
    let mut line = format!(
        "  subtree: {} item{} ({} completed · {} in-progress · {} open",
        tally.total,
        if tally.total == 1 { "" } else { "s" },
        tally.completed,
        tally.in_progress,
        tally.open,
    );
    if tally.rejected > 0 {
        line.push_str(&format!(" · {} rejected", tally.rejected));
    }
    line.push(')');
    println!("{line}");
    Ok(())
}

#[cfg(test)]
mod bug_678_focus_rollup_tally_tests {
    use super::*;

    #[test]
    fn rejected_excluded_from_open_and_total() {
        // Acceptance: one approved + one completed + one rejected → open=1,
        // completed=1, rejected excluded from open (and from total).
        let t = tally_focus_statuses(["approved", "completed", "rejected"]);
        assert_eq!(t.open, 1, "rejected must not inflate open");
        assert_eq!(t.completed, 1);
        assert_eq!(t.rejected, 1);
        assert_eq!(
            t.total, 2,
            "total = completed + in_progress + open, excludes rejected"
        );
    }

    #[test]
    fn open_bucket_is_draft_approved_planned() {
        let t = tally_focus_statuses(["draft", "approved", "planned"]);
        assert_eq!(t.open, 3);
        assert_eq!(t.total, 3);
        assert_eq!(t.rejected, 0);
    }

    #[test]
    fn epic_54_repro_reports_true_open_three() {
        // BUG-678 repro: 3 approved (open) + 9 rejected → open=3, not 12.
        let statuses: Vec<&str> = std::iter::repeat("approved")
            .take(3)
            .chain(std::iter::repeat("rejected").take(9))
            .collect();
        let t = tally_focus_statuses(statuses);
        assert_eq!(t.open, 3, "true open count, not rejected-inflated");
        assert_eq!(t.rejected, 9);
        assert_eq!(t.total, 3);
    }

    #[test]
    fn status_variants_and_case_insensitivity() {
        let t = tally_focus_statuses(["Done", "IN-PROGRESS", "in_progress", "Completed"]);
        assert_eq!(t.completed, 2);
        assert_eq!(t.in_progress, 2);
        assert_eq!(t.open, 0);
        assert_eq!(t.total, 4);
    }
}
