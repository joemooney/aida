//! `aida goal` command cluster (TASK-242), lifted out of `main.rs` (SPIKE-78).
//!
//! Derives a machine-checkable `/goal` completion condition from AIDA metadata
//! — a batch, an epic's children, a spec, a PR, or a role queue — where each
//! flag contributes one AND-composed clause carrying its own deterministic
//! verify command. Pure formatting over the clause vocabulary; `--copy` /
//! `--invoke` / deep-link output reach shared helpers in `crate::`. Extracted
//! verbatim; no behavior change.

use anyhow::Result;
use colored::Colorize;

use crate::copy_to_clipboard;
use crate::deep_link;

// ────────────────────────────────────────────────────────────────────
// TASK-242: `aida goal` — derive machine-checkable completion conditions
// from AIDA metadata for /goal, /schedule, and autonomous-mode runs.
// trace:TASK-242 | ai:claude
// ────────────────────────────────────────────────────────────────────

/// One clause of a `/goal` completion condition: a human-readable
/// statement plus an explicit, deterministic command to verify it.
pub(crate) struct GoalClause {
    description: String,
    pub(crate) verify: String,
}

/// Build the ordered list of `GoalClause`s from the `aida goal` flags.
/// Pure — no store/IO — so the condition vocabulary is unit-testable.
/// Bails when no condition flag is given.
// trace:TASK-242 | ai:claude
pub(crate) fn build_goal_clauses(
    batch: Option<&str>,
    epic: Option<&str>,
    spec: Option<&str>,
    pr: Option<u64>,
    queue_empty: Option<&str>,
) -> Result<Vec<GoalClause>> {
    let mut clauses: Vec<GoalClause> = Vec::new();

    if let Some(name) = batch {
        let name = name.strip_prefix("batch:").unwrap_or(name);
        clauses.push(GoalClause {
            description: format!(
                "all specs tagged `batch:{}` are resolved (Completed or Rejected)",
                name
            ),
            verify: format!("`aida list --tags batch:{}` returns no rows", name),
        });
    }
    if let Some(id) = epic {
        clauses.push(GoalClause {
            description: format!(
                "all direct children of {} are resolved (Completed or Rejected)",
                id
            ),
            verify: format!("`aida list --parent {}` returns no rows", id),
        });
    }
    if let Some(id) = spec {
        clauses.push(GoalClause {
            description: format!("spec {} is status Completed", id),
            verify: format!("`aida show {}` reports `Status: Completed`", id),
        });
    }
    if let Some(n) = pr {
        clauses.push(GoalClause {
            description: format!("PR #{} is merged", n),
            verify: format!(
                "`gh pr view {} --json state --jq .state` prints `MERGED`",
                n
            ),
        });
    }
    if let Some(role) = queue_empty {
        let role = role.strip_prefix("role:").unwrap_or(role);
        clauses.push(GoalClause {
            description: format!("the {} queue is empty", role),
            verify: format!(
                "`aida queue list --for {} --no-in-flight` shows no queued items",
                role
            ),
        });
    }

    if clauses.is_empty() {
        anyhow::bail!(
            "no condition flags given — pass at least one of \
             --batch / --epic / --spec / --pr / --queue-empty"
        );
    }
    Ok(clauses)
}

/// Join clauses into a single `/goal`-pasteable condition string. Each
/// clause inlines its verification command so a Haiku-class evaluator
/// can check it deterministically.
// trace:TASK-242 | ai:claude
pub(crate) fn assemble_goal_condition(clauses: &[GoalClause]) -> String {
    clauses
        .iter()
        .map(|c| format!("{} (verify: {})", c.description, c.verify))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// `aida goal` handler — emit the condition framed, bare (`--invoke`),
/// or to the clipboard (`--copy`).
// trace:TASK-242 | ai:claude
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_goal_command(
    batch: Option<&str>,
    epic: Option<&str>,
    spec: Option<&str>,
    pr: Option<u64>,
    queue_empty: Option<&str>,
    copy: bool,
    invoke: bool,
    as_deep_link: bool,
) -> Result<()> {
    let clauses = build_goal_clauses(batch, epic, spec, pr, queue_empty)?;
    let condition = assemble_goal_condition(&clauses);
    let goal_line = format!("/goal {}", condition);

    // --invoke: bare line only, for `$(aida goal --invoke ...)`.
    if invoke {
        println!("{}", goal_line);
        return Ok(());
    }

    // SPIKE-33: emit a claude-cli:// deep link whose `q=` is the assembled
    // /goal line. Opens Claude Code in the current dir with the prompt
    // pre-filled (inert until Enter). trace:SPIKE-33 | ai:claude
    if as_deep_link {
        let cwd = std::env::current_dir().ok();
        let mut link = deep_link::DeepLink::new().with_prompt(&goal_line);
        if let Some(c) = cwd {
            link = link.with_cwd(c);
        }
        let rendered = link.render();
        if rendered.exceeds {
            println!(
                "{} the assembled /goal line exceeds Claude Code's 5000-char URL ceiling — \
                 emitting anyway, but the link may fail to open",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
        println!("{}", rendered.url);
        return Ok(());
    }

    if copy {
        if copy_to_clipboard(&goal_line) {
            println!(
                "{} copied the /goal condition to the clipboard",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            println!(
                "{} no clipboard tool found (wl-copy/xclip/xsel/pbcopy/clip) — printing instead",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
        println!();
        println!("  {}", goal_line.cyan());
        return Ok(());
    }

    // Default: framed output with the per-clause verification recipe.
    println!(
        "{} machine-checkable completion condition ({} clause{}):",
        crate::glyph(crate::glyphs::Glyph::FlowActive)
            .green()
            .bold(),
        clauses.len(),
        if clauses.len() == 1 { "" } else { "s" }
    );
    println!();
    println!("  {}", goal_line.cyan());
    println!();
    println!("  {}", "verify each clause:".dimmed());
    for c in &clauses {
        println!("    {} {}", "·".dimmed(), c.verify);
    }
    println!();
    println!(
        "  {}",
        "paste into /goal or /schedule — or re-run with --copy / --invoke".dimmed()
    );
    Ok(())
}

#[cfg(test)]
mod goal_command_tests {
    use super::{assemble_goal_condition, build_goal_clauses};

    #[test]
    fn batch_clause_strips_prefix_and_emits_verify() {
        let c = build_goal_clauses(Some("batch:plan-tooling"), None, None, None, None).unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].description.contains("`batch:plan-tooling`"));
        assert!(c[0].verify.contains("aida list --tags batch:plan-tooling"));
        // Bare name (no prefix) behaves identically.
        let c2 = build_goal_clauses(Some("plan-tooling"), None, None, None, None).unwrap();
        assert_eq!(c2[0].verify, c[0].verify);
    }

    #[test]
    fn epic_spec_pr_clauses() {
        let c = build_goal_clauses(None, Some("EPIC-23"), None, None, None).unwrap();
        assert!(c[0].verify.contains("aida list --parent EPIC-23"));

        let c = build_goal_clauses(None, None, Some("TASK-9"), None, None).unwrap();
        assert!(c[0].verify.contains("aida show TASK-9"));

        let c = build_goal_clauses(None, None, None, Some(42), None).unwrap();
        assert!(c[0].verify.contains("gh pr view 42"));
    }

    #[test]
    fn queue_empty_clause_strips_role_prefix() {
        let c = build_goal_clauses(None, None, None, None, Some("role:implementer")).unwrap();
        assert!(c[0].verify.contains("aida queue list --for implementer"));
        let c2 = build_goal_clauses(None, None, None, None, Some("implementer")).unwrap();
        assert_eq!(c2[0].verify, c[0].verify);
    }

    #[test]
    fn multiple_flags_compose_with_and() {
        let clauses = build_goal_clauses(Some("lifecycle"), None, None, Some(30), None).unwrap();
        assert_eq!(clauses.len(), 2);
        let condition = assemble_goal_condition(&clauses);
        assert!(condition.contains(" AND "));
        assert!(condition.contains("batch:lifecycle"));
        assert!(condition.contains("PR #30"));
    }

    #[test]
    fn no_flags_is_an_error() {
        assert!(build_goal_clauses(None, None, None, None, None).is_err());
    }
}
