//! `aida lifecycle` command handler — spec-state state-machine view/visualization.
//!
//! Extracted verbatim from `main.rs` (SPIKE-78, pure movement). The declared
//! state machine itself lives in `aida_core::lifecycle`; this module is only the
//! CLI handler plus its render helpers exclusive to `aida lifecycle`.

use crate::*;

/// `aida lifecycle --diagram [--check [--write]] [--doc <FILE>]` — Phase 1 of
/// SPIKE-56 (TASK-737), generate-only. The spec-state transition model lives in
/// `aida_core::lifecycle` as the single declared source; this renders it to a
/// Mermaid `stateDiagram-v2`. `--check` pins the committed mermaid block in
/// `docs/lifecycle.md` against the generated one and exits non-zero on drift
/// (pre-commit-hook-able); `--write` inserts/refreshes that block. No guard
/// enforcement, no empirical diffing — those are sibling phases. Zero behavior
/// change to any other command.
// trace:TASK-737 | ai:claude
pub(crate) fn handle_lifecycle_command(
    diagram: bool,
    check: bool,
    write: bool,
    doc: Option<&str>,
    empirical: bool,
    diff: bool,
) -> Result<()> {
    use aida_core::lifecycle::{fenced_mermaid, first_mermaid_block, LifecycleModel};

    let model = LifecycleModel::declared();

    // Phase 3 (TASK-742): empirical reconstruction + declared-vs-observed diff.
    // `--diff` implies `--empirical`. These read the spec store's `history:`
    // arrays and do not touch the diagram/pin path. trace:TASK-742 | ai:claude
    if empirical || diff {
        return handle_lifecycle_empirical(&model, diff);
    }

    let generated_body = model.to_mermaid();

    // Resolve the pinned doc path: explicit --doc, else docs/lifecycle.md under
    // the project root, else relative to cwd.
    let doc_path = if let Some(d) = doc {
        std::path::PathBuf::from(d)
    } else {
        let root = find_project_root().unwrap_or_else(|_| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
        root.join("docs").join("lifecycle.md")
    };

    // --check (optionally with --write) pins the committed diagram.
    if check {
        let exists = doc_path.exists();
        let markdown = if exists {
            std::fs::read_to_string(&doc_path)
                .with_context(|| format!("reading {}", doc_path.display()))?
        } else {
            String::new()
        };
        let committed = first_mermaid_block(&markdown);

        let matches = committed.as_deref().is_some_and(|body| {
            aida_core::scaffolding::generated_text_matches(body, &generated_body)
        });
        if matches {
            println!(
                "lifecycle diagram pin: OK — committed diagram in {} matches the declared model.",
                doc_path.display()
            );
            return Ok(());
        }

        if write {
            let updated = if let Some(existing) = committed {
                // Replace the first mermaid block body in place.
                replace_first_mermaid_block(&markdown, &existing, &generated_body)
            } else if exists {
                // No mermaid block yet — append a fenced one.
                let mut s = markdown;
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                s.push('\n');
                s.push_str(&fenced_mermaid(&model));
                s
            } else {
                // Doc doesn't exist — create it with just the fenced block.
                if let Some(parent) = doc_path.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                fenced_mermaid(&model)
            };
            std::fs::write(&doc_path, updated)
                .with_context(|| format!("writing {}", doc_path.display()))?;
            println!(
                "lifecycle diagram pin: updated committed diagram in {} to match the declared model.",
                doc_path.display()
            );
            return Ok(());
        }

        // Drift, no --write: report and exit non-zero so a pre-commit hook fails.
        eprintln!(
            "lifecycle diagram pin: DRIFT — the committed diagram in {} does not match the declared model.",
            doc_path.display()
        );
        if committed.is_none() {
            eprintln!("  (no mermaid block found in the doc)");
        }
        eprintln!("  Run `aida lifecycle --check --write` to refresh it, or `aida lifecycle --diagram` to inspect the generated diagram.");
        std::process::exit(1);
    }

    // --diagram (default action): print the generated diagram. If neither
    // --diagram nor --check is given, still print it (the only useful default).
    let _ = diagram;
    print!("{generated_body}");
    Ok(())
}

/// Phase 3 (TASK-742): reconstruct the observed state machine from the spec
/// store's `history:` arrays and either print it (`--empirical`) or diff it
/// against the declared model (`--diff`, which implies `--empirical`).
///
/// Reads the same per-spec `history:` arrays that `aida history events`
/// walks: each spec's `HistoryEntry` carries a `changes:` list, and we keep the
/// `{field_name == "status"}` triples as observed `old_value → new_value`
/// flips. Exits non-zero from `--diff` when any undocumented flip is found, so
/// it is CI-gate-able.
// trace:TASK-742 | ai:claude
fn handle_lifecycle_empirical(
    declared: &aida_core::lifecycle::LifecycleModel,
    diff: bool,
) -> Result<()> {
    use aida_core::lifecycle::{self, EmpiricalModel};

    let project_root = find_project_root().unwrap_or_else(|_| {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });
    let store = load_store_for_lookup(&project_root)
        .context("could not load the requirement store to walk history")?;

    // For each spec, project its history array down to status flips. The store
    // is the source of truth for the per-spec `history:` arrays (CLAUDE.md:
    // \"This is the source-of-truth for spec-state time series\").
    let per_spec: Vec<Vec<(String, String)>> = store
        .requirements
        .iter()
        .map(|req| {
            req.history
                .iter()
                .flat_map(|entry| entry.changes.iter())
                .filter(|chg| chg.field_name == "status")
                .map(|chg| (chg.old_value.clone(), chg.new_value.clone()))
                .collect::<Vec<_>>()
        })
        .collect();

    let empirical = EmpiricalModel::from_status_changes(per_spec);

    if !diff {
        // --empirical: print the reconstructed observed machine.
        println!(
            "Observed spec-state transitions (from {} status flips across {} specs):",
            empirical.total_flips, empirical.specs_with_history
        );
        if empirical.transitions.is_empty() {
            println!("  (no status transitions recorded in any spec's history)");
            return Ok(());
        }
        println!();
        for t in &empirical.transitions {
            let from_lbl = t.from.map(|s| s.label()).unwrap_or(t.from_raw.as_str());
            let to_lbl = t.to.map(|s| s.label()).unwrap_or(t.to_raw.as_str());
            let unknown = if t.from.is_none() || t.to.is_none() {
                "  (unrecognized status)"
            } else {
                ""
            };
            println!(
                "  {} → {}  ×{}{}",
                from_lbl.cyan(),
                to_lbl.green(),
                t.count,
                unknown.red()
            );
        }
        return Ok(());
    }

    // --diff: declared-vs-observed.
    let d = lifecycle::diff(declared, &empirical);

    println!(
        "Lifecycle diff — declared model vs {} observed status flips across {} specs.\n",
        empirical.total_flips, empirical.specs_with_history
    );

    if d.undocumented.is_empty() {
        println!(
            "{} No undocumented observed transitions — every observed flip is in the declared model.",
            "OK:".green()
        );
    } else {
        println!(
            "{} Observed transitions NOT in the declared model (undocumented / illegal flips):",
            "DIVERGENCE:".red()
        );
        for t in &d.undocumented {
            let from_lbl = t.from.map(|s| s.label()).unwrap_or(t.from_raw.as_str());
            let to_lbl = t.to.map(|s| s.label()).unwrap_or(t.to_raw.as_str());
            let unknown = if t.from.is_none() || t.to.is_none() {
                "  (unrecognized status value)"
            } else {
                ""
            };
            println!(
                "  {} → {}  ×{}{}",
                from_lbl.yellow(),
                to_lbl.yellow(),
                t.count,
                unknown.red()
            );
        }
    }

    println!();
    if d.unobserved.is_empty() {
        println!(
            "{} Every declared transition has been observed at least once.",
            "OK:".green()
        );
    } else {
        println!(
            "{} Declared transitions never observed (dead edges):",
            "NOTE:".dimmed()
        );
        for t in &d.unobserved {
            println!(
                "  {} → {}  ({})",
                t.from.label().dimmed(),
                t.to.label().dimmed(),
                t.verb.dimmed()
            );
        }
        println!(
            "  {}",
            "(the `[*] → Draft` entry edge and `Completed → Released` are status-less by design — \
             release is a git tag, not a status flip — so they show here normally.)"
                .dimmed()
        );
    }

    if d.has_undocumented() {
        std::process::exit(1);
    }
    Ok(())
}

/// Replace the body of the first ```mermaid fenced block in `markdown` with
/// `new_body`, leaving everything else byte-identical. `old_body` is the exact
/// body returned by [`first_mermaid_block`] (used to locate the block).
// trace:TASK-737 | ai:claude
fn replace_first_mermaid_block(markdown: &str, old_body: &str, new_body: &str) -> String {
    // Reconstruct the fenced form of the old body (the extractor strips fences),
    // then swap. We locate the opening fence and the matching close by walking
    // lines, the same way first_mermaid_block does.
    let lines: Vec<&str> = markdown.lines().collect();
    let mut open_idx = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("```mermaid") {
            open_idx = Some(i);
            break;
        }
    }
    let Some(open) = open_idx else {
        return markdown.to_string();
    };
    // Find the close fence after `open`.
    let mut close_idx = None;
    for (offset, line) in lines.iter().enumerate().skip(open + 1) {
        if line.trim_start().starts_with("```") {
            close_idx = Some(offset);
            break;
        }
    }
    let Some(close) = close_idx else {
        return markdown.to_string();
    };
    let _ = old_body; // located positionally; old_body kept for clarity/API symmetry

    // Rebuild: lines[..=open], new_body lines, lines[close..].
    let mut out = String::new();
    for line in &lines[..=open] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(new_body); // new_body already ends with '\n'
    for line in &lines[close..] {
        out.push_str(line);
        out.push('\n');
    }
    // Preserve a trailing newline iff the original had one.
    if !markdown.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod bug1555_tests {
    #[test]
    fn lifecycle_pin_ignores_platform_newlines_but_not_model_drift() {
        // Mirrors the comparison in the command without invoking its process-exit path.
        // trace:BUG-1555 | ai:codex
        assert!(aida_core::scaffolding::generated_text_matches(
            "stateDiagram-v2\r\n  Draft --> Approved\r\n",
            "stateDiagram-v2\n  Draft --> Approved\n",
        ));
        assert!(!aida_core::scaffolding::generated_text_matches(
            "stateDiagram-v2\r\n  Draft --> Rejected\r\n",
            "stateDiagram-v2\n  Draft --> Approved\n",
        ));
    }
}

/// Spec-state lifecycle tooling. Today (generate-only): `aida lifecycle
/// --diagram` prints a Mermaid state diagram generated from the single
/// declared transition model in `aida-core`, so the picture can't drift
/// from the code. `--check` pins the committed diagram in
/// `docs/lifecycle.md` against the generated one and exits non-zero on
/// drift (pre-commit-hook-able); with `--write` it inserts/refreshes the
/// committed block instead. Phase 1 is purely additive — no guard
/// enforcement, no empirical diffing.
// trace:TASK-737 | ai:claude
// trace:TASK-852 | ai:claude — hidden from top-level --help (still runs).
#[derive(clap::Parser, Debug)]
#[clap(name = "lifecycle")]
#[clap(hide = true)]
pub(crate) struct LifecycleArgs {
    /// Emit the Mermaid `stateDiagram-v2` generated from the declared
    /// transition model.
    #[clap(long)]
    pub diagram: bool,

    /// Compare the generated diagram against the committed mermaid block
    /// in `docs/lifecycle.md` (or `--doc <FILE>`). Exits non-zero on
    /// drift. Pair with `--write` to fix drift in place.
    #[clap(long)]
    pub check: bool,

    /// Insert or refresh the committed mermaid block in the doc with the
    /// generated diagram. Use with `--check` to auto-fix drift.
    #[clap(long)]
    pub write: bool,

    /// The doc whose first mermaid block is the pinned diagram. Defaults
    /// to `docs/lifecycle.md` under the project root.
    #[clap(long, value_name = "FILE")]
    pub doc: Option<String>,

    /// Reconstruct the OBSERVED state machine by walking the `history:`
    /// arrays in the spec store — every recorded status flip becomes one
    /// observed transition, tallied with counts. On its own, prints the
    /// observed transitions; pair with `--diff` to compare against the
    /// declared model.
    // trace:TASK-742 | ai:claude
    #[clap(long)]
    pub empirical: bool,

    /// Diff the declared model against the observed one: report observed
    /// transitions the declared model never authorized (undocumented
    /// flips) and declared transitions never observed (dead edges). Exits
    /// non-zero when any undocumented flip is found, so it is CI-gate-able.
    /// Implies `--empirical`.
    // trace:TASK-742 | ai:claude
    #[clap(long)]
    pub diff: bool,
}

// `aida lifecycle` (Phase 1, generate-only) is self-contained: it renders a
// Mermaid diagram from the declared transition model in aida-core and
// optionally pins it against `docs/lifecycle.md`. No storage handle, no LLM,
// no behavior change to any other command. trace:TASK-737 | ai:claude
fn run_lifecycle(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <LifecycleArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let LifecycleArgs {
        diagram,
        check,
        write,
        doc,
        empirical,
        diff,
    } = &args;
    return self::handle_lifecycle_command(
        *diagram,
        *check,
        *write,
        doc.as_deref(),
        *empirical,
        *diff,
    );
}

// trace:STORY-1728 | ai:claude
pub(crate) static LIFECYCLE: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "lifecycle",
        help_after: "verify",
        command: <LifecycleArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_lifecycle),
    };
