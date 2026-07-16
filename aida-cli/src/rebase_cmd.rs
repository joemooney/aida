//! `aida rebase` command cluster (TASK-103).
//!
//! The convenience rebase verb: `handle_rebase_command` detects + classifies
//! the current branch against its upstream (via `aida_core::rebase`), then
//! optionally executes a safe rebase (auto-stash, abort-on-conflict) and
//! reports the outcome. The private helpers are the interactive confirmation
//! prompt (`rebase_confirm`) and the human/JSON report renderer
//! (`rebase_report`). Extracted verbatim from `main.rs` (SPIKE-78); no
//! behavior change. The shared two-leg git-mirror leg helpers (code/store
//! runners used by fetch/pull/push) stay in `main.rs`.

use anyhow::Result;
use colored::Colorize;

pub(crate) fn handle_rebase_command(
    store_path: &std::path::Path,
    auto: bool,
    dry_run: bool,
    no_fetch: bool,
    no_stash: bool,
    json: bool,
    branch: Option<&str>,
) -> Result<()> {
    use aida_core::rebase::{self, DetectError};

    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // ---- Phase 1+2: detect + classify ----
    let detection = match rebase::detect(&project_root, branch, !no_fetch) {
        Ok(d) => d,
        Err(DetectError::NoUpstream(b)) => {
            // No upstream is a soft "nothing to rebase onto" — report
            // and exit 0 so scripts/skills can call this unconditionally.
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "classification": "clean",
                        "branch": b,
                        "note": "no upstream configured",
                    })
                );
            } else {
                println!(
                    "{} branch {} has no upstream — nothing to rebase onto.",
                    "·".dimmed(),
                    b.cyan()
                );
                println!(
                    "  {}",
                    "Pass --branch <ref> to pick a target explicitly.".dimmed()
                );
            }
            return Ok(());
        }
        Err(e) => anyhow::bail!("rebase detection failed: {}", e),
    };
    let class = detection.class();

    // ---- Phase 3: execute ----
    let mut executed = false;
    let mut aborted_conflict = false;
    let mut conflicts: Vec<String> = Vec::new();
    let mut refused_dirty = false;

    let want_execute = !dry_run && class.needs_rebase();
    if want_execute {
        let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
        // Safe cases run under --auto; risky cases always need a human.
        let proceed = if class.is_safe() {
            if auto {
                true
            } else if interactive {
                rebase_confirm(&format!(
                    "Rebase {} onto {} ({} behind)?",
                    detection.branch, detection.upstream, detection.behind
                ))
            } else {
                false // non-interactive, no --auto: report only
            }
        } else {
            // diverged-risky: surface the overlap, always prompt.
            if !json {
                eprintln!(
                    "{} {} file{} touched on BOTH sides — rebase may conflict:",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    detection.overlap.len(),
                    if detection.overlap.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                );
                for f in detection.overlap.iter().take(10) {
                    eprintln!("    {}", f.dimmed());
                }
            }
            interactive
                && rebase_confirm(&format!(
                    "Rebase anyway ({} ahead, {} behind, overlap exists)?",
                    detection.ahead, detection.behind
                ))
        };

        if proceed {
            if !detection.working_tree_clean && no_stash {
                refused_dirty = true;
            } else {
                let stashed = !detection.working_tree_clean;
                if stashed {
                    let _ = std::process::Command::new("git")
                        .arg("-C")
                        .arg(&project_root)
                        .args(["stash", "push", "-u", "-m", "aida rebase auto-stash"])
                        .status();
                }
                let status = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&project_root)
                    .args(["rebase", &detection.upstream])
                    .status();
                match status {
                    Ok(s) if s.success() => executed = true,
                    _ => {
                        // Conflict (or other failure): collect the
                        // conflicted paths, then abort to leave a clean
                        // state — `aida rebase` is a convenience verb,
                        // not a place to strand a half-finished rebase.
                        conflicts = std::process::Command::new("git")
                            .arg("-C")
                            .arg(&project_root)
                            .args(["diff", "--name-only", "--diff-filter=U"])
                            .output()
                            .ok()
                            .filter(|o| o.status.success())
                            .map(|o| {
                                String::from_utf8_lossy(&o.stdout)
                                    .lines()
                                    .map(|l| l.to_string())
                                    .collect()
                            })
                            .unwrap_or_default();
                        let _ = std::process::Command::new("git")
                            .arg("-C")
                            .arg(&project_root)
                            .args(["rebase", "--abort"])
                            .status();
                        aborted_conflict = true;
                    }
                }
                if stashed {
                    let _ = std::process::Command::new("git")
                        .arg("-C")
                        .arg(&project_root)
                        .args(["stash", "pop"])
                        .status();
                }
            }
        }
    }

    // ---- Phase 4: report ----
    rebase_report(
        &detection,
        class,
        executed,
        aborted_conflict,
        refused_dirty,
        &conflicts,
        json,
    );
    if aborted_conflict {
        anyhow::bail!("rebase hit conflicts and was aborted — resolve manually");
    }
    Ok(())
}

/// TASK-103: yes/no confirmation for the rebase execute phase.
fn rebase_confirm(question: &str) -> bool {
    use std::io::Write;
    eprint!("{} [y/N] ", question);
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// TASK-103: render the rebase report — human-readable, or `--json`.
fn rebase_report(
    d: &aida_core::rebase::RebaseDetection,
    class: aida_core::rebase::RebaseClass,
    executed: bool,
    aborted_conflict: bool,
    refused_dirty: bool,
    conflicts: &[String],
    json: bool,
) {
    // Suggested follow-ups, classification + outcome aware.
    let mut followups: Vec<String> = Vec::new();
    if refused_dirty {
        followups.push("commit or stash your changes, then re-run `aida rebase`".to_string());
    } else if aborted_conflict {
        followups.push(format!(
            "git rebase {} — resolve conflicts manually",
            d.upstream
        ));
    } else if executed {
        if d.ahead > 0 {
            followups.push("aida push — publish the rebased commits".to_string());
        }
    } else {
        match class {
            aida_core::rebase::RebaseClass::AheadOnly => {
                followups.push("aida push — publish your local commits".to_string());
            }
            aida_core::rebase::RebaseClass::BehindOnly
            | aida_core::rebase::RebaseClass::DivergedSafe => {
                followups.push("aida rebase --auto — execute this safe rebase".to_string());
            }
            aida_core::rebase::RebaseClass::DivergedRisky => {
                followups.push(format!(
                    "review the {} overlapping file(s), then `aida rebase` to decide",
                    d.overlap.len()
                ));
            }
            aida_core::rebase::RebaseClass::Clean => {}
        }
    }

    if json {
        let obj = serde_json::json!({
            "branch": d.branch,
            "upstream": d.upstream,
            "fetched": d.fetched,
            "ahead": d.ahead,
            "behind": d.behind,
            "classification": class.label(),
            "overlap": d.overlap,
            "working_tree_clean": d.working_tree_clean,
            "executed": executed,
            "aborted_conflict": aborted_conflict,
            "refused_dirty": refused_dirty,
            "conflicts": conflicts,
            "followups": followups,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).unwrap_or_else(|_| "{}".to_string())
        );
        return;
    }

    println!(
        "{} {} vs {}",
        "Rebase:".bold(),
        d.branch.cyan(),
        d.upstream.cyan()
    );
    println!(
        "  state      {} ahead · {} behind{}",
        d.ahead,
        d.behind,
        if d.fetched {
            ""
        } else {
            "  (--no-fetch: cached refs)"
        }
    );
    let class_colored = match class {
        aida_core::rebase::RebaseClass::Clean | aida_core::rebase::RebaseClass::AheadOnly => {
            class.label().green().to_string()
        }
        aida_core::rebase::RebaseClass::BehindOnly
        | aida_core::rebase::RebaseClass::DivergedSafe => class.label().cyan().to_string(),
        aida_core::rebase::RebaseClass::DivergedRisky => class.label().yellow().to_string(),
    };
    println!("  class      {}", class_colored);
    if !d.overlap.is_empty() {
        println!(
            "  overlap    {} file{} on both sides",
            d.overlap.len(),
            if d.overlap.len() == 1 { "" } else { "s" }
        );
        for f in d.overlap.iter().take(10) {
            println!("               {}", f.dimmed());
        }
    }
    if !d.working_tree_clean {
        println!(
            "  worktree   {} ({} dirty path(s))",
            "dirty".yellow(),
            d.dirty_files.len()
        );
    }
    if executed {
        println!("  {}", "rebase executed — branch is now up to date".green());
    } else if aborted_conflict {
        println!(
            "  {} ({} conflicted path(s)) — rebase aborted, working tree restored",
            "conflicts".red(),
            conflicts.len()
        );
        for f in conflicts.iter().take(10) {
            println!("               {}", f.dimmed());
        }
    } else if refused_dirty {
        println!(
            "  {}",
            "--no-stash and the working tree is dirty — not rebasing".yellow()
        );
    }
    if !followups.is_empty() {
        println!("  {}", "next:".bold());
        for f in &followups {
            println!("    {} {}", "·".dimmed(), f);
        }
    }
}
