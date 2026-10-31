//! `aida compete` — competitive-analysis brief assembly and multi-vendor bake-off.
//!
//! Extracted verbatim from `main.rs` (SPIKE-78 pure-movement refactor). Runs one
//! spec through N vendors headless in isolated worktrees, applies a deterministic
//! objective gate, ranks the gate-passers, optionally runs a rubric LLM judge, and
//! reports a recommended winner — report-only, never merges.
// trace:SPIKE-78 | ai:claude

use crate::process_retry::RetryEtxtbsy;
use crate::*;

/// Assemble the implementer brief a compete arm hands to a vendor: the rich
/// spec context (reusing the AIDA planning-prompt assembly) wrapped in compete-specific
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
    // Refuse the entire run before briefs, worktrees, branches or any arm run.
    // trace:BUG-2172 | ai:codex
    preflight_compete_containment(vendors, judge.then_some(judge_vendor))?;
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
            // Preview vendor posture without preparing settings or an OS wrapper:
            // both operational helpers can write artifacts in an existing tree.
            let actual_args =
                compete_arm_argv(&argv, vendor, session::aida_contained_env()?, || {
                    Ok(vec![
                        "--setting-sources".into(),
                        "project".into(),
                        // A JSON stand-in: the headless posture edits it.
                        // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
                        "--settings".into(),
                        "{}".into(),
                    ])
                })?;
            println!(
                "  {}: would create worktree on `{}` and run vendor argv (settings/OS wrapper resolved at launch): {} {}",
                vendor,
                branch,
                command,
                actual_args.iter()
                    .take(actual_args.len().saturating_sub(1))
                    .cloned().collect::<Vec<_>>().join(" ")
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

    // The fixed bake-off includes Codex, which has no contained adapter.
    // trace:BUG-2172 | ai:codex
    preflight_compete_containment(
        &compete::BAKEOFF_VENDORS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        None,
    )?;
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
        // The arm's mail is retired first; a refusal keeps its worktree and
        // branch. trace:STORY-1843 | ai:claude
        if dir.exists() {
            if let Err(e) =
                crate::mail_retention::remove_worktree(&project_root, &dir, true, "compete cleanup")
            {
                println!("  kept `{}`: {e:#}", r.branch);
                continue;
            }
        }
        let flag = if r.branch == winner_branch {
            "-d"
        } else {
            "-D"
        };
        // trace:BUG-2211 | ai:claude
        let _ = aida_core::git_ops::output_healing_config_lock(
            &project_root,
            || {
                let mut cmd = std::process::Command::new("git");
                cmd.current_dir(&project_root);
                cmd
            },
            &["branch", flag, &r.branch],
        );
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
    let aida = crate::aida_exe_path();
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
        .output_retrying_etxtbsy()
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
    let mut judge_cmd =
        build_compete_judge_command(project_root, &cmd_bin, &cmd_args, judge_vendor)?;
    let o = judge_cmd
        .output_retrying_etxtbsy()
        .context("failed to spawn the judge")?;
    let mut s = String::from_utf8_lossy(&o.stdout).to_string();
    s.push_str(&String::from_utf8_lossy(&o.stderr));
    Ok(s)
}

// trace:BUG-2124 | ai:antigravity
pub(crate) fn build_compete_judge_command(
    project_root: &std::path::Path,
    cmd_bin: &str,
    cmd_args: &[String],
    judge_vendor: compete::JudgeVendor,
) -> Result<std::process::Command> {
    let headless_vendor = match judge_vendor {
        compete::JudgeVendor::Claude => session::HeadlessVendor::Claude,
        compete::JudgeVendor::Codex => session::HeadlessVendor::Codex,
    };
    let mut judge_cmd =
        build_compete_arm_command(project_root, cmd_bin, cmd_args, headless_vendor.as_str())?;
    if judge_vendor.needs_stdin_closed() {
        judge_cmd.stdin(std::process::Stdio::null());
    }
    Ok(judge_cmd)
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

    // Fresh start: drop any stale worktree/branch from a prior compete run,
    // retiring its mail first. trace:STORY-1843 | ai:claude
    if worktree_dir.exists() {
        crate::mail_retention::remove_worktree(project_root, &worktree_dir, true, "compete reuse")
            .with_context(|| format!("clearing the stale compete worktree {worktree_str}"))?;
    }
    let _ = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(project_root)
        .output();
    // trace:BUG-2211 | ai:claude
    let git_in_root = || {
        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(project_root);
        cmd
    };
    let _ = aida_core::git_ops::output_healing_config_lock(
        project_root,
        git_in_root,
        &["branch", "-D", branch],
    );

    let add = aida_core::git_ops::output_healing_config_lock(
        project_root,
        git_in_root,
        &["worktree", "add", "-b", branch, &worktree_str, "HEAD"],
    )
    .context("failed to run `git worktree add`")?;
    if !add.status.success() {
        anyhow::bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        );
    }
    aida_core::git_ops::warn_worktree_container_gitdir(project_root, &worktree_dir);
    aida_core::git_ops::init_submodules_or_warn(
        &worktree_dir,
        crate::worktree_config_init_submodules(project_root),
    )
    .with_context(|| {
        format!(
            "prepare submodules in compete worktree {}",
            worktree_dir.display()
        )
    })?;

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
    let mut arm_cmd = build_compete_arm_command(&worktree_dir, command, argv, vendor)?;
    let vendor_out = arm_cmd.output_retrying_etxtbsy();
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
    // BUG-1985: the vendor ran contained in this tree, so host staging first
    // refuses a planted embedded repository; host git staging into it would
    // run the repository's own filter drivers.
    // trace:BUG-1985 | ai:claude
    let staging_refused = aida_core::git_ops::refuse_untrusted_staging(&worktree_dir).err();
    if let Some(err) = &staging_refused {
        eprintln!("     not committing the vendor's leftover changes: {err:#}");
    } else {
        let _ = aida_core::git_ops::host_git_command(&worktree_dir)
            .args(["add", "-A"])
            .output();
    }
    let has_staged = staging_refused.is_none()
        && !aida_core::git_ops::host_git_command(&worktree_dir)
            .args(["diff", "--cached", "--quiet"])
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

/// Validate all children together, before any run artifacts or destructive cleanup.
// trace:BUG-2172 | ai:codex
fn preflight_compete_containment(
    vendors: &[String],
    judge: Option<compete::JudgeVendor>,
) -> Result<()> {
    if !session::aida_contained_env()? {
        return Ok(());
    }
    for name in vendors
        .iter()
        .map(String::as_str)
        .chain(judge.map(|v| match v {
            compete::JudgeVendor::Claude => "claude",
            compete::JudgeVendor::Codex => "codex",
        }))
    {
        let vendor = session::HeadlessVendor::parse(name).ok_or_else(|| {
            anyhow::anyhow!("[contained] unsupported compete vendor `{name}` — refusing the run before launch artifacts")
        })?;
        session::require_vendor_containment(vendor, true, "compete child")?;
    }
    Ok(())
}

// trace:BUG-2124 | ai:antigravity
pub(crate) fn build_compete_arm_command(
    worktree_dir: &std::path::Path,
    command: &str,
    argv: &[String],
    vendor: &str,
) -> Result<std::process::Command> {
    // trace:BUG-2172 | ai:codex
    let contained = session::aida_contained_env()?;
    let headless_vendor = session::HeadlessVendor::parse(vendor);
    let args = compete_arm_argv(argv, vendor, contained, || {
        session::claude_contained_flags_for(worktree_dir)
    })?;
    let (program, args) = if contained {
        session::os_wrapped_program_and_args(worktree_dir, command, args)?
    } else {
        (command.to_string(), args)
    };
    // trace:BUG-2218 TASK-2328 | ai:codex
    if contained {
        session::prepare_contained_mailbox_for_launch(worktree_dir)?;
    }
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).current_dir(worktree_dir);
    if headless_vendor.is_some_and(|v| session::containment_applied(v, contained)) {
        cmd.env("AIDA_CONTAINED", "1");
    } else {
        cmd.env_remove("AIDA_CONTAINED");
    }
    Ok(cmd)
}

/// Shared posture composition; only launch supplies an operational settings builder.
// trace:BUG-2172 | ai:codex
fn compete_arm_argv(
    argv: &[String],
    vendor: &str,
    contained: bool,
    contained_flags: impl FnOnce() -> Result<Vec<String>>,
) -> Result<Vec<String>> {
    // BUG-050: a claude arm or judge is a headless `claude -p`: its flags lead
    // with the headless posture (no Monitor, no background tasks), whose
    // tool list the template's `-p` ends.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    let claude = vendor == "claude";
    if !contained {
        let mut args = if claude {
            session::headless_claude_posture(Vec::new())?
        } else {
            Vec::new()
        };
        args.extend_from_slice(argv);
        return Ok(args);
    }
    let parsed = session::HeadlessVendor::parse(vendor)
        .ok_or_else(|| anyhow::anyhow!("[contained] unknown compete vendor `{vendor}`"))?;
    session::require_vendor_containment(parsed, true, "compete child")?;
    // Generated argv ends with the prompt. Never parse flags inside it.
    let split = argv.len().saturating_sub(1);
    let (template, _) = crate::agent_launch::split_posture_flags(vendor, argv[..split].to_vec());
    let flags = contained_flags()?;
    let (mut args, trailing) = if claude {
        (session::headless_claude_posture(flags)?, Vec::new())
    } else {
        (Vec::new(), flags)
    };
    args.extend(template);
    args.extend(["--permission-mode".to_string(), "dontAsk".to_string()]);
    args.extend(trailing);
    args.extend_from_slice(&argv[split..]);
    Ok(args)
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
    if let Some(home) = crate::home_dir() {
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
        .output_retrying_etxtbsy()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// trace:BUG-2172 | ai:codex
#[cfg(test)]
mod containment_tests {
    use super::*;

    #[test]
    fn compete_children_inherit_containment_and_replace_bypass_argv() {
        let dir = tempfile::tempdir().unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_CONTAINED", Some("1")),
            ("AIDA_OS_WRAP", Some("0")),
        ]);
        let prompt = "preserve --permission-mode in prompt";
        let (bin, argv) = compete::judge_command(compete::JudgeVendor::Claude, None, prompt);
        let judge =
            build_compete_judge_command(dir.path(), &bin, &argv, compete::JudgeVendor::Claude)
                .unwrap();
        let arm = build_compete_arm_command(dir.path(), &bin, &argv, "claude").unwrap();
        let exec = session::build_exec_vendor_headless_command(
            session::HeadlessVendor::Claude,
            prompt,
            "sid",
            false,
            None,
            None,
            "implementer",
            "",
        )
        .unwrap();
        for cmd in [judge, arm, exec] {
            let args: Vec<_> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().to_string())
                .collect();
            assert_eq!(args.last().map(String::as_str), Some(prompt));
            // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
            crate::session::assert_headless_posture(&args, true, "contained compete child");
            assert!(!args.iter().any(|a| a == "bypassPermissions"));
            assert!(args
                .windows(2)
                .any(|a| a == ["--permission-mode", "dontAsk"]));
            assert!(cmd
                .get_envs()
                .any(|(k, v)| k == "AIDA_CONTAINED" && v == Some(std::ffi::OsStr::new("1"))));
        }
        assert!(build_compete_arm_command(dir.path(), "codex", &[], "codex").is_err());
        assert!(
            build_compete_judge_command(dir.path(), "codex", &[], compete::JudgeVendor::Codex)
                .is_err()
        );
        assert!(build_compete_arm_command(dir.path(), "unknown", &[], "unknown").is_err());
    }

    /// BUG-050: an uncontained claude arm or judge carries the headless
    /// posture too; a codex arm's argv is untouched.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    #[test]
    fn uncontained_claude_compete_children_carry_the_headless_posture() {
        let (_, argv) = compete::judge_command(compete::JudgeVendor::Claude, None, "PROMPT");
        let args = compete_arm_argv(&argv, "claude", false, || unreachable!()).unwrap();
        crate::session::assert_headless_posture(&args, false, "uncontained compete");
        assert!(args
            .windows(2)
            .any(|a| a == ["--permission-mode", "bypassPermissions"]));
        assert_eq!(args.last().map(String::as_str), Some("PROMPT"));
        let codex = vec!["exec".to_string(), "PROMPT".to_string()];
        assert_eq!(
            compete_arm_argv(&codex, "codex", false, || unreachable!()).unwrap(),
            codex
        );
    }

    // trace:BUG-2172 | ai:codex
    #[test]
    fn compete_dry_run_vendor_argv_is_contained_and_does_not_prepare_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".aida")).unwrap();
        let policy = dir.path().join(".aida/contained-managed-settings.json");
        std::fs::write(&policy, "preserve policy").unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_CONTAINED", Some("1")),
            ("AIDA_OS_WRAP", Some("1")),
        ]);
        let prompt = "--permission-mode bypassPermissions inside prompt";
        let (bin, argv) = compete::judge_command(compete::JudgeVendor::Claude, None, prompt);
        assert_eq!(bin, "claude");
        let preview = compete_arm_argv(&argv, "claude", true, || {
            Ok(vec![
                "--setting-sources".into(),
                "project".into(),
                "--settings".into(),
                "{}".into(),
            ])
        })
        .unwrap();
        assert!(preview
            .windows(2)
            .any(|a| a == ["--permission-mode", "dontAsk"]));
        assert!(!preview.iter().any(|a| a == "bypassPermissions"));
        assert_eq!(preview.last().unwrap(), prompt);
        assert_eq!(std::fs::read_to_string(policy).unwrap(), "preserve policy");
        assert_eq!(
            std::fs::read_dir(dir.path().join(".aida")).unwrap().count(),
            1
        );
    }

    // trace:BUG-2172 | ai:codex
    #[test]
    fn contained_compete_refuses_all_unsupported_children_before_side_effects() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let out = crate::test_git::git_command()
                .arg("-C")
                .arg(&root)
                .args(args)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
            out.stdout
        };
        git(&["init", "-q"]);
        git(&["commit", "--allow-empty", "-qm", "initial"]);
        let branch = compete::vendor_branch("TASK-1", "codex");
        let child = compete_worktree_dir(&root, &branch);
        git(&["worktree", "add", "-b", &branch, child.to_str().unwrap()]);
        let sentinel = child.join("uncommitted-work");
        std::fs::write(&sentinel, "preserve me").unwrap();
        let before_refs = git(&["show-ref"]);
        let before_worktrees = git(&["worktree", "list", "--porcelain"]);
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            &root,
            "advisor",
            &[],
            &[("AIDA_CONTAINED", Some("1"))],
        );
        for (vendors, judge) in [
            (vec!["claude".to_string(), "codex".to_string()], false),
            (vec!["claude".to_string()], true),
            (vec!["antigravity".to_string()], false),
            (vec!["unknown".to_string()], false),
        ] {
            for dry_run in [false, true] {
                let err = handle_compete_command("TASK-1", &vendors, None, dry_run, judge, "codex")
                    .unwrap_err();
                assert!(err.to_string().contains("[contained]"), "{err:#}");
                assert_eq!(git(&["show-ref"]), before_refs);
                assert_eq!(git(&["worktree", "list", "--porcelain"]), before_worktrees);
                assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "preserve me");
                assert!(!root.join(".aida").join("agent-briefs").exists());
            }
        }
        assert!(handle_zen_compete("TASK-1", false)
            .unwrap_err()
            .to_string()
            .contains("[contained]"));
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "preserve me");
        assert!(preflight_compete_containment(
            &["claude".into()],
            Some(compete::JudgeVendor::Claude)
        )
        .is_ok());
    }

    #[test]
    fn malformed_containment_refuses_even_an_explicit_contained_request() {
        let dir = tempfile::tempdir().unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("typo"))]);
        assert!(session::compose_headless_command(
            session::HeadlessVendor::Claude,
            "prompt",
            "sid",
            true,
            None,
            None
        )
        .is_err());
        assert!(build_compete_arm_command(dir.path(), "claude", &[], "claude").is_err());
        assert!(build_compete_judge_command(
            dir.path(),
            "claude",
            &[],
            compete::JudgeVendor::Claude
        )
        .is_err());
        assert!(session::interactive_reviewer_launch_plan(
            session::HeadlessVendor::Claude,
            None,
            None,
            "prompt",
            "sid",
            false,
        )
        .is_err());
        assert!(session::spawn_claude_resume_with_grant("sid", None, true, None).is_err());
        assert!(
            handle_compete_command("TASK-1", &["claude".into()], None, false, false, "claude")
                .is_err()
        );
        let log = dir.path().join("must-not-exist.log");
        assert!(session::spawn_vendor_headless_with_seat_granted(
            session::HeadlessVendor::Claude,
            aida_core::agents_config::AgentSeat::Implementer,
            "prompt",
            "sid",
            &log,
            &crate::headless_tee::TeeOptions {
                enabled: false,
                label: None
            },
            true,
        )
        .is_err());
        assert!(session::spawn_claude_headless_resume_granted(
            "prompt",
            "sid",
            &log,
            dir.path(),
            &crate::headless_tee::TeeOptions {
                enabled: false,
                label: None
            },
            true,
            "implementer",
        )
        .is_err());
        assert!(!log.exists());
    }
}

/// Run one spec through N vendors headless, in isolated worktrees, then an
/// objective gate — the neutral cross-vendor quality layer. For each vendor
/// AIDA creates a per-vendor worktree+branch, assembles the implementer
/// brief from the spec, runs the vendor headless (claude/codex), commits the
/// result, and runs a deterministic gate that mirrors PR CI by default
/// (fmt-check + build + tests + clippy + glyph-lint, so a gate-passing
/// winner is actually mergeable). Reports a
/// table, ranks the gate-passers (smaller/focused diff first), optionally
/// runs a rubric LLM judge (--judge), and leaves every branch in place to
/// pick. Report-only: it recommends a winner but never merges.
// trace:STORY-659 trace:STORY-660 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "compete")]
pub(crate) struct CompeteArgs {
    /// SPEC-ID (or UUID) to run through the vendors.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude
    #[clap(value_parser = crate::spec_arg::spec_id)]
    pub spec: String,

    /// Comma-separated vendors to run, e.g. `claude,codex`. Headless
    /// vendors run directly; a non-headless vendor (antigravity) is emitted
    /// as a human-run brief instead.
    #[clap(long, value_delimiter = ',')]
    pub vendors: Vec<String>,

    /// Override the deterministic gate command run in each worktree.
    /// Default mirrors PR CI: fmt-check + build + the aida-cli test suite +
    /// clippy correctness + glyph-lint, so a gate-passing winner is
    /// mergeable. Pass your own command for non-Rust repos / custom CI.
    #[clap(long)]
    pub gate: Option<String>,

    /// Assemble the briefs + plan the run and print what WOULD happen
    /// without spawning any vendor or touching git.
    #[clap(long)]
    pub dry_run: bool,

    /// Add a rubric LLM judge after the gate: spawn a `claude -p` judge over
    /// the spec + each candidate's diff, scoring spec-adherence / correctness
    /// / simplicity / test-coverage (1-5) and recommending a winner. Opt-in
    /// and REPORT-ONLY — it never merges; the human/advisor still picks. The
    /// cheap deterministic ranking (smaller, focused diff first) is always
    /// shown regardless of this flag.
    // trace:STORY-660 | ai:claude
    #[clap(long)]
    pub judge: bool,

    /// Which vendor renders the rubric judgment (claude or codex). The judge
    /// PROMPT is identical for both — only the executing model changes — so a
    /// codex judge over a claude-vs-codex run is no longer the same model
    /// grading itself, removing the self-evaluation caveat. Defaults to
    /// claude (unchanged behaviour). Set AIDA_COMPETE_JUDGE to override the
    /// judge binary. Only applies with --judge.
    // trace:TASK-869 | ai:claude
    #[clap(long = "judge-vendor", default_value = "claude")]
    pub judge_vendor: String,
}

// `aida compete` self-loads the store (to assemble the brief) and
// orchestrates worktrees + headless vendor runs + the objective gate.
// Dispatch early alongside ultraplan — it doesn't need the shared storage
// handle. trace:STORY-659 | ai:claude
fn run_compete(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <CompeteArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let CompeteArgs {
        spec,
        vendors,
        gate,
        dry_run,
        judge,
        judge_vendor,
    } = &args;
    return self::handle_compete_command(
        spec,
        vendors,
        gate.as_deref(),
        *dry_run,
        *judge,
        judge_vendor,
    );
}

// trace:STORY-1728 | ai:claude
pub(crate) static COMPETE: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "compete",
        help_after: "brief",
        command: <CompeteArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_compete),
    };
