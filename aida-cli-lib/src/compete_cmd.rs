//! `aida compete` — competitive-analysis brief assembly and multi-vendor bake-off.
//!
//! Extracted verbatim from `main.rs` (SPIKE-78 pure-movement refactor). Runs one
//! spec through N vendors headless in isolated worktrees, applies a deterministic
//! objective gate, ranks the gate-passers, optionally runs a rubric LLM judge, and
//! reports a recommended winner — report-only, never merges.
// trace:SPIKE-78 | ai:claude

use crate::*;

/// Assemble the implementer brief a compete arm hands to a vendor: the rich
/// spec context (reusing the `/ultraplan` assembly) wrapped in compete-specific
/// marching orders — implement, build+test, commit on the CURRENT branch, no
/// PR. The vendor runs headless in its own worktree, so "current branch" is the
/// per-vendor branch we already checked out for it.
// trace:STORY-659 | ai:claude
fn assemble_compete_brief(
    store: &aida_core::RequirementsStore,
    target: &aida_core::models::Requirement,
    project_root: &std::path::Path,
    gate: &str,
) -> String {
    let helpers = build_reusable_helpers_section(store, project_root, target);
    let (reservations, _warnings) = read_reserved_paths(project_root);
    let (context, _warnings) =
        assemble_ultraplan_prompt(store, target, helpers.as_deref(), true, &reservations);
    let display = target.display_id();
    let mut brief = String::new();
    brief.push_str(&format!(
        "You are competing to implement {display} in this worktree. Another vendor is \
         implementing the SAME spec in a separate worktree; your work will be judged on \
         correctness and quality, not speed.\n\n"
    ));
    brief.push_str("## Your task\n\n");
    brief.push_str(
        "1. Implement the spec below in THIS worktree (you are already on the correct branch).\n",
    );
    brief.push_str(&format!(
        "2. Build and test as you go. The objective gate that will be run is:\n   `{gate}`\n"
    ));
    brief.push_str(
        "3. Commit ALL your work on the CURRENT branch with a clear conventional-commit \
         message. Do NOT open a pull request — the operator collects every vendor's branch.\n",
    );
    brief.push_str(
        "4. Prefer reusing the codebase's existing helpers over re-deriving parallel logic.\n\n",
    );
    brief.push_str("---\n\n");
    brief.push_str(&context);
    brief
}

/// `aida compete <SPEC> --vendors <csv> [--gate <cmd>]` — run one spec through N
/// vendors headless, in isolated worktrees, then a deterministic objective gate;
/// report a table, rank the gate-passers, optionally run a rubric LLM judge
/// (`--judge`), and leave every branch in place. Report-only: it recommends a
/// winner but never merges.
// trace:STORY-659 trace:STORY-660 | ai:claude
pub(crate) fn handle_compete_command(
    spec_arg: &str,
    vendors: &[String],
    gate: Option<&str>,
    dry_run: bool,
    judge: bool,
    judge_vendor: &str,
) -> Result<()> {
    use compete::{ArmResult, Ran, ReportGlyphs, VendorAdapter};

    // Resolve the judge vendor up front so a typo fails fast (before any vendor
    // runs), not after the expensive arms. trace:TASK-869 | ai:claude
    let judge_vendor = compete::JudgeVendor::parse(judge_vendor).ok_or_else(|| {
        anyhow::anyhow!("unknown --judge-vendor `{judge_vendor}` — supported: claude, codex")
    })?;

    if vendors.is_empty() {
        anyhow::bail!(
            "no vendors given — pass --vendors claude,codex (slice 1 supports claude + codex \
             headless; antigravity is emitted as a human-run brief)"
        );
    }
    let project_root = find_project_root()?;
    let store = load_store_for_lookup(&project_root)
        .ok_or_else(|| anyhow::anyhow!("could not load the AIDA requirements store"))?;
    let target = if let Ok(uuid) = uuid::Uuid::parse_str(spec_arg) {
        store.requirements.iter().find(|r| r.id == uuid)
    } else {
        store.get_requirement_by_spec_id(spec_arg)
    }
    .ok_or_else(|| anyhow::anyhow!("requirement `{spec_arg}` not found"))?;
    let spec_id = target.display_id();

    let gate_cmd = gate.unwrap_or(compete::DEFAULT_GATE);
    let brief = assemble_compete_brief(&store, target, &project_root, gate_cmd);

    // Guard the obvious: a dirty starting tree means a vendor worktree branched
    // off HEAD won't include the operator's uncommitted work — warn, don't fail.
    if working_tree_is_dirty(&project_root) {
        eprintln!(
            "{} starting tree has uncommitted changes — vendor worktrees branch off HEAD and \
             will NOT see them. Commit or stash first for a clean comparison.",
            glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }

    println!(
        "{} competing {} across {} vendor(s): {}",
        glyph(crate::glyphs::Glyph::Robot),
        spec_id.bold(),
        vendors.len(),
        vendors.join(", ").cyan()
    );
    println!("  gate: {}", gate_cmd.dimmed());
    if dry_run {
        println!(
            "  {}",
            "(dry run — no vendor spawned, no git touched)".dimmed()
        );
    }
    println!();

    let mut results: Vec<ArmResult> = Vec::new();
    // Per-vendor files-touched, gathered for the deterministic ranking. trace:STORY-660
    let mut files_touched: Vec<(String, usize)> = Vec::new();
    for vendor in vendors {
        let adapter = match compete::vendor_adapter(vendor) {
            Some(a) => a,
            None => {
                eprintln!(
                    "{} unknown vendor `{}` — skipping (known: claude, codex, antigravity)",
                    glyph(crate::glyphs::Glyph::Warning).yellow(),
                    vendor
                );
                results.push(ArmResult {
                    vendor: vendor.clone(),
                    ran: Ran::Skipped,
                    built: None,
                    gate_passed: None,
                    diff_lines: None,
                    branch: String::new(),
                });
                continue;
            }
        };

        // Non-headless vendor (antigravity): emit a human-run brief instead of
        // trying to spawn it. This is the cross-vendor coordination path.
        if matches!(adapter, VendorAdapter::HumanBriefed) {
            if !dry_run {
                match create_agent_brief(
                    &project_root,
                    &store,
                    vendor,
                    &spec_id,
                    Some(&brief),
                    None,
                    None,
                ) {
                    Ok(path) => println!(
                        "  {} {}: no headless CLI — wrote human-run brief at {}",
                        glyph(crate::glyphs::Glyph::Mailbox),
                        vendor.cyan(),
                        path.display()
                    ),
                    Err(e) => eprintln!(
                        "{} {}: failed to write brief: {e}",
                        glyph(crate::glyphs::Glyph::Warning).yellow(),
                        vendor
                    ),
                }
            } else {
                println!(
                    "  {}: would write a human-run brief (no headless CLI)",
                    vendor
                );
            }
            results.push(ArmResult {
                vendor: vendor.clone(),
                ran: Ran::Briefed,
                built: None,
                gate_passed: None,
                diff_lines: None,
                branch: String::new(),
            });
            continue;
        }

        let VendorAdapter::Headless { command, .. } = &adapter else {
            unreachable!("non-headless handled above");
        };

        // Vendor CLI missing → skip with a clear note, keep the run going.
        if !binary_on_path(command) {
            eprintln!(
                "{} {}: `{}` not found on PATH — skipping this arm",
                glyph(crate::glyphs::Glyph::Warning).yellow(),
                vendor,
                command
            );
            results.push(ArmResult {
                vendor: vendor.clone(),
                ran: Ran::Skipped,
                built: None,
                gate_passed: None,
                diff_lines: None,
                branch: String::new(),
            });
            continue;
        }

        let branch = compete::vendor_branch(&spec_id, vendor);
        let argv = compete::headless_argv(&adapter, &brief).expect("headless adapter");

        if dry_run {
            println!(
                "  {}: would create worktree on `{}` and run: {} {}",
                vendor,
                branch,
                command,
                argv.iter()
                    .take(argv.len().saturating_sub(1))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            results.push(ArmResult {
                vendor: vendor.clone(),
                ran: Ran::Ok,
                built: None,
                gate_passed: None,
                diff_lines: None,
                branch,
            });
            continue;
        }

        match run_compete_arm(&project_root, vendor, command, &argv, &branch, gate_cmd) {
            Ok((arm, files)) => {
                files_touched.push((vendor.clone(), files));
                results.push(arm);
            }
            Err(e) => {
                eprintln!(
                    "{} {}: arm errored: {e}",
                    glyph(crate::glyphs::Glyph::Warning).yellow(),
                    vendor
                );
                results.push(ArmResult {
                    vendor: vendor.clone(),
                    ran: Ran::Failed,
                    built: None,
                    gate_passed: None,
                    diff_lines: None,
                    branch,
                });
            }
        }
    }

    println!();
    let g = ReportGlyphs {
        check: glyph(crate::glyphs::Glyph::Check).to_string(),
        cross: glyph(crate::glyphs::Glyph::Cross).to_string(),
        pending: glyph(crate::glyphs::Glyph::Pending).to_string(),
    };
    print!("{}", compete::render_report(&spec_id, &results, &g));
    println!();

    // STORY-660: after the gate, a judge step. On a dry run there are no real
    // diffs to rank/judge, so skip both and keep the report-only message.
    if dry_run {
        println!(
            "{}",
            "Branches left in place for review — pick the best, then merge it yourself \
             (report-only — no auto-merge)."
                .dimmed()
        );
        return Ok(());
    }

    // 1. Cheap deterministic ranking — ALWAYS. Smaller, focused diff first (an
    //    honest tie-breaker on its own now that BUG-575 cleaned the diff signal).
    let ranked = compete::deterministic_ranking(&results, &files_touched);
    print!("{}", compete::render_deterministic_ranking(&ranked));
    println!();

    // 2. Rubric LLM judge — opt-in (`--judge`), report-only. Gated like every
    //    other LLM call: never in tests, and it needs the claude CLI on PATH.
    if judge {
        run_compete_judge(&project_root, &spec_id, &brief, &results, judge_vendor);
    } else if !ranked.is_empty() {
        println!(
            "{}",
            "Tip: re-run with --judge for a rubric LLM verdict (spec-adherence / \
             correctness / simplicity / test-coverage) and a recommended winner."
                .dimmed()
        );
    }

    println!(
        "{}",
        "Branches left in place for review — pick the best, then merge it yourself \
         (report-only — no auto-merge)."
            .dimmed()
    );
    Ok(())
}

/// Run the opt-in rubric LLM judge over the gate-passing candidates and print
/// its verdict. REPORT-ONLY: it never merges. Gathers each gate-passing arm's
/// diff, builds the judge prompt, spawns a one-shot judge (claude or codex,
/// per `--judge-vendor`), parses the structured verdict, and renders the score
/// table + recommended winner. The judge PROMPT is vendor-independent — only the
/// executing model changes, which is what removes the self-evaluation caveat
/// (a Codex judge over a Claude-vs-Codex bake-off is no longer Claude grading
/// Claude). Any failure degrades gracefully to a note (the deterministic ranking
/// still stands).
// trace:STORY-660 trace:TASK-869 | ai:claude
fn run_compete_judge(
    project_root: &std::path::Path,
    spec_id: &str,
    spec_context: &str,
    results: &[compete::ArmResult],
    judge_vendor: compete::JudgeVendor,
) {
    // The binary defaults to the vendor's own CLI; `AIDA_COMPETE_JUDGE` overrides
    // it (parallels the other vendor-binary env knobs). trace:TASK-869
    let judge_bin = resolved_judge_binary(judge_vendor);
    if !binary_on_path(&judge_bin) {
        eprintln!(
            "{} --judge needs the `{judge_bin}` CLI on PATH — skipping the rubric judge \
             (the deterministic ranking above still applies).",
            glyph(crate::glyphs::Glyph::Warning).yellow()
        );
        return;
    }

    // Only judge gate-passers — a failing arm isn't a ship candidate. Collect
    // each one's diff vs the base it branched from.
    let mut candidates: Vec<(String, String)> = Vec::new();
    for r in results.iter().filter(|r| r.gate_passed == Some(true)) {
        if r.branch.is_empty() {
            continue;
        }
        let diff = std::process::Command::new("git")
            .args(["diff", "HEAD", &r.branch])
            .current_dir(project_root)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        if !diff.trim().is_empty() {
            candidates.push((r.vendor.clone(), diff));
        }
    }
    if candidates.len() < 2 {
        eprintln!(
            "{} fewer than 2 gate-passing candidates — nothing for the judge to rank.",
            glyph(crate::glyphs::Glyph::Warning).yellow()
        );
        return;
    }

    println!(
        "{} running {} rubric judge over {} candidate(s)...",
        glyph(crate::glyphs::Glyph::Hourglass),
        judge_vendor.as_str(),
        candidates.len()
    );
    let prompt = compete::build_judge_prompt(spec_id, spec_context, &candidates);
    let raw = match spawn_judge(project_root, judge_vendor, &prompt) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!(
                "{} judge invocation failed: {e} — deterministic ranking above stands.",
                glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            return;
        }
    };
    match compete::parse_judge_verdict(&raw) {
        Some(verdict) => {
            println!();
            print!("{}", compete::render_judge_verdict(&verdict));
            println!();
            println!(
                "{}",
                compete::render_recommended_winner(&verdict, results).bold()
            );
        }
        None => {
            eprintln!(
                "{} could not parse a structured verdict from the judge — \
                 deterministic ranking above stands.",
                glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
    }
}

/// `aida zen <SPEC> --compete` — the 2-agent bake-off (STORY-722). Fans
/// claude + codex to INDEPENDENTLY implement the same spec (separate
/// worktrees, same acceptance brief, no shared context), then judges:
/// each candidate's CI (the gate mirroring PR CI) runs first and a failing
/// candidate is eliminated, no debate; the passers then go to ONE BLIND
/// reviewer-judge that scores candidate-A/candidate-B side-by-side with
/// vendor identity stripped from everything it sees. SELECT: the
/// highest-scoring passer is merged into the current branch, the loser
/// branch is deleted (work discarded — the record is the learning), and the
/// outcome lands as one `CompeteOutcome` event row in `.aida/events.jsonl`
/// (winner vendor, per-candidate scores, spec-kind) plus a spec comment.
/// N>2 agents, AGY, runner-up grafting, and the dispatch-policy learning
/// loop are deferred follow-ons per the spec.
// trace:STORY-722 | ai:claude
pub(crate) fn handle_zen_compete(spec_arg: &str, dry_run: bool) -> Result<()> {
    use compete::{ArmResult, Ran, ReportGlyphs, VendorAdapter};

    let project_root = find_project_root()?;
    let store = load_store_for_lookup(&project_root)
        .ok_or_else(|| anyhow::anyhow!("could not load the AIDA requirements store"))?;
    let target = if let Ok(uuid) = uuid::Uuid::parse_str(spec_arg) {
        store.requirements.iter().find(|r| r.id == uuid)
    } else {
        store.get_requirement_by_spec_id(spec_arg)
    }
    .ok_or_else(|| anyhow::anyhow!("requirement `{spec_arg}` not found"))?;
    let spec_id = target.display_id();
    let spec_kind = target.req_type.to_string();

    // Fail fast: a one-armed bake-off is not a competition. Both vendor CLIs
    // must be present BEFORE any expensive arm runs.
    for vendor in compete::BAKEOFF_VENDORS {
        let Some(VendorAdapter::Headless { command, .. }) = compete::vendor_adapter(vendor) else {
            anyhow::bail!("bake-off vendor `{vendor}` has no headless adapter");
        };
        if !binary_on_path(command) {
            anyhow::bail!(
                "bake-off needs both vendor CLIs — `{command}` ({vendor}) not found on PATH"
            );
        }
    }

    let gate_cmd = compete::DEFAULT_GATE;
    let brief = assemble_compete_brief(&store, target, &project_root, gate_cmd);

    if working_tree_is_dirty(&project_root) {
        eprintln!(
            "{} starting tree has uncommitted changes — candidate worktrees branch off HEAD \
             and will NOT see them, and the winner-merge may conflict. Commit or stash first.",
            glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }

    println!(
        "{} bake-off for {}: {} implement independently; CI eliminates, a blind \
         reviewer picks, the winner merges.",
        glyph(crate::glyphs::Glyph::Robot),
        spec_id.bold(),
        compete::BAKEOFF_VENDORS.join(" vs ").cyan()
    );
    println!("  CI gate: {}", gate_cmd.dimmed());
    if dry_run {
        for vendor in compete::BAKEOFF_VENDORS {
            let branch = compete::vendor_branch(&spec_id, vendor);
            println!(
                "  {}: would create worktree {} on `{}`, implement headless, then gate",
                vendor,
                compete_worktree_dir(&project_root, &branch).display(),
                branch
            );
        }
        println!(
            "  {}",
            "(dry run — no vendor spawned, no git touched, no judge, no merge)".dimmed()
        );
        return Ok(());
    }
    println!();

    // ── FAN: both vendors, separate worktrees, same brief, no shared context ──
    let mut results: Vec<ArmResult> = Vec::new();
    for vendor in compete::BAKEOFF_VENDORS {
        let adapter = compete::vendor_adapter(vendor).expect("bake-off vendors are known");
        let VendorAdapter::Headless { command, .. } = &adapter else {
            unreachable!("bake-off vendors are headless (checked above)");
        };
        let branch = compete::vendor_branch(&spec_id, vendor);
        let argv = compete::headless_argv(&adapter, &brief).expect("headless adapter");
        match run_compete_arm(&project_root, vendor, command, &argv, &branch, gate_cmd) {
            Ok((arm, _files)) => results.push(arm),
            Err(e) => {
                eprintln!(
                    "{} {}: arm errored: {e}",
                    glyph(crate::glyphs::Glyph::Warning).yellow(),
                    vendor
                );
                results.push(ArmResult {
                    vendor: vendor.to_string(),
                    ran: Ran::Failed,
                    built: None,
                    gate_passed: None,
                    diff_lines: None,
                    branch,
                });
            }
        }
    }

    println!();
    let g = ReportGlyphs {
        check: glyph(crate::glyphs::Glyph::Check).to_string(),
        cross: glyph(crate::glyphs::Glyph::Cross).to_string(),
        pending: glyph(crate::glyphs::Glyph::Pending).to_string(),
    };
    print!("{}", compete::render_report(&spec_id, &results, &g));
    println!();

    // ── JUDGE step 1: CI eliminates, no debate. A passer must also have real
    // work on its branch — an empty diff "passes CI" by doing nothing. ──
    let mut passers: Vec<(String, String, String)> = Vec::new(); // (vendor, branch, diff)
    for r in &results {
        if r.gate_passed != Some(true) || r.branch.is_empty() {
            continue;
        }
        let diff = std::process::Command::new("git")
            .args(["diff", "HEAD", &r.branch])
            .current_dir(&project_root)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        if diff.trim().is_empty() {
            println!(
                "  {} {}: CI passed but the branch has NO changes — eliminated (no work).",
                glyph(crate::glyphs::Glyph::Cross),
                r.vendor
            );
            continue;
        }
        passers.push((r.vendor.clone(), r.branch.clone(), diff));
    }
    if passers.is_empty() {
        anyhow::bail!(
            "no candidate passed CI — no winner. Candidate branches are left in place for \
             inspection ({}).",
            results
                .iter()
                .filter(|r| !r.branch.is_empty())
                .map(|r| r.branch.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // ── JUDGE step 2: ONE BLIND reviewer over the passers (skipped on a
    // walkover — CI elimination already decided). ──
    let (winner_vendor, verdict) = if passers.len() == 1 {
        println!(
            "  {} single CI-passer — {} wins by elimination (blind review not needed).",
            glyph(crate::glyphs::Glyph::Check),
            passers[0].0.cyan()
        );
        (passers[0].0.clone(), None)
    } else {
        let judge_vendor = compete::JudgeVendor::default();
        let judge_bin = resolved_judge_binary(judge_vendor);
        if !binary_on_path(&judge_bin) {
            anyhow::bail!(
                "the blind reviewer-judge needs the `{judge_bin}` CLI on PATH — no winner \
                 selected; candidate branches left in place."
            );
        }
        // Blind the candidates: labels only, vendor identity stripped from
        // everything the reviewer sees; label order randomized per run so the
        // labeling is not a learnable convention.
        let cands: Vec<(String, String)> = passers
            .iter()
            .map(|(v, _b, d)| (v.clone(), d.clone()))
            .collect();
        let swap = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() % 2 == 1)
            .unwrap_or(false);
        let (labeled, mapping) = compete::blind_candidates(&cands, swap);
        println!(
            "{} blind reviewer-judge scoring {} candidates (vendor identity stripped)...",
            glyph(crate::glyphs::Glyph::Hourglass),
            labeled.len()
        );
        let prompt = compete::build_blind_judge_prompt(&spec_id, &brief, &labeled);
        let raw = spawn_judge(&project_root, judge_vendor, &prompt)?;
        let blind_verdict = compete::parse_judge_verdict(&raw).ok_or_else(|| {
            anyhow::anyhow!(
                "could not parse a structured verdict from the blind reviewer — no winner \
                 selected; candidate branches left in place."
            )
        })?;
        let verdict = compete::unblind_verdict(&blind_verdict, &mapping);
        println!();
        print!("{}", compete::render_judge_verdict(&verdict));
        println!();
        // The winner must be an actual passer. If the reviewer went
        // off-contract on the winner field, fall back to its own score table
        // (highest total among passers); a fully unusable verdict is a bail.
        let winner = if passers.iter().any(|(v, _, _)| *v == verdict.winner) {
            verdict.winner.clone()
        } else {
            verdict
                .scores
                .iter()
                .filter(|s| passers.iter().any(|(v, _, _)| *v == s.vendor))
                .max_by_key(|s| s.total())
                .map(|s| s.vendor.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "the blind reviewer's verdict named no known candidate — no winner \
                         selected; candidate branches left in place."
                    )
                })?
        };
        (winner, Some(verdict))
    };

    // ── SELECT: merge the winner into the current branch. ──
    let winner_branch = passers
        .iter()
        .find(|(v, _, _)| *v == winner_vendor)
        .map(|(_, b, _)| b.clone())
        .expect("winner is a passer");
    let current_branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&project_root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "HEAD".to_string());
    println!(
        "{} winner: {} — merging `{}` into `{}`...",
        glyph(crate::glyphs::Glyph::Check).green(),
        winner_vendor.bold(),
        winner_branch,
        current_branch
    );
    let merge_msg =
        format!("[AI:{winner_vendor}] feat: {spec_id} via compete bake-off winner ({spec_id})");
    let merge = std::process::Command::new("git")
        .args(["merge", "--no-ff", &winner_branch, "-m", &merge_msg])
        .current_dir(&project_root)
        .output()
        .context("failed to run `git merge`")?;
    if !merge.status.success() {
        // Leave the merge state for the operator; don't half-clean.
        let _ = std::process::Command::new("git")
            .args(["merge", "--abort"])
            .current_dir(&project_root)
            .output();
        anyhow::bail!(
            "merging the winner failed: {} — no branches deleted; resolve manually \
             (winner branch: {winner_branch}).",
            String::from_utf8_lossy(&merge.stderr).trim()
        );
    }

    // ── Cleanup: worktrees go; the loser branch is deleted (work discarded —
    // the record is the learning); the merged winner branch is deleted too. ──
    for r in results.iter().filter(|r| !r.branch.is_empty()) {
        let dir = compete_worktree_dir(&project_root, &r.branch);
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force", &dir.to_string_lossy()])
            .current_dir(&project_root)
            .output();
        let flag = if r.branch == winner_branch {
            "-d"
        } else {
            "-D"
        };
        let _ = std::process::Command::new("git")
            .args(["branch", flag, &r.branch])
            .current_dir(&project_root)
            .output();
        if r.branch != winner_branch {
            println!("  loser branch `{}` deleted — work discarded.", r.branch);
        }
    }
    let _ = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(&project_root)
        .output();

    // ── RECORD: one CompeteOutcome event row + a spec comment — no new file
    // format. This is the dispatch-policy data point. ──
    let scores: Vec<crate::events::CompeteCandidateScore> = results
        .iter()
        .filter(|r| !r.branch.is_empty())
        .map(|r| crate::events::CompeteCandidateScore {
            vendor: r.vendor.clone(),
            ci_passed: r.gate_passed == Some(true),
            rubric_total: verdict.as_ref().and_then(|v| {
                v.scores
                    .iter()
                    .find(|s| s.vendor == r.vendor)
                    .map(|s| s.total())
            }),
        })
        .collect();
    crate::events::emit(
        &project_root,
        &crate::events::Event::new(
            Some(spec_id.clone()),
            "",
            crate::events::EventKind::CompeteOutcome {
                winner: winner_vendor.clone(),
                scores: scores.clone(),
                spec_kind: spec_kind.clone(),
            },
        ),
    );
    let scores_line = scores
        .iter()
        .map(|s| {
            let rubric = s
                .rubric_total
                .map(|t| format!(", rubric {t}/20"))
                .unwrap_or_default();
            let ci = if s.ci_passed {
                "CI pass"
            } else {
                "CI fail (eliminated)"
            };
            format!("{}: {ci}{rubric}", s.vendor)
        })
        .collect::<Vec<_>>()
        .join("; ");
    let comment_text = format!(
        "Compete bake-off (zen --compete) outcome: winner {winner_vendor}, merged into \
         `{current_branch}`; loser branch deleted, work discarded. Scores — {scores_line}. \
         Spec-kind: {spec_kind}. Reviewer was blind (candidate labels; vendor identity \
         stripped)."
    );
    // Route the comment through the CLI's own dispatch so it lands correctly
    // on either storage backend (git-canonical or legacy). Best-effort.
    let aida = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("aida"));
    let commented = std::process::Command::new(&aida)
        .args([
            "comment",
            "add",
            &spec_id,
            &comment_text,
            "--author",
            "aida-compete",
        ])
        .current_dir(&project_root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !commented {
        eprintln!(
            "{} could not add the outcome comment to {} — the events.jsonl record stands.",
            glyph(crate::glyphs::Glyph::Warning).yellow(),
            spec_id
        );
    }

    println!();
    println!(
        "{} bake-off complete: {} won for {} — merged into `{}`; outcome recorded \
         (events.jsonl + spec comment).",
        glyph(crate::glyphs::Glyph::Check).green(),
        winner_vendor.bold(),
        spec_id.bold(),
        current_branch
    );
    Ok(())
}

/// The judge binary that would be spawned: the `AIDA_COMPETE_JUDGE` env
/// override when set (parallels the other vendor-binary env knobs), else the
/// vendor's own CLI. Shared by the opt-in `--judge` rubric judge (TASK-869)
/// and the `aida zen --compete` blind reviewer-judge (STORY-722).
// trace:TASK-869 trace:STORY-722 | ai:claude
fn resolved_judge_binary(judge_vendor: compete::JudgeVendor) -> String {
    std::env::var("AIDA_COMPETE_JUDGE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| judge_vendor.default_binary().to_string())
}

/// Spawn a one-shot judge process over `prompt` and return its combined
/// stdout+stderr. Resolves the binary via [`resolved_judge_binary`], builds
/// the vendor argv, and closes stdin where the vendor needs it (codex `exec`
/// reads stdin and would hang non-interactively). The prompt is identical
/// across vendors — only the executing model changes.
// trace:TASK-869 trace:STORY-722 | ai:claude
fn spawn_judge(
    project_root: &std::path::Path,
    judge_vendor: compete::JudgeVendor,
    prompt: &str,
) -> Result<String> {
    let binary_override = std::env::var("AIDA_COMPETE_JUDGE").ok();
    let (cmd_bin, cmd_args) =
        compete::judge_command(judge_vendor, binary_override.as_deref(), prompt);
    let mut judge_cmd = std::process::Command::new(&cmd_bin);
    judge_cmd.args(&cmd_args).current_dir(project_root);
    if judge_vendor.needs_stdin_closed() {
        judge_cmd.stdin(std::process::Stdio::null());
    }
    let o = judge_cmd.output().context("failed to spawn the judge")?;
    let mut s = String::from_utf8_lossy(&o.stdout).to_string();
    s.push_str(&String::from_utf8_lossy(&o.stderr));
    Ok(s)
}

/// Run a single headless vendor arm: create the worktree, spawn the vendor,
/// ensure its work is committed, then run the objective gate and measure the
/// diff. Returns the assembled [`compete::ArmResult`]. Any I/O failure bubbles
/// up so the caller records the arm as `Failed` and continues.
// trace:STORY-659
fn run_compete_arm(
    project_root: &std::path::Path,
    vendor: &str,
    command: &str,
    argv: &[String],
    branch: &str,
    gate_cmd: &str,
) -> Result<(compete::ArmResult, usize)> {
    use compete::{ArmResult, Ran};

    let worktree_dir = compete_worktree_dir(project_root, branch);
    let worktree_str = worktree_dir.to_string_lossy().to_string();

    // Fresh start: drop any stale worktree/branch from a prior compete run.
    let _ = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", &worktree_str])
        .current_dir(project_root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(project_root)
        .output();
    let _ = std::process::Command::new("git")
        .args(["branch", "-D", branch])
        .current_dir(project_root)
        .output();

    let add = std::process::Command::new("git")
        .args(["worktree", "add", "-b", branch, &worktree_str, "HEAD"])
        .current_dir(project_root)
        .output()
        .context("failed to run `git worktree add`")?;
    if !add.status.success() {
        anyhow::bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        );
    }

    // BUG-575 defense-in-depth: even though the logs now live outside the
    // worktree, belt-and-suspenders exclude the legacy log glob in the worktree's
    // private exclude file so a stray run-log never lands in the candidate's
    // `git add -A` auto-commit. trace:BUG-575 | ai:claude
    exclude_compete_logs_in_worktree(&worktree_dir);

    println!(
        "  {} {}: worktree {} on `{}` — running headless...",
        glyph(crate::glyphs::Glyph::InFlight),
        vendor.cyan(),
        worktree_dir.display(),
        branch
    );

    // BUG-575: the vendor run-log and gate-log must live OUTSIDE the candidate
    // worktree, otherwise the `git add -A` auto-commit below sweeps them into the
    // vendor's branch — inflating its diff (a verbose vendor committed an
    // 8138-line log) and leaking the log onto main if that arm is merged. Write
    // both logs to a sibling run-log dir under ~/.aida/compete/<run>/ (falls back
    // to a sibling temp dir) so the candidate branch contains ONLY vendor work.
    // trace:BUG-575 | ai:claude
    let log_dir = compete_log_dir(project_root, branch);
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join(format!("{vendor}.log"));
    let vendor_out = std::process::Command::new(command)
        .args(argv)
        .current_dir(&worktree_dir)
        .output();
    let ran = match &vendor_out {
        Ok(o) if o.status.success() => Ran::Ok,
        Ok(_) => Ran::Failed,
        Err(_) => Ran::Failed,
    };
    if let Ok(o) = &vendor_out {
        let mut combined = o.stdout.clone();
        combined.extend_from_slice(&o.stderr);
        let _ = std::fs::write(&log_path, &combined);
    }
    println!(
        "     {} vendor exited ({}) — log: {}",
        glyph(crate::glyphs::Glyph::SubArrow),
        ran.label(),
        log_path.display()
    );

    // Ensure the work is committed on the per-vendor branch even if the vendor
    // left changes uncommitted.
    let _ = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&worktree_dir)
        .output();
    let has_staged = !std::process::Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(&worktree_dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(true);
    if has_staged {
        let _ = std::process::Command::new("git")
            .args([
                "commit",
                "-m",
                &format!("[AI:{vendor}] compete arm: uncommitted vendor work (auto-committed)"),
            ])
            .current_dir(&worktree_dir)
            .output();
    }

    // Diff vs the base (HEAD the worktree branched from) — coarse "how much".
    let numstat = std::process::Command::new("git")
        .args(["diff", "--numstat", "HEAD", branch])
        .current_dir(project_root)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let diff_lines = Some(compete::count_diff_lines(&numstat));
    // Files touched = non-empty `--numstat` rows (BUG-575 keeps run-logs out of
    // this count, so it's an honest "how many files" signal for the ranking).
    let files_touched = numstat.lines().filter(|l| !l.trim().is_empty()).count();

    // Run the deterministic objective gate in the worktree.
    println!(
        "     {} running gate...",
        glyph(crate::glyphs::Glyph::Hourglass)
    );
    let gate_out = std::process::Command::new("sh")
        .args(["-c", gate_cmd])
        .current_dir(&worktree_dir)
        .output()
        .context("failed to run the gate command")?;
    let mut gate_combined = String::from_utf8_lossy(&gate_out.stdout).to_string();
    gate_combined.push_str(&String::from_utf8_lossy(&gate_out.stderr));
    let gate_log = log_dir.join(format!("{vendor}-gate.log"));
    let _ = std::fs::write(&gate_log, &gate_combined);
    let (built, gate_passed) =
        compete::parse_gate_result(gate_out.status.success(), &gate_combined);

    Ok((
        ArmResult {
            vendor: vendor.to_string(),
            ran,
            built: Some(built),
            gate_passed: Some(gate_passed),
            diff_lines,
            branch: branch.to_string(),
        },
        files_touched,
    ))
}

/// The worktree directory a compete arm uses for `branch` — a sibling of the
/// project root, so vendor work never lands inside the operator's tree. One
/// source of truth so the STORY-722 bake-off cleanup removes exactly what
/// [`run_compete_arm`] created.
// trace:STORY-722 | ai:claude
fn compete_worktree_dir(project_root: &std::path::Path, branch: &str) -> std::path::PathBuf {
    project_root
        .parent()
        .unwrap_or(project_root)
        .join(format!("aida-compete-{}", branch.replace('/', "-")))
}

/// Where a compete run's per-vendor logs live — OUTSIDE every candidate
/// worktree so they never get auto-committed into a vendor branch (BUG-575).
/// Prefers `~/.aida/compete/<run>/`; falls back to a sibling temp dir next to
/// the project when the home dir can't be resolved. `<run>` is derived from the
/// branch (which is namespaced per spec+vendor) so concurrent runs don't clash.
// trace:BUG-575 | ai:claude
fn compete_log_dir(project_root: &std::path::Path, branch: &str) -> std::path::PathBuf {
    let run_slug = branch.replace('/', "-");
    if let Some(home) = dirs::home_dir() {
        return home.join(".aida").join("compete").join(run_slug);
    }
    project_root
        .parent()
        .unwrap_or(project_root)
        .join(format!("aida-compete-logs-{run_slug}"))
}

/// Belt-and-suspenders: add the legacy run-log glob to the worktree's private
/// `.git/info/exclude` so a stray `.aida-compete-*.log` is never swept into the
/// candidate's `git add -A` auto-commit. Idempotent.
// trace:BUG-575 | ai:claude
fn exclude_compete_logs_in_worktree(worktree_dir: &std::path::Path) {
    // In a linked worktree, `.git` is a file pointing at the real gitdir; resolve
    // it via `git rev-parse --git-path info/exclude` so we write the right file.
    let exclude_path = std::process::Command::new("git")
        .args(["rev-parse", "--git-path", "info/exclude"])
        .current_dir(worktree_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|p| !p.is_empty());
    let Some(rel) = exclude_path else { return };
    let abs = if std::path::Path::new(&rel).is_absolute() {
        std::path::PathBuf::from(&rel)
    } else {
        worktree_dir.join(&rel)
    };
    let entry = ".aida-compete-*.log";
    let existing = std::fs::read_to_string(&abs).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == entry) {
        return;
    }
    if let Some(parent) = abs.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut contents = existing;
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(entry);
    contents.push('\n');
    let _ = std::fs::write(&abs, contents);
}

/// Is `name` an executable on PATH? Cheap probe via `<name> --version`, matching
/// every other launcher's availability check in this crate.
// trace:STORY-659
fn binary_on_path(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
