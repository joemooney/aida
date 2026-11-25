// trace:TASK-3862 | ai:codex
// trace:STORY-3076 | ai:antigravity
//! CLI handler for `aida review prepare` — deterministic review preparation,
//! path-scoped playbook grouping, semantic file batching, file-coverage verification,
//! and snippet-anchored line positioning.

use crate::{detect_forge_from_origin, extract_acceptance_section, find_project_root, Storage};
use aida_core::review_engine::{
    anchor_findings_with, apply_adversarial_challenger_with_dismissed, bundle_review_batches,
    classify_diff_inventory, compute_addressed_findings_with, compute_severity_weighted_score,
    detect_ai_authors, estimate_review_cost_usd, extract_confirmed_findings_from_sarif,
    findings_exceeding_threshold, findings_to_sarif_json_with_telemetry,
    format_forge_review_markdown_with_adversarial, format_review_telemetry_report_markdown,
    parse_runner_findings_json, resolve_ab_arm_assignment, summarize_review_telemetry_by_arm,
    verify_coverage_ledger, AdversarialReviewSummary, ClassifiedInventory, CoverageLedger,
    CoverageVerificationReport, DismissedReviewFinding, FileChangeKind, PlaybookRegistry,
    ProjectReviewRulesConfig, RawDiffEntry, RawReviewFinding, ResolvedPlaybook, ReviewBatch,
    ReviewTelemetryRecord, DEFAULT_BATCH_TOKEN_BUDGET, DEFAULT_MAX_FILES_PER_BATCH,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// Clap arguments for `aida review prepare`.
// trace:STORY-3076 | ai:antigravity
#[derive(clap::Args, Debug, Clone, Default)]
pub struct ReviewPrepareArgs {
    /// Comma-separated list of AIDA spec IDs whose `## Acceptance` criteria
    /// should be injected into the review preparation bundle.
    #[clap(long, value_name = "SPEC-IDS")]
    #[clap(value_parser = crate::spec_arg::spec_id_csv)]
    pub specs: Option<String>,

    /// Pull spec IDs and base/head refs from PR/MR number N.
    #[clap(long, value_name = "N", conflicts_with_all = ["commit", "from", "to"])]
    pub pr: Option<u64>,

    /// Base Git revision for range comparison (e.g. `main`).
    #[clap(long, value_name = "REF")]
    pub from: Option<String>,

    /// Target Git revision for range comparison (defaults to `HEAD` when `--from` is set).
    #[clap(long, value_name = "REF")]
    #[clap(requires = "from")]
    pub to: Option<String>,

    /// Single commit SHA to inspect against its parent.
    #[clap(long, short = 'c', value_name = "SHA", conflicts_with = "from")]
    pub commit: Option<String>,

    /// Full-file scan mode (`aida review prepare --scan [--path <DIR_OR_FILE>...]`).
    /// Inventories tracked files under the specified paths (or the entire repo if
    /// `--path` is omitted) instead of computing a Git diff.
    // trace:STORY-3076 | ai:antigravity
    #[clap(
        long,
        conflicts_with_all = ["from", "to", "commit", "pr"]
    )]
    pub scan: bool,

    /// Target file or directory path(s) to scope `--scan` (or filter diff entries).
    /// Repeatable (`--path aida-core/src --path aida-cli-lib/src`).
    #[clap(long, value_name = "PATH")]
    pub path: Vec<String>,

    /// Additional comma-separated glob patterns to exclude from review.
    #[clap(long, value_name = "GLOBS", value_delimiter = ',')]
    pub exclude: Vec<String>,

    /// Verify a reviewer coverage ledger JSON file against the classified inventory.
    /// Exits non-zero if any reviewable file is unaccounted for or skipped without a reason.
    #[clap(long, value_name = "LEDGER_JSON")]
    pub verify_coverage: Option<PathBuf>,

    /// Deterministically anchor quoted `existing_code` snippets in a findings JSON
    /// file to exact 1-based `(start_line, end_line)` spans on the target revision.
    #[clap(long, value_name = "FINDINGS_JSON")]
    pub anchor_findings: Option<PathBuf>,

    /// Execute automated batch code review across the prepared batches using
    /// the resolved project agent (`claude`, `codex`, `antigravity`/`agy`, or `gemini`)
    /// or `AIDA_REVIEW_RUNNER_CMD`, automatically anchoring reported `existing_code`
    /// snippets to exact 1-based line numbers and verifying 100% file coverage.
    // trace:STORY-3120 | ai:antigravity
    #[clap(long)]
    pub run_review: bool,

    /// Agent CLI to use for `--run-review` (`claude`, `codex`, `antigravity`/`agy`, or `gemini`).
    /// When omitted, resolves from `.aida/review-rules.toml` (`default_agent`),
    /// `[agents] vendor` (`.aida/agents.toml` / `.aida/config.toml` / `~/.aida/agents.toml`),
    /// or the project's `aida init` `[agents] enabled` list (e.g. `codex` on a Codex-only repo).
    // trace:STORY-3120 | ai:antigravity
    #[clap(long, value_name = "AGENT")]
    pub agent: Option<String>,

    /// Model to pass to the review runner CLI (e.g. `o3`, `gpt-5.3-codex`, `claude-opus-4-6`).
    /// Overrides `.aida/review-rules.toml` (`default_model`) and `[agents.<vendor>.seats.reviewer] model`.
    // trace:STORY-3120 | ai:antigravity
    #[clap(long, value_name = "MODEL")]
    pub model: Option<String>,

    /// Reasoning effort level to pass to the review runner CLI (`low`, `medium`, `high`, or `xhigh`).
    /// Overrides `.aida/review-rules.toml` (`default_effort`) and `[agents.<vendor>.seats.reviewer] effort`.
    // trace:STORY-3120 | ai:antigravity
    #[clap(long, value_name = "LEVEL")]
    pub effort: Option<String>,

    /// Run a two-pass Red-Team Finder -> Blue-Team Skeptical Challenger adversarial review.
    /// Pass 1 hunts for defects; Pass 2 attempts to disprove each candidate finding against
    /// surrounding code and type constraints, dropping false positives.
    // trace:STORY-3120 | ai:antigravity
    #[clap(long)]
    pub adversarial: bool,

    /// Optional agent CLI for Pass 2 of `--adversarial` (`claude`, `codex`, `antigravity`/`agy`, or `gemini`).
    /// When omitted in `--adversarial` mode, automatically selects an opposing enabled vendor from
    /// `.aida/config.toml` if multiple agents are enabled, or falls back to the Finder agent.
    // trace:STORY-3120 | ai:antigravity
    #[clap(long, value_name = "AGENT")]
    pub challenger_agent: Option<String>,

    /// Optional model override for Pass 2 of `--adversarial` (enables same-vendor, cross-model
    /// adversarial verification — e.g. Finder on default model, Challenger on `o3`).
    // trace:STORY-3120 | ai:antigravity
    #[clap(long, value_name = "MODEL")]
    pub challenger_model: Option<String>,

    /// Optional reasoning effort override (`low`, `medium`, `high`, or `xhigh`) for Pass 2 of
    /// `--adversarial` (enables same-model asymmetric-effort adversarial review — e.g. Finder at
    /// `medium`, Challenger at `high`).
    // trace:STORY-3120 | ai:antigravity
    #[clap(long, value_name = "LEVEL")]
    pub challenger_effort: Option<String>,

    /// Fail with a non-zero exit code if any confirmed finding has severity at or above
    /// this threshold (`critical`, `high`, `medium`, or `low`). Also configurable via
    /// `fail_on` in `.aida/review-rules.toml`.
    // trace:STORY-3120 | ai:antigravity
    #[clap(long, value_name = "SEVERITY")]
    pub fail_on: Option<String>,

    /// Automatically file confirmed findings (at or above `--fail-on` severity, or `high`
    /// by default) as draft `BUG` requirements in the AIDA store, pre-populated with the
    /// anchored `file:line` location, quoted snippet, suggested fix, and linked spec tags.
    // trace:STORY-3120 | ai:antigravity
    #[clap(long)]
    pub file_bugs: bool,

    /// Export anchored findings as a SARIF 2.1.0 JSON file inside the repository
    /// (for GitHub Code Scanning / GitLab SAST).
    // trace:STORY-3120 | ai:antigravity
    #[clap(long, value_name = "SARIF_PATH")]
    pub sarif: Option<PathBuf>,

    /// Persist review telemetry (duration breakdown, token/cost estimates, A/B arm,
    /// false-positive suppression rate, anchor precision) to `.aida/review-telemetry/reviews.jsonl`
    /// and archive the full SARIF 2.1.0 findings report (including Pass-2 suppressed candidates)
    /// under `.aida/review-telemetry/sarif/`.
    // trace:STORY-3120 | ai:antigravity
    #[clap(long)]
    pub record_telemetry: bool,

    /// Tag this review execution with an A/B experiment arm label (`single-pass`, `adversarial`,
    /// or `auto` to deterministically randomize 50/50 by PR number or commit SHA).
    // trace:STORY-3120 | ai:antigravity
    #[clap(long, value_name = "ARM")]
    pub ab_arm: Option<String>,

    /// Aggregate `.aida/review-telemetry/reviews.jsonl` and print an A/B telemetry comparison
    /// report (duration, token/cost burn, Pass-1 vs. Pass-2 false-positive suppression rate,
    /// exact line-anchor precision %, and SARIF finding categories).
    // trace:STORY-3120 | ai:antigravity
    #[clap(long)]
    pub telemetry_report: bool,

    /// Post a consolidated Markdown review report as a comment on `--pr <N>`
    /// (supports GitHub `gh pr comment` and GitLab `glab mr note`).
    // trace:STORY-3120 | ai:antigravity
    #[clap(long, requires = "pr")]
    pub post_comment: bool,

    /// Emit machine-readable JSON output.
    #[clap(long)]
    pub json: bool,

    /// Write output inside the repository (relative paths start at the project root).
    #[clap(long, value_name = "PATH")]
    pub write: Option<PathBuf>,
}

/// Linked AIDA spec summary + extracted acceptance criteria included in the bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedSpecAcceptance {
    pub spec_id: String,
    pub title: String,
    pub status: String,
    pub acceptance_markdown: String,
}

/// Complete output of `aida review prepare`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewPreparationBundle {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_base: Option<String>,
    pub inventory: ClassifiedInventory,
    pub batches: Vec<ReviewBatch>,
    pub playbooks: Vec<ResolvedPlaybook>,
    pub linked_specs: Vec<LinkedSpecAcceptance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage_verification: Option<CoverageVerificationReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchored_findings: Option<Vec<RawReviewFinding>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adversarial_summary: Option<AdversarialReviewSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detected_author_agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filed_bug_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<ReviewTelemetryRecord>,
}

// trace:TASK-3862 | ai:codex
fn load_review_rules(project_root: &Path) -> Result<ProjectReviewRulesConfig> {
    ProjectReviewRulesConfig::load_from_root(project_root).context("load .aida/review-rules.toml")
}

/// Execute `aida review prepare`.
// trace:STORY-3076 | ai:antigravity
pub(crate) fn handle_review_prepare(args: &ReviewPrepareArgs, storage: &Storage) -> Result<()> {
    let total_start = Instant::now();
    let project_root = find_project_root()?;

    if args.telemetry_report {
        return handle_telemetry_report(&project_root, args);
    }

    let prepare_start = Instant::now();
    let rules_cfg = load_review_rules(&project_root)?;
    let registry = PlaybookRegistry::new(rules_cfg.rules.clone());

    let mut excludes = rules_cfg.exclude.clone();
    excludes.extend(args.exclude.iter().cloned());

    // 1. Resolve comparison mode and git endpoints.
    let (mode, base_ref, head_ref, merge_base) = resolve_comparison_endpoints(&project_root, args)?;

    // 2. Collect raw diff or full-file scan entries from git.
    let mut raw_entries = if mode == "scan" {
        collect_git_scan_entries(&project_root, &args.path, &excludes)?
    } else {
        collect_git_diff_entries(
            &project_root,
            &mode,
            merge_base.as_deref(),
            head_ref.as_deref(),
            &excludes,
        )?
    };
    if mode != "scan" && !args.path.is_empty() {
        raw_entries.retain(|e| path_matches_any_scope(&e.path, &args.path));
    }

    // 3. Classify inventory and cache target revision file contents for trace + facet detection.
    let token_budget = rules_cfg
        .batch_token_budget
        .unwrap_or(DEFAULT_BATCH_TOKEN_BUDGET);
    let target_rev = head_ref.as_deref();
    let mut content_cache: BTreeMap<String, String> = BTreeMap::new();
    let mut read_error = None;
    let detect_trace =
        |rel_path: &str| match read_file_at_revision(&project_root, target_rev, rel_path) {
            Ok(Some(content)) => {
                let has_trace = content.contains("trace:");
                content_cache.insert(rel_path.to_owned(), content);
                has_trace
            }
            Err(err) => {
                read_error = Some(err);
                false
            }
            Ok(None) => false,
        };
    let inventory = classify_diff_inventory(
        &raw_entries,
        &registry,
        &excludes,
        token_budget,
        detect_trace,
    );
    if let Some(err) = read_error {
        return Err(err);
    }

    // 4. Bundle reviewable files into semantic, token-bounded batches.
    let max_files = rules_cfg
        .max_files_per_batch
        .unwrap_or(DEFAULT_MAX_FILES_PER_BATCH);
    let batches = bundle_review_batches(&inventory.reviewable_files, token_budget, max_files)?;

    // 5. Collect distinct resolved playbooks referenced by reviewable files and
    //    enrich each with diff/content-triggered facets (ANSSI / Tokio / Rust API).
    let mut playbooks_by_id: BTreeMap<String, (ResolvedPlaybook, Vec<&str>)> = BTreeMap::new();
    for rf in &inventory.reviewable_files {
        let entry = playbooks_by_id
            .entry(rf.playbook_id.clone())
            .or_insert_with(|| (registry.resolve(&rf.path), Vec::new()));
        if let Some(body) = content_cache.get(&rf.path) {
            entry.1.push(body.as_str());
        }
    }
    let playbooks: Vec<ResolvedPlaybook> = playbooks_by_id
        .into_values()
        .map(|(pb, samples)| registry.enrich_with_content_facets(pb, &samples))
        .collect();

    // 6. Resolve linked AIDA requirements and extract their `## Acceptance` criteria.
    let linked_specs = resolve_linked_specs(
        &project_root,
        storage,
        args,
        merge_base.as_deref(),
        head_ref.as_deref(),
    )?;

    // 6b. Detect AI authorship (`[AI:tool]` in commit messages and `trace:... | ai:tool` in files)
    //     for automatic cross-vendor author-opposition when multiple agents are enabled.
    let detected_author_agents = detect_changed_ai_authors(
        &project_root,
        args,
        merge_base.as_deref(),
        head_ref.as_deref(),
        &content_cache,
    );
    let prepare_elapsed_ms = prepare_start.elapsed().as_millis() as u64;

    // 7. Optional: execute automated batch review (`--run-review`), or load external
    //    coverage ledger (`--verify-coverage`) and findings (`--anchor-findings`).
    let requested_ab_arm = args
        .ab_arm
        .clone()
        .or_else(|| std::env::var("AIDA_REVIEW_AB_ARM").ok())
        .or_else(|| rules_cfg.ab_arm.clone());
    let explicit_adversarial = args.adversarial || rules_cfg.adversarial.unwrap_or(false);
    let (resolved_ab_arm, is_adversarial) = resolve_ab_arm_assignment(
        requested_ab_arm.as_deref(),
        args.pr,
        head_ref.as_deref(),
        &inventory.inventory_sha256,
        args.run_review,
        explicit_adversarial,
    );

    let mut runner_findings: Option<Vec<RawReviewFinding>> = None;
    let mut runner_ledger: Option<CoverageLedger> = None;
    let mut adversarial_summary: Option<AdversarialReviewSummary> = None;
    let mut runner_metrics = RunnerExecutionMetrics::default();
    let mut finder_agent_used: Option<String> = None;
    let mut finder_model_used: Option<String> = None;
    let mut finder_effort_used: Option<String> = None;

    if args.run_review {
        let resolved_agent = resolve_review_runner_agent_with_authors(
            &project_root,
            args,
            &rules_cfg,
            &detected_author_agents,
        )?;
        let resolved_model =
            resolve_review_runner_model(&project_root, &resolved_agent, args, &rules_cfg);
        let resolved_effort =
            resolve_review_runner_effort(&project_root, &resolved_agent, args, &rules_cfg);
        finder_agent_used = Some(resolved_agent.clone());
        finder_model_used = resolved_model.clone();
        finder_effort_used = resolved_effort.clone();

        let challenger_target = if is_adversarial {
            Some(resolve_challenger_target(
                &project_root,
                &resolved_agent,
                resolved_model.as_deref(),
                resolved_effort.as_deref(),
                args,
                &rules_cfg,
            )?)
        } else {
            None
        };
        let (f, l, mut adv, metrics) = execute_automated_batch_reviews(
            &project_root,
            &resolved_agent,
            resolved_model.as_deref(),
            resolved_effort.as_deref(),
            challenger_target.as_ref(),
            &mode,
            merge_base.as_deref(),
            head_ref.as_deref(),
            &batches,
            &playbooks,
            &linked_specs,
            &content_cache,
        )?;
        if let Some(ref mut summary) = adv {
            summary.detected_author_agents = detected_author_agents.clone();
            let raw_dismissed: Vec<RawReviewFinding> = summary
                .dismissed_details
                .iter()
                .map(|d| d.finding.clone())
                .collect();
            let anchored_dismissed =
                anchor_findings_with(&raw_dismissed, |path| content_cache.get(path).cloned());
            for (d, anchored) in summary
                .dismissed_details
                .iter_mut()
                .zip(anchored_dismissed.into_iter())
            {
                d.finding = anchored;
            }
        }
        runner_findings = Some(f);
        runner_ledger = Some(l);
        adversarial_summary = adv;
        runner_metrics = metrics;
    }

    let verify_start = Instant::now();
    let coverage_verification = if let Some(ledger_path) = &args.verify_coverage {
        let raw_ledger = std::fs::read_to_string(ledger_path)
            .with_context(|| format!("read coverage ledger {}", ledger_path.display()))?;
        let ledger: CoverageLedger = serde_json::from_str(&raw_ledger)
            .with_context(|| format!("parse coverage ledger JSON {}", ledger_path.display()))?;
        Some(verify_coverage_ledger(&inventory, &ledger))
    } else if let Some(ledger) = &runner_ledger {
        Some(verify_coverage_ledger(&inventory, ledger))
    } else {
        None
    };

    // 8. Optional: anchor findings' `existing_code` snippets to exact 1-based line numbers.
    let anchored_findings = if let Some(findings_path) = &args.anchor_findings {
        let raw_findings = std::fs::read_to_string(findings_path)
            .with_context(|| format!("read findings JSON {}", findings_path.display()))?;
        let parsed: Vec<RawReviewFinding> = serde_json::from_str(&raw_findings)
            .with_context(|| format!("parse findings JSON {}", findings_path.display()))?;
        Some(anchor_findings_with(&parsed, |path| {
            content_cache.get(path).cloned()
        }))
    } else if let Some(parsed) = &runner_findings {
        Some(anchor_findings_with(parsed, |path| {
            content_cache.get(path).cloned()
        }))
    } else {
        None
    };
    let verify_elapsed_ms = verify_start.elapsed().as_millis() as u64;

    let effective_fail_on = args
        .fail_on
        .as_deref()
        .or(rules_cfg.fail_on.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let gate_failed_coverage = coverage_verification
        .as_ref()
        .map(|c| !c.passed)
        .unwrap_or(false);
    let gate_failed_severity = if let (Some(threshold), Some(findings)) =
        (effective_fail_on, anchored_findings.as_deref())
    {
        findings_exceeding_threshold(findings, threshold)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    } else {
        false
    };

    // 8b. Build the comprehensive ReviewTelemetryRecord for this execution.
    let mut telemetry_record = build_review_telemetry_record(
        &resolved_ab_arm,
        &mode,
        args.pr,
        base_ref.as_deref(),
        head_ref.as_deref(),
        &inventory,
        &batches,
        &playbooks,
        &linked_specs,
        &detected_author_agents,
        finder_agent_used.as_deref(),
        finder_model_used.as_deref(),
        finder_effort_used.as_deref(),
        adversarial_summary.as_ref(),
        &runner_metrics,
        prepare_elapsed_ms,
        verify_elapsed_ms,
        total_start.elapsed().as_millis() as u64,
        anchored_findings.as_deref().unwrap_or(&[]),
        coverage_verification.as_ref(),
        gate_failed_coverage || gate_failed_severity,
    );

    let dismissed_slice: &[DismissedReviewFinding] = adversarial_summary
        .as_ref()
        .map(|s| s.dismissed_details.as_slice())
        .unwrap_or(&[]);

    // 9. Optional: write SARIF 2.1.0 report (`--sarif <PATH>`) and/or persist to
    //    `.aida/review-telemetry/` (`--record-telemetry` / `record_telemetry = true`).
    let should_record_telemetry = is_review_telemetry_recording_enabled(args, &rules_cfg);
    if should_record_telemetry {
        let archive_rel = persist_review_telemetry_and_sarif(
            &project_root,
            anchored_findings.as_deref().unwrap_or(&[]),
            dismissed_slice,
            &mut telemetry_record,
        )?;
        telemetry_record.sarif_archive_path = Some(archive_rel);
    }

    if let Some(sarif_path) = &args.sarif {
        let findings_slice = anchored_findings.as_deref().unwrap_or(&[]);
        let sarif_doc = findings_to_sarif_json_with_telemetry(
            findings_slice,
            dismissed_slice,
            Some(&telemetry_record),
        );
        let sarif_str = serde_json::to_string_pretty(&sarif_doc)?;
        write_output(&project_root, sarif_path, &sarif_str)?;
    }

    // 10. Optional: post consolidated Markdown review report to GitHub PR / GitLab MR (`--post-comment`).
    if args.post_comment {
        let pr_n = args.pr.context("--post-comment requires --pr <N>")?;
        let spec_pairs: Vec<(String, String)> = linked_specs
            .iter()
            .map(|s| {
                (
                    aida_core::node_alias::display_spec_id(&s.spec_id).to_string(),
                    s.title.clone(),
                )
            })
            .collect();
        let comment_md = format_forge_review_markdown_with_adversarial(
            &mode,
            head_ref.as_deref(),
            &inventory,
            coverage_verification.as_ref(),
            &spec_pairs,
            anchored_findings.as_deref().unwrap_or(&[]),
            adversarial_summary.as_ref(),
        );
        post_forge_review_comment(&project_root, pr_n, &comment_md)?;
    }

    // 11. Optional: automatically file confirmed findings into the AIDA store as draft BUG requirements (`--file-bugs`).
    let filed_bug_ids = if args.file_bugs {
        let bug_min_sev = effective_fail_on.unwrap_or("high");
        file_bugs_for_confirmed_findings(
            storage,
            anchored_findings.as_deref().unwrap_or(&[]),
            bug_min_sev,
            &linked_specs,
        )?
    } else {
        Vec::new()
    };

    let bundle = ReviewPreparationBundle {
        mode,
        base_ref,
        head_ref,
        merge_base,
        inventory,
        batches,
        playbooks,
        linked_specs,
        coverage_verification,
        anchored_findings,
        adversarial_summary,
        detected_author_agents,
        filed_bug_ids,
        telemetry: Some(telemetry_record),
    };

    let rendered = if args.json {
        serde_json::to_string_pretty(&bundle)?
    } else {
        render_bundle_markdown(&bundle)
    };

    if let Some(write_path) = &args.write {
        write_output(&project_root, write_path, &rendered)?;
    } else {
        println!("{rendered}");
    }

    if let Some(cov) = &bundle.coverage_verification {
        if !cov.passed {
            anyhow::bail!(
                "file coverage verification failed ({}% accounted; missing: {:?}, ambiguous: {:?}, skipped without reason: {:?})",
                cov.coverage_rate_pct,
                cov.unaccounted_files,
                cov.ambiguous_files,
                cov.skipped_without_reason
            );
        }
    }

    if let Some(threshold) = effective_fail_on {
        if let Some(findings) = &bundle.anchored_findings {
            let failing = findings_exceeding_threshold(findings, threshold)
                .map_err(|e| anyhow::anyhow!(e))?;
            if !failing.is_empty() {
                anyhow::bail!(
                    "review quality gate failed (--fail-on {threshold}): {} confirmed finding(s) at or above `{threshold}` severity",
                    failing.len()
                );
            }
        }
    }

    Ok(())
}

// trace:TASK-3862 | ai:codex
fn write_output(root: &Path, path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Component;

    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .context("--write outside repository")?
    } else {
        path
    };
    let name = relative
        .file_name()
        .context("--write requires a file name")?;
    anyhow::ensure!(
        relative
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "--write escapes repository"
    );
    anyhow::ensure!(
        !path.components().any(|c| c.as_os_str() == ".git"),
        "--write refuses Git metadata"
    );
    let destination = root.canonicalize()?.join(relative);
    for flag in ["--absolute-git-dir", "--git-common-dir"] {
        let output = git_text(root, &["rev-parse", "--path-format=absolute", flag])?;
        let metadata = Path::new(output.strip_suffix('\n').context("invalid git directory")?)
            .canonicalize()?;
        anyhow::ensure!(
            !destination.starts_with(metadata),
            "--write refuses Git metadata"
        );
    }
    let mut dir = std::fs::File::open(root)?;
    let pinned =
        |file: &std::fs::File| PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    for component in relative.parent().into_iter().flat_map(Path::components) {
        let Component::Normal(part) = component else {
            continue;
        };
        let child = pinned(&dir).join(part);
        match std::fs::create_dir(&child) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e.into()),
            _ => {}
        }
        dir = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(child)?;
    }
    let dest = pinned(&dir).join(name);
    if let Ok(meta) = dest.symlink_metadata() {
        anyhow::ensure!(meta.is_file(), "--write destination must be a regular file");
    }
    let mut tmp = tempfile::NamedTempFile::new_in(pinned(&dir))?;
    tmp.write_all(text.as_bytes())?;
    tmp.persist(dest)?;
    Ok(())
}

fn resolve_comparison_endpoints(
    project_root: &Path,
    args: &ReviewPrepareArgs,
) -> Result<(String, Option<String>, Option<String>, Option<String>)> {
    if args.scan {
        for p in &args.path {
            crate::git_arg_guard::reject_option_like("--path", p)?;
        }
        let head_sha = git_rev_parse(project_root, "HEAD").ok();
        return Ok(("scan".to_string(), head_sha, None, None));
    }

    if let Some(commit) = &args.commit {
        crate::git_arg_guard::reject_option_like("--commit", commit)?;
        let head_sha = git_rev_parse(project_root, &format!("{commit}^{{commit}}"))?;
        // Raw headers retain parents hidden by shallow revision traversal.
        let raw = git_text(
            project_root,
            &["show", "-s", "--format=raw", &head_sha, "--"],
        )?;
        let parent = raw
            .lines()
            .take_while(|line| !line.is_empty())
            .find_map(|line| line.strip_prefix("parent "));
        let parent_sha = match parent {
            Some(parent) => git_rev_parse(project_root, &format!("{parent}^{{commit}}"))
                .context("commit parent unavailable")?,
            None => git_text(project_root, &["hash-object", "-t", "tree", "/dev/null"])?
                .trim()
                .to_owned(),
        };
        return Ok((
            "commit".to_string(),
            Some(parent_sha.clone()),
            Some(head_sha),
            Some(parent_sha),
        ));
    }

    if let Some(from) = &args.from {
        crate::git_arg_guard::reject_option_like("--from", from)?;
        let to = args.to.as_deref().unwrap_or("HEAD");
        crate::git_arg_guard::reject_option_like("--to", to)?;
        let head_sha = git_rev_parse(project_root, to)?;
        let mb = git_merge_base(project_root, from, &head_sha)?;
        return Ok((
            "range".to_string(),
            Some(from.clone()),
            Some(head_sha),
            Some(mb),
        ));
    }

    if let Some(pr_n) = args.pr {
        let forge = detect_forge_from_origin(project_root).context("cannot resolve PR forge")?;
        let reader: Box<dyn crate::forge::Forge> = match forge.forge_kind() {
            crate::forge::ForgeKind::GitHub => {
                Box::new(crate::forge::GitHubForge::for_review(project_root)?)
            }
            kind => crate::forge::forge_for_kind(project_root, kind),
        };
        let meta = reader.change_metadata(pr_n, &mut crate::network_retry::NoopSink)?;
        let (head, mb) = pr_comparison(project_root, &meta.base_ref, &meta.head_sha)?;
        return Ok(("range".into(), Some(meta.base_ref), Some(head), Some(mb)));
    }

    let head_sha = git_rev_parse(project_root, "HEAD").ok();
    Ok(("workspace".to_string(), head_sha, None, None))
}

// trace:TASK-3862 | ai:codex
fn pr_comparison(root: &Path, base: &str, head: &str) -> Result<(String, String)> {
    anyhow::ensure!(
        crate::forge::is_full_commit_sha(head),
        "invalid forge head SHA"
    );
    let base_ref = format!("refs/remotes/origin/{base}");
    git_text(root, &["check-ref-format", &base_ref])?;
    let resolved = git_rev_parse(root, &format!("{head}^{{commit}}"))?;
    anyhow::ensure!(
        resolved.eq_ignore_ascii_case(head),
        "forge head SHA mismatch"
    );
    let mb = git_merge_base(root, &base_ref, &resolved)?;
    Ok((resolved, mb))
}

// trace:TASK-3862 | ai:codex
fn git_rev_parse(root: &Path, rev: &str) -> Result<String> {
    let value = git_text(root, &["rev-parse", "--verify", "--end-of-options", rev])?;
    Ok(value.trim_end().to_owned())
}

fn git_merge_base(root: &Path, base: &str, head: &str) -> Result<String> {
    let value = git_text(root, &["merge-base", "--end-of-options", base, head])?;
    Ok(value.trim_end().to_owned())
}

// trace:STORY-3076 | ai:antigravity
fn collect_git_diff_entries(
    project_root: &Path,
    mode: &str,
    merge_base: Option<&str>,
    head_ref: Option<&str>,
    excludes: &[String],
) -> Result<Vec<RawDiffEntry>> {
    let range = match (mode, merge_base, head_ref) {
        ("commit" | "range", Some(base), Some(head)) => format!("{base}..{head}"),
        _ => "HEAD".into(),
    };
    let diff = |flag: &str, renames: &str| {
        git_text(
            project_root,
            &["diff", "--no-ext-diff", renames, "-z", flag, &range, "--"],
        )
    };
    let mut stats = BTreeMap::new();
    for row in diff("--numstat", "--no-renames")?.split_terminator('\0') {
        let mut fields = row.splitn(3, '\t');
        let added = fields.next().context("missing additions")?;
        let removed = fields.next().context("missing deletions")?;
        let path = fields.next().context("missing numstat path")?;
        let binary = added == "-" && removed == "-";
        let counts = if binary {
            (0, 0, true)
        } else {
            (added.parse::<usize>()?, removed.parse::<usize>()?, false)
        };
        stats.insert(path.to_owned(), counts);
    }
    let status = diff("--name-status", "--find-renames")?;
    let mut fields = status.split_terminator('\0');
    let mut entries = Vec::new();
    while let Some(code) = fields.next() {
        let first = fields.next().context("missing status path")?;
        let (kind, path, old_path) = match code.as_bytes().first() {
            Some(b'R') => (
                FileChangeKind::Renamed,
                fields.next().context("missing rename target")?,
                Some(first.to_owned()),
            ),
            Some(b'A') => (FileChangeKind::Added, first, None),
            Some(b'D') => (FileChangeKind::Deleted, first, None),
            _ => (FileChangeKind::Modified, first, None),
        };
        let &(additions, deletions, is_binary) = stats.get(path).context("missing diff stats")?;
        entries.push(RawDiffEntry {
            path: path.into(),
            old_path,
            kind,
            additions,
            deletions,
            is_binary,
            patch_bytes: additions.saturating_add(deletions).saturating_mul(48),
        });
    }
    if mode == "workspace" {
        let paths = git_text(
            project_root,
            &["ls-files", "-z", "--others", "--exclude-standard"],
        )?;
        entries.extend(worktree_entries(
            project_root,
            &paths,
            FileChangeKind::Untracked,
            excludes,
        )?);
    }
    Ok(entries)
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .current_dir(root)
        .args(args)
        .output()
        .with_context(|| format!("run git {}", args[0]))?;
    anyhow::ensure!(
        out.status.success(),
        "git {} failed: {}",
        args[0],
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8(out.stdout)?)
}

fn path_matches_any_scope(candidate: &str, scopes: &[String]) -> bool {
    let norm_cand = candidate.replace('\\', "/");
    let norm_cand = norm_cand.trim_matches('/');
    for s in scopes {
        let norm_s = s.replace('\\', "/");
        let norm_s = norm_s.trim_matches('/');
        if norm_s.is_empty() || norm_cand == norm_s || norm_cand.starts_with(&format!("{norm_s}/"))
        {
            return true;
        }
    }
    false
}

// trace:STORY-3076 | ai:antigravity
fn collect_git_scan_entries(
    project_root: &Path,
    paths: &[String],
    excludes: &[String],
) -> Result<Vec<RawDiffEntry>> {
    for p in paths {
        crate::git_arg_guard::reject_option_like("--path", p)?;
        if Path::new(p).components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        }) {
            anyhow::bail!("invalid --path `{p}`: must be a relative path inside the repository");
        }
    }

    let mut args = vec![
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    let paths = git_text(project_root, &args)?;
    worktree_entries(project_root, &paths, FileChangeKind::Scanned, excludes)
}

fn worktree_entries(
    root: &Path,
    paths: &str,
    kind: FileChangeKind,
    excludes: &[String],
) -> Result<Vec<RawDiffEntry>> {
    let mut entries = Vec::new();
    for path in paths.split_terminator('\0').collect::<BTreeSet<_>>() {
        let full = root.join(path);
        if !full.is_file() {
            continue;
        }
        let mut entry = RawDiffEntry {
            path: path.into(),
            old_path: None,
            kind,
            additions: 0,
            deletions: 0,
            is_binary: false,
            patch_bytes: 0,
        };
        if aida_core::review_engine::classify_exclusion(&entry, excludes).is_none() {
            let Some(bytes) = read_workspace_file(root, path)? else {
                continue;
            };
            entry.is_binary = bytes.iter().take(8000).any(|&b| b == 0);
            if !entry.is_binary {
                entry.additions = bytes.split_inclusive(|&b| b == b'\n').count();
            }
            entry.patch_bytes = bytes.len();
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn read_file_at_revision(root: &Path, rev: Option<&str>, path: &str) -> Result<Option<String>> {
    if let Some(rev) = rev {
        let spec = format!("{rev}:{path}");
        return git_text(root, &["show", "--end-of-options", &spec, "--"]).map(Some);
    }
    Ok(read_workspace_file(root, path)?.and_then(|bytes| String::from_utf8(bytes).ok()))
}

fn read_workspace_file(root: &Path, path: &str) -> Result<Option<Vec<u8>>> {
    let full = root.join(path);
    // Skip symlinks, including intermediate directory aliases to excluded paths.
    if full.canonicalize()? != full || !full.is_file() {
        return Ok(None);
    }
    Ok(Some(std::fs::read(full)?))
}

fn resolve_linked_specs(
    project_root: &Path,
    storage: &Storage,
    args: &ReviewPrepareArgs,
    merge_base: Option<&str>,
    head_ref: Option<&str>,
) -> Result<Vec<LinkedSpecAcceptance>> {
    let mut spec_ids: Vec<String> = Vec::new();
    if let Some(csv) = &args.specs {
        spec_ids.extend(
            csv.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    } else if let (Some(mb), Some(head)) = (merge_base, head_ref) {
        let messages = if args.commit.is_some() {
            vec![git_text(
                project_root,
                &["show", "-s", "--format=%B", head, "--"],
            )?]
        } else {
            let range = format!("{mb}..{head}");
            git_text(
                project_root,
                &["log", "-z", "--format=%B", "--end-of-options", &range, "--"],
            )?
            .split_terminator('\0')
            .map(str::to_owned)
            .collect()
        };
        let cov = crate::review_gates::review_coverage_from_commits(&messages);
        spec_ids.extend(cov.delivered);
        spec_ids.extend(cov.slice_refs);
    }

    let Ok(store) = storage.load() else {
        return Ok(Vec::new());
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for id in spec_ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(req) = store.requirements.iter().find(|r| {
            r.spec_id
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(&id))
                || r.agreed_id
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&id))
        }) {
            let acceptance = extract_acceptance_section(&req.description)
                .unwrap_or_else(|| req.description.trim().to_string());
            out.push(LinkedSpecAcceptance {
                spec_id: req.display_id().to_string(), // canonical-id: JSON bundle identity
                title: req.title.clone(),
                status: req.status.to_string(),
                acceptance_markdown: acceptance,
            });
        }
    }
    Ok(out)
}

fn render_bundle_markdown(bundle: &ReviewPreparationBundle) -> String {
    let mut out = String::new();
    out.push_str("# AIDA Deterministic Review Preparation\n\n");
    out.push_str(&format!("- **Mode**: `{}`\n", bundle.mode));
    if let Some(mb) = &bundle.merge_base {
        out.push_str(&format!("- **Merge base**: `{mb}`\n"));
    }
    if let Some(head) = &bundle.head_ref {
        out.push_str(&format!("- **Head**: `{head}`\n"));
    }
    out.push_str(&format!(
        "- **Inventory SHA-256**: `{}`\n",
        bundle.inventory.inventory_sha256
    ));
    out.push_str(&format!(
        "- **Reviewable files**: {} | **Excluded files**: {} | **Batches**: {}\n\n",
        bundle.inventory.reviewable_files.len(),
        bundle.inventory.excluded_files.len(),
        bundle.batches.len()
    ));

    if !bundle.linked_specs.is_empty() {
        out.push_str("## Linked AIDA Requirement Acceptance Criteria\n\n");
        for spec in &bundle.linked_specs {
            out.push_str(&format!(
                "### {} — {} (`{}`)\n\n{}\n\n",
                aida_core::node_alias::display_spec_id(&spec.spec_id),
                spec.title,
                spec.status,
                spec.acceptance_markdown
            ));
        }
    }

    out.push_str("## Reviewable File Inventory (100% Accountability Required)\n\n");
    if bundle.inventory.reviewable_files.is_empty() {
        out.push_str("_No reviewable files in scope._\n\n");
    } else {
        for f in &bundle.inventory.reviewable_files {
            let trace_badge = if f.has_trace_comment {
                "trace:yes"
            } else {
                "trace:no"
            };
            out.push_str(&format!(
                "- `{}` ({}, +{} / -{}, ~{} tok, playbook:`{}`, {})\n",
                f.path,
                f.kind.as_str(),
                f.additions,
                f.deletions,
                f.estimated_tokens,
                f.playbook_id,
                trace_badge
            ));
        }
        out.push('\n');
    }

    if !bundle.batches.is_empty() {
        out.push_str("## Token-Budgeted Semantic Review Batches\n\n");
        for b in &bundle.batches {
            out.push_str(&format!(
                "### Batch {} — `{}` (~{} tokens, playbooks: `{}`)\n",
                b.batch_index,
                b.label,
                b.estimated_tokens,
                b.playbook_ids.join(", ")
            ));
            for path in &b.files {
                out.push_str(&format!("  - `{path}`\n"));
            }
            out.push('\n');
        }
    }

    if !bundle.playbooks.is_empty() {
        out.push_str("## Active Path-Scoped Review Playbooks\n\n");
        for pb in &bundle.playbooks {
            out.push_str(&format!(
                "### Playbook `{}` — {} (`{}`)\n\n{}\n\n",
                pb.id, pb.title, pb.pattern, pb.checklist
            ));
        }
    }

    if !bundle.inventory.excluded_files.is_empty() {
        out.push_str("## Excluded Files\n\n");
        for ex in &bundle.inventory.excluded_files {
            out.push_str(&format!(
                "- `{}` ({}) -> `{}`\n",
                ex.path,
                ex.kind.as_str(),
                ex.reason
            ));
        }
        out.push('\n');
    }

    if let Some(cov) = &bundle.coverage_verification {
        out.push_str("## File Coverage Verification\n\n");
        out.push_str(&format!(
            "- **Passed**: `{}` ({}% of {} reviewable files accounted)\n",
            cov.passed, cov.coverage_rate_pct, cov.total_reviewable
        ));
        for file in &cov.not_reviewable_by_batch {
            out.push_str(&format!(
                "- **Not reviewable by batch**: `{}` ({})\n",
                file.path, file.reason
            ));
        }
        if !cov.ambiguous_files.is_empty() {
            out.push_str(&format!(
                "- **Ambiguous ledger rows**: `{}`\n",
                cov.ambiguous_files.join(", ")
            ));
        }
        if !cov.unaccounted_files.is_empty() {
            out.push_str(&format!(
                "- **Unaccounted files**: `{}`\n",
                cov.unaccounted_files.join(", ")
            ));
        }
        if !cov.skipped_without_reason.is_empty() {
            out.push_str(&format!(
                "- **Skipped without reason**: `{}`\n",
                cov.skipped_without_reason.join(", ")
            ));
        }
        out.push('\n');
    }

    if let Some(adv) = &bundle.adversarial_summary {
        out.push_str("## Adversarial Verification (Red-Team Finder → Blue-Team Challenger)\n\n");
        let finder_label = match &adv.finder_model {
            Some(m) => format!("`{}` (`{m}`)", adv.finder_agent),
            None => format!("`{}`", adv.finder_agent),
        };
        let challenger_label = match &adv.challenger_model {
            Some(m) => format!("`{}` (`{m}`)", adv.challenger_agent),
            None => format!("`{}`", adv.challenger_agent),
        };
        out.push_str(&format!(
            "- **Finder**: {finder_label} → **Challenger**: {challenger_label}\n- **Candidates challenged**: `{}` → **Confirmed**: `{}` | **Dismissed (false positives)**: `{}`\n\n",
            adv.candidate_findings, adv.confirmed_findings, adv.dismissed_findings
        ));
    }

    if !bundle.detected_author_agents.is_empty() {
        out.push_str(&format!(
            "- **Detected AI Author(s)**: `{}`\n\n",
            bundle.detected_author_agents.join("`, `")
        ));
    }

    if !bundle.filed_bug_ids.is_empty() {
        out.push_str("## Auto-Filed AIDA Bug Requirements (`--file-bugs`)\n\n");
        for bug_id in &bundle.filed_bug_ids {
            out.push_str(&format!("- `{bug_id}`\n"));
        }
        out.push('\n');
    }

    if let Some(tel) = &bundle.telemetry {
        out.push_str("## Review Telemetry & Cost Summary\n\n");
        out.push_str(&format!(
            "- **Review ID**: `{}` | **A/B Arm**: `{}`\n",
            tel.review_id, tel.ab_arm
        ));
        out.push_str(&format!(
            "- **Duration**: `{}` ms total (`prepare`: {} ms, `finder`: {} ms, `challenger`: {} ms, `verify`: {} ms)\n",
            tel.total_elapsed_ms,
            tel.prepare_elapsed_ms,
            tel.finder_elapsed_ms,
            tel.challenger_elapsed_ms,
            tel.verify_elapsed_ms
        ));
        out.push_str(&format!(
            "- **Tokens & Cost**: ~`{}` input / ~`{}` output tokens (est. `${:.5}` USD)\n",
            tel.estimated_input_tokens, tel.estimated_output_tokens, tel.estimated_cost_usd
        ));
        out.push_str(&format!(
            "- **Signal & Anchor Precision**: `{}` candidate(s) → `{}` confirmed (`{}` dismissed, `{}%` suppression) | `{}`% exact line-anchor rate\n",
            tel.pass1_candidate_findings,
            tel.confirmed_findings,
            tel.dismissed_findings,
            tel.false_positive_suppression_pct,
            tel.exact_anchor_rate_pct
        ));
        if let Some(archive_path) = &tel.sarif_archive_path {
            out.push_str(&format!("- **SARIF 2.1.0 Archive**: `{archive_path}`\n"));
        }
        out.push('\n');
    }

    out
}

/// Timing and byte-size metrics collected across Pass 1 (Finder) and Pass 2 (Challenger).
#[derive(Debug, Clone, Default)]
struct RunnerExecutionMetrics {
    finder_elapsed_ms: u64,
    challenger_elapsed_ms: u64,
    finder_prompt_bytes: usize,
    finder_response_bytes: usize,
    challenger_prompt_bytes: usize,
    challenger_response_bytes: usize,
}

#[allow(clippy::too_many_arguments)]
fn build_review_telemetry_record(
    ab_arm: &str,
    mode: &str,
    pr_number: Option<u64>,
    base_ref: Option<&str>,
    head_ref: Option<&str>,
    inventory: &ClassifiedInventory,
    batches: &[ReviewBatch],
    playbooks: &[ResolvedPlaybook],
    linked_specs: &[LinkedSpecAcceptance],
    detected_author_agents: &[String],
    finder_agent: Option<&str>,
    finder_model: Option<&str>,
    finder_effort: Option<&str>,
    adversarial: Option<&AdversarialReviewSummary>,
    metrics: &RunnerExecutionMetrics,
    prepare_elapsed_ms: u64,
    verify_elapsed_ms: u64,
    total_elapsed_ms: u64,
    anchored_findings: &[RawReviewFinding],
    coverage: Option<&CoverageVerificationReport>,
    gate_failed: bool,
) -> ReviewTelemetryRecord {
    let now = chrono::Utc::now();
    let short_inv = if inventory.inventory_sha256.len() > 8 {
        &inventory.inventory_sha256[..8]
    } else {
        &inventory.inventory_sha256
    };
    let review_id = format!("rev-{}-{short_inv}", now.format("%Y%m%dT%H%M%SZ"));
    let timestamp = now.to_rfc3339();

    let total_added_lines: usize = inventory.reviewable_files.iter().map(|f| f.additions).sum();
    let total_removed_lines: usize = inventory.reviewable_files.iter().map(|f| f.deletions).sum();
    let active_playbooks: Vec<String> = playbooks.iter().map(|p| p.id.clone()).collect();
    let spec_ids: Vec<String> = linked_specs
        .iter()
        .map(|s| aida_core::node_alias::display_spec_id(&s.spec_id).to_string())
        .collect();

    let finder_in_tok = (metrics.finder_prompt_bytes + 3) / 4;
    let finder_out_tok = (metrics.finder_response_bytes + 3) / 4;
    let chall_in_tok = (metrics.challenger_prompt_bytes + 3) / 4;
    let chall_out_tok = (metrics.challenger_response_bytes + 3) / 4;
    let estimated_input_tokens = finder_in_tok + chall_in_tok;
    let estimated_output_tokens = finder_out_tok + chall_out_tok;

    let finder_cost =
        estimate_review_cost_usd(finder_agent, finder_model, finder_in_tok, finder_out_tok);
    let challenger_cost = if let Some(adv) = adversarial {
        estimate_review_cost_usd(
            Some(adv.challenger_agent.as_str()),
            adv.challenger_model.as_deref(),
            chall_in_tok,
            chall_out_tok,
        )
    } else {
        0.0
    };
    let estimated_cost_usd = ((finder_cost + challenger_cost) * 100_000.0).round() / 100_000.0;

    let confirmed_findings = anchored_findings.len();
    let (pass1_candidate_findings, dismissed_findings) = if let Some(adv) = adversarial {
        (adv.candidate_findings, adv.dismissed_findings)
    } else {
        (confirmed_findings, 0)
    };
    let false_positive_suppression_pct = if pass1_candidate_findings > 0 {
        ((dismissed_findings * 100) / pass1_candidate_findings).min(100) as u8
    } else {
        0
    };

    let mut critical_count = 0usize;
    let mut high_count = 0usize;
    let mut medium_count = 0usize;
    let mut low_count = 0usize;
    let mut counts_by_category: BTreeMap<String, usize> = BTreeMap::new();
    let mut exact_anchor_count = 0usize;
    let mut shifted_anchor_count = 0usize;
    let mut unanchored_count = 0usize;

    for f in anchored_findings {
        match f.severity.trim().to_ascii_lowercase().as_str() {
            "critical" => critical_count += 1,
            "high" => high_count += 1,
            "medium" => medium_count += 1,
            _ => low_count += 1,
        }
        let cat = if f.category.trim().is_empty() {
            "general".to_string()
        } else {
            f.category.trim().to_string()
        };
        *counts_by_category.entry(cat).or_insert(0) += 1;

        match f.anchor_status.as_deref() {
            Some("exact") => exact_anchor_count += 1,
            Some("trimmed_whitespace") => shifted_anchor_count += 1,
            _ => unanchored_count += 1,
        }
    }

    let exact_anchor_rate_pct = if confirmed_findings > 0 {
        (((exact_anchor_count + shifted_anchor_count) * 100) / confirmed_findings).min(100) as u8
    } else {
        100
    };

    ReviewTelemetryRecord {
        review_id,
        timestamp,
        ab_arm: ab_arm.to_string(),
        mode: mode.to_string(),
        pr_number,
        base_ref: base_ref.map(str::to_string),
        head_ref: head_ref.map(str::to_string),
        inventory_sha256: inventory.inventory_sha256.clone(),
        linked_specs: spec_ids,
        detected_author_agents: detected_author_agents.to_vec(),
        finder_agent: finder_agent.map(str::to_string),
        finder_model: finder_model.map(str::to_string),
        finder_effort: finder_effort.map(str::to_string),
        challenger_agent: adversarial.map(|a| a.challenger_agent.clone()),
        challenger_model: adversarial.and_then(|a| a.challenger_model.clone()),
        challenger_effort: adversarial.and_then(|a| a.challenger_effort.clone()),
        reviewable_files: inventory.reviewable_files.len(),
        excluded_files: inventory.excluded_files.len(),
        batch_count: batches.len(),
        total_added_lines,
        total_removed_lines,
        active_playbooks,
        prepare_elapsed_ms,
        finder_elapsed_ms: metrics.finder_elapsed_ms,
        challenger_elapsed_ms: metrics.challenger_elapsed_ms,
        verify_elapsed_ms,
        total_elapsed_ms,
        finder_prompt_bytes: metrics.finder_prompt_bytes,
        finder_response_bytes: metrics.finder_response_bytes,
        challenger_prompt_bytes: metrics.challenger_prompt_bytes,
        challenger_response_bytes: metrics.challenger_response_bytes,
        estimated_input_tokens,
        estimated_output_tokens,
        estimated_cost_usd,
        pass1_candidate_findings,
        confirmed_findings,
        dismissed_findings,
        false_positive_suppression_pct,
        critical_count,
        high_count,
        medium_count,
        low_count,
        counts_by_category,
        exact_anchor_count,
        shifted_anchor_count,
        unanchored_count,
        exact_anchor_rate_pct,
        coverage_rate_pct: coverage.map(|c| c.coverage_rate_pct.min(100) as u8),
        gate_failed,
        sarif_archive_path: None,
        severity_weighted_score: compute_severity_weighted_score(
            critical_count,
            high_count,
            medium_count,
            low_count,
        ),
        addressed_findings: None,
        addressed_rate_pct: None,
    }
}

fn is_review_telemetry_recording_enabled(
    args: &ReviewPrepareArgs,
    rules_cfg: &ProjectReviewRulesConfig,
) -> bool {
    if let Ok(global_kill) = std::env::var("AIDA_TELEMETRY") {
        let v = global_kill.trim().to_ascii_lowercase();
        if v == "0" || v == "false" || v == "off" {
            return false;
        }
    }
    if args.record_telemetry {
        return true;
    }
    if let Ok(env_rec) = std::env::var("AIDA_REVIEW_RECORD_TELEMETRY") {
        let v = env_rec.trim().to_ascii_lowercase();
        if v == "1" || v == "true" || v == "yes" {
            return true;
        }
    }
    rules_cfg.record_telemetry.unwrap_or(false)
}

/// Persist the full SARIF 2.1.0 report (confirmed + Pass-2 suppressed findings + embedded telemetry)
/// to `.aida/review-telemetry/sarif/<review_id>.sarif.json` and append the `ReviewTelemetryRecord`
/// row to `.aida/review-telemetry/reviews.jsonl`.
// trace:STORY-3120 | ai:antigravity
fn persist_review_telemetry_and_sarif(
    project_root: &Path,
    confirmed: &[RawReviewFinding],
    dismissed: &[DismissedReviewFinding],
    telemetry: &mut ReviewTelemetryRecord,
) -> Result<String> {
    let sarif_rel = format!(
        ".aida/review-telemetry/sarif/{}.sarif.json",
        telemetry.review_id
    );
    telemetry.sarif_archive_path = Some(sarif_rel.clone());

    let sarif_doc = findings_to_sarif_json_with_telemetry(confirmed, dismissed, Some(telemetry));
    let sarif_str = serde_json::to_string_pretty(&sarif_doc)?;
    write_output(project_root, Path::new(&sarif_rel), &sarif_str)?;

    let jsonl_rel = Path::new(".aida/review-telemetry/reviews.jsonl");
    let jsonl_full = project_root.join(jsonl_rel);
    let mut existing = std::fs::read_to_string(&jsonl_full).unwrap_or_default();
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    let line = serde_json::to_string(telemetry)?;
    existing.push_str(&line);
    existing.push('\n');
    write_output(project_root, jsonl_rel, &existing)?;

    Ok(sarif_rel)
}

/// Aggregate `.aida/review-telemetry/reviews.jsonl` and print the A/B telemetry comparison report.
// trace:STORY-3120 | ai:antigravity
fn handle_telemetry_report(project_root: &Path, args: &ReviewPrepareArgs) -> Result<()> {
    let jsonl_path = project_root
        .join(".aida")
        .join("review-telemetry")
        .join("reviews.jsonl");
    let mut records: Vec<ReviewTelemetryRecord> = Vec::new();
    if jsonl_path.is_file() {
        let raw = std::fs::read_to_string(&jsonl_path)
            .with_context(|| format!("read {}", jsonl_path.display()))?;
        for (line_no, line) in raw.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut rec: ReviewTelemetryRecord =
                serde_json::from_str(trimmed).with_context(|| {
                    format!(
                        "parse telemetry JSONL row {} in {}",
                        line_no + 1,
                        jsonl_path.display()
                    )
                })?;
            // Live Online Addressed-Rate evaluation (ReviewBench counterpart to online precision):
            // When the SARIF 2.1.0 archive exists on disk, inspect its confirmed findings against
            // the current workspace files to check how many quoted defective snippets were modified
            // or replaced by `suggestion_code` in subsequent edits/commits.
            if let Some(sarif_rel) = rec.sarif_archive_path.as_deref() {
                let sarif_full = project_root.join(sarif_rel);
                if let Ok(sarif_raw) = std::fs::read_to_string(&sarif_full) {
                    if let Ok(sarif_val) = serde_json::from_str::<serde_json::Value>(&sarif_raw) {
                        let confirmed = extract_confirmed_findings_from_sarif(&sarif_val);
                        let (addressed, evaluable, rate) =
                            compute_addressed_findings_with(&confirmed, |rel_path| {
                                std::fs::read_to_string(project_root.join(rel_path)).ok()
                            });
                        if evaluable > 0 {
                            rec.addressed_findings = Some(addressed);
                            rec.addressed_rate_pct = rate;
                        }
                    }
                }
            }
            records.push(rec);
        }
    }

    let rendered = if args.json {
        let summaries = summarize_review_telemetry_by_arm(&records);
        serde_json::to_string_pretty(&serde_json::json!({
            "total_runs": records.len(),
            "arms": summaries,
            "records": records
        }))?
    } else {
        format_review_telemetry_report_markdown(&records)
    };

    if let Some(write_path) = &args.write {
        write_output(project_root, write_path, &rendered)?;
    } else {
        println!("{rendered}");
    }
    Ok(())
}

fn detect_changed_ai_authors(
    project_root: &Path,
    args: &ReviewPrepareArgs,
    merge_base: Option<&str>,
    head_ref: Option<&str>,
    content_cache: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut messages = Vec::new();
    if let (Some(mb), Some(head)) = (merge_base, head_ref) {
        if args.commit.is_some() {
            if let Ok(m) = git_text(project_root, &["show", "-s", "--format=%B", head, "--"]) {
                messages.push(m);
            }
        } else {
            let range = format!("{mb}..{head}");
            if let Ok(raw) = git_text(
                project_root,
                &["log", "-z", "--format=%B", "--end-of-options", &range, "--"],
            ) {
                messages.extend(raw.split_terminator('\0').map(str::to_owned));
            }
        }
    } else if let Ok(m) = git_text(project_root, &["show", "-s", "--format=%B", "HEAD", "--"]) {
        messages.push(m);
    }
    let samples: Vec<&str> = content_cache.values().map(String::as_str).collect();
    detect_ai_authors(&messages, &samples)
}

/// File confirmed findings at or above `min_severity` as draft `BUG` requirements in the AIDA store.
// trace:STORY-3120 | ai:antigravity
fn file_bugs_for_confirmed_findings(
    storage: &Storage,
    findings: &[RawReviewFinding],
    min_severity: &str,
    linked_specs: &[LinkedSpecAcceptance],
) -> Result<Vec<String>> {
    let qualifying =
        findings_exceeding_threshold(findings, min_severity).map_err(|e| anyhow::anyhow!(e))?;
    if qualifying.is_empty() {
        return Ok(Vec::new());
    }

    let mut store = storage.load().unwrap_or_default();
    let mut filed_ids = Vec::with_capacity(qualifying.len());

    for f in qualifying {
        let loc = match (f.start_line, f.end_line) {
            (Some(s), Some(e)) if s != e => format!("{}:{s}-{e}", f.path),
            (Some(s), _) => format!("{}:{s}", f.path),
            _ => f.path.clone(),
        };
        let short_msg: String = f
            .message
            .lines()
            .next()
            .unwrap_or(&f.message)
            .chars()
            .take(72)
            .collect();
        let title = format!(
            "[review:{}] {}: {}",
            f.category.trim(),
            loc,
            short_msg.trim()
        );

        let mut desc = String::new();
        desc.push_str(&format!(
            "## Problem\n\nConfirmed `{}` (`{}`) defect at `{loc}`:\n\n{}\n\n",
            f.severity.trim().to_ascii_uppercase(),
            f.category.trim(),
            f.message.trim()
        ));
        if let Some(snippet) = f.existing_code.as_deref() {
            if !snippet.trim().is_empty() {
                desc.push_str(&format!(
                    "### Existing Code\n\n```text\n{}\n```\n\n",
                    snippet.trim_end()
                ));
            }
        }
        if let Some(sugg) = f.suggestion_code.as_deref() {
            if !sugg.trim().is_empty() {
                desc.push_str(&format!(
                    "## Suggested Fix\n\n```text\n{}\n```\n\n",
                    sugg.trim_end()
                ));
            }
        }
        desc.push_str(&format!(
            "## Acceptance\n\n- [ ] Defect at `{loc}` (`{}`) is resolved and covered by a unit/regression test.\n",
            f.category.trim()
        ));

        let mut req = aida_core::Requirement::new(title, desc);
        req.req_type = aida_core::RequirementType::Bug;
        req.status = aida_core::RequirementStatus::Draft;
        req.owner = crate::get_default_author();
        req.tags.insert("from-review".to_string());
        req.tags.insert(format!(
            "severity:{}",
            f.severity.trim().to_ascii_lowercase()
        ));
        req.tags.insert(format!(
            "category:{}",
            f.category.trim().to_ascii_lowercase()
        ));
        for spec in linked_specs {
            req.tags.insert(format!(
                "spec:{}",
                aida_core::node_alias::display_spec_id(&spec.spec_id)
            ));
        }

        aida_core::provenance::stamp_if_absent(&mut req);
        let id = req.id;
        let type_prefix = store.get_type_prefix(&req.req_type);
        store.add_requirement_with_id(req, None, type_prefix.as_deref());
        let display_id = store
            .get_requirement_by_id(&id)
            .and_then(|r| r.spec_id.clone())
            .unwrap_or_else(|| id.to_string());
        filed_ids.push(display_id);
    }

    storage.save(&store)?;
    Ok(filed_ids)
}

/// Post a consolidated Markdown review report to a GitHub PR or GitLab MR.
// trace:STORY-3120 | ai:antigravity
fn post_forge_review_comment(project_root: &Path, pr_n: u64, comment_md: &str) -> Result<()> {
    let forge = detect_forge_from_origin(project_root).context("cannot resolve PR forge")?;
    let client: Box<dyn crate::forge::Forge> = match forge.forge_kind() {
        crate::forge::ForgeKind::GitHub => {
            Box::new(crate::forge::GitHubForge::for_review(project_root)?)
        }
        kind => crate::forge::forge_for_kind(project_root, kind),
    };
    let change_ref = crate::forge::ChangeRef {
        id: pr_n,
        url: String::new(),
        branch: String::new(),
        base: String::new(),
        title: None,
    };
    client.comment(&change_ref, comment_md)
}

/// Resolve the review runner agent CLI to invoke for `--run-review`.
///
/// Precedence (highest first):
///   1. `--agent <AGENT>` CLI flag
///   2. `AIDA_REVIEW_AGENT` environment variable
///   3. `.aida/review-rules.toml` `default_agent`
///   4. `[agents] vendor` from `.aida/agents.toml` > `.aida/config.toml` > `~/.aida/agents.toml`
///   5. `[agents] enabled = [...]` written by `aida init` (e.g. `["codex"]` on a Codex-only repo)
///   6. `"claude"` fallback
// trace:STORY-3120 | ai:antigravity
fn resolve_review_runner_agent(
    project_root: &Path,
    args: &ReviewPrepareArgs,
    rules_cfg: &ProjectReviewRulesConfig,
) -> Result<String> {
    resolve_review_runner_agent_with_authors(project_root, args, rules_cfg, &[])
}

fn resolve_review_runner_agent_with_authors(
    project_root: &Path,
    args: &ReviewPrepareArgs,
    rules_cfg: &ProjectReviewRulesConfig,
    detected_authors: &[String],
) -> Result<String> {
    if let Some(flag_agent) = &args.agent {
        return normalize_review_agent(flag_agent);
    }
    if let Ok(env_agent) = std::env::var("AIDA_REVIEW_AGENT") {
        let trimmed = env_agent.trim();
        if !trimmed.is_empty() {
            return normalize_review_agent(trimmed);
        }
    }
    if let Some(cfg_agent) = &rules_cfg.default_agent {
        let trimmed = cfg_agent.trim();
        if !trimmed.is_empty() {
            return normalize_review_agent(trimmed);
        }
    }
    if let Some(vendor) = aida_core::agents_config::resolve_default_vendor(project_root) {
        return normalize_review_agent(&vendor);
    }
    if let Some(selection) = crate::init_cmd::read_enabled_agent_selection(project_root) {
        // When both claude and codex are enabled on the project and a single AI author
        // is detected in the commit/trace metadata, automatically select the opposing vendor
        // so an agent does not grade its own code.
        if selection.claude && selection.codex && detected_authors.len() == 1 {
            if detected_authors[0] == "claude" {
                return Ok("codex".to_string());
            }
            if detected_authors[0] == "codex" {
                return Ok("claude".to_string());
            }
        }
        if selection.codex && !selection.claude {
            return Ok("codex".to_string());
        }
        if selection.antigravity && !selection.claude && !selection.codex {
            return Ok("agy".to_string());
        }
        if selection.claude {
            return Ok("claude".to_string());
        }
    }
    Ok("claude".to_string())
}

fn normalize_review_agent(raw: &str) -> Result<String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "claude" => Ok("claude".to_string()),
        "codex" => Ok("codex".to_string()),
        "antigravity" | "agy" => Ok("agy".to_string()),
        "gemini" => Ok("gemini".to_string()),
        other => anyhow::bail!(
            "unsupported review agent `{other}` (expected `claude`, `codex`, `antigravity`/`agy`, or `gemini`)"
        ),
    }
}

/// Resolve the model to pass to the primary review runner.
///
/// Precedence (highest first):
///   1. `--model <MODEL>` CLI flag
///   2. `AIDA_REVIEW_MODEL` environment variable
///   3. `.aida/review-rules.toml` `default_model`
///   4. `[agents.<agent>.seats.reviewer] model` > `[agents.<agent>] model`
// trace:STORY-3120 | ai:antigravity
fn resolve_review_runner_model(
    project_root: &Path,
    agent: &str,
    args: &ReviewPrepareArgs,
    rules_cfg: &ProjectReviewRulesConfig,
) -> Option<String> {
    if let Some(m) = &args.model {
        let trimmed = m.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Ok(m) = std::env::var("AIDA_REVIEW_MODEL") {
        let trimmed = m.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(m) = &rules_cfg.default_model {
        let trimmed = m.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    aida_core::agents_config::resolve_agent_tuning(
        project_root,
        agent,
        aida_core::agents_config::AgentSeat::Reviewer,
    )
    .model
}

/// Resolve the reasoning effort level (`low`, `medium`, `high`, `xhigh`) for the primary review runner.
///
/// Precedence (highest first):
///   1. `--effort <LEVEL>` CLI flag
///   2. `AIDA_REVIEW_EFFORT` environment variable
///   3. `.aida/review-rules.toml` `default_effort`
///   4. `[agents.<agent>.seats.reviewer] effort`
// trace:STORY-3120 | ai:antigravity
fn resolve_review_runner_effort(
    project_root: &Path,
    agent: &str,
    args: &ReviewPrepareArgs,
    rules_cfg: &ProjectReviewRulesConfig,
) -> Option<String> {
    if let Some(e) = &args.effort {
        let trimmed = e.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Ok(e) = std::env::var("AIDA_REVIEW_EFFORT") {
        let trimmed = e.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(e) = &rules_cfg.default_effort {
        let trimmed = e.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    aida_core::agents_config::resolve_agent_tuning(
        project_root,
        agent,
        aida_core::agents_config::AgentSeat::Reviewer,
    )
    .effort
}

/// Resolved `(agent, model, effort)` triple for Pass 2 of `--adversarial`.
type ChallengerTarget = (String, Option<String>, Option<String>);

/// Resolve `(challenger_agent, challenger_model, challenger_effort)` for Pass 2 of `--adversarial`.
///
/// When `--challenger-agent` / `rules_cfg.challenger_agent` is omitted, automatically
/// selects an opposing enabled vendor from `.aida/config.toml` if both `claude` and
/// `codex` are enabled, or falls back to `finder_agent` (for single-vendor projects
/// such as Codex-only environments).
// trace:STORY-3120 | ai:antigravity
fn resolve_challenger_target(
    project_root: &Path,
    finder_agent: &str,
    finder_model: Option<&str>,
    finder_effort: Option<&str>,
    args: &ReviewPrepareArgs,
    rules_cfg: &ProjectReviewRulesConfig,
) -> Result<ChallengerTarget> {
    let challenger_agent = if let Some(flag_ca) = &args.challenger_agent {
        normalize_review_agent(flag_ca)?
    } else if let Ok(env_ca) = std::env::var("AIDA_REVIEW_CHALLENGER_AGENT") {
        let trimmed = env_ca.trim();
        if !trimmed.is_empty() {
            normalize_review_agent(trimmed)?
        } else {
            auto_opposing_agent(project_root, finder_agent)
        }
    } else if let Some(cfg_ca) = &rules_cfg.challenger_agent {
        let trimmed = cfg_ca.trim();
        if !trimmed.is_empty() {
            normalize_review_agent(trimmed)?
        } else {
            auto_opposing_agent(project_root, finder_agent)
        }
    } else {
        auto_opposing_agent(project_root, finder_agent)
    };

    let challenger_tuning = aida_core::agents_config::resolve_agent_tuning(
        project_root,
        &challenger_agent,
        aida_core::agents_config::AgentSeat::Reviewer,
    );

    let challenger_model = if let Some(m) = &args.challenger_model {
        let trimmed = m.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    } else if let Ok(m) = std::env::var("AIDA_REVIEW_CHALLENGER_MODEL") {
        let trimmed = m.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    } else if let Some(m) = &rules_cfg.challenger_model {
        let trimmed = m.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    } else if challenger_agent != finder_agent {
        challenger_tuning.model
    } else {
        finder_model.map(str::to_string)
    };

    let challenger_effort = if let Some(e) = &args.challenger_effort {
        let trimmed = e.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    } else if let Ok(e) = std::env::var("AIDA_REVIEW_CHALLENGER_EFFORT") {
        let trimmed = e.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    } else if let Some(e) = &rules_cfg.challenger_effort {
        let trimmed = e.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    } else if challenger_agent != finder_agent {
        challenger_tuning.effort
    } else {
        finder_effort.map(str::to_string)
    };

    Ok((challenger_agent, challenger_model, challenger_effort))
}

fn auto_opposing_agent(project_root: &Path, finder_agent: &str) -> String {
    if let Some(selection) = crate::init_cmd::read_enabled_agent_selection(project_root) {
        if finder_agent == "claude" && selection.codex {
            return "codex".to_string();
        }
        if finder_agent == "codex" && selection.claude {
            return "claude".to_string();
        }
    }
    finder_agent.to_string()
}

/// Execute automated batch code reviews over each prepared batch using either
/// `AIDA_REVIEW_RUNNER_CMD` (invoked with prompt on stdin) or the resolved agent CLI.
/// When `challenger` is `Some((challenger_agent, challenger_model))`, runs a second
/// Blue-Team Skeptical Challenger pass to disprove false-positive candidate findings.
// trace:STORY-3120 | ai:antigravity
fn execute_automated_batch_reviews(
    project_root: &Path,
    agent: &str,
    model: Option<&str>,
    effort: Option<&str>,
    challenger: Option<&ChallengerTarget>,
    mode: &str,
    merge_base: Option<&str>,
    head_ref: Option<&str>,
    batches: &[ReviewBatch],
    playbooks: &[ResolvedPlaybook],
    linked_specs: &[LinkedSpecAcceptance],
    content_cache: &BTreeMap<String, String>,
) -> Result<(
    Vec<RawReviewFinding>,
    CoverageLedger,
    Option<AdversarialReviewSummary>,
    RunnerExecutionMetrics,
)> {
    let playbooks_map: BTreeMap<&str, &ResolvedPlaybook> =
        playbooks.iter().map(|p| (p.id.as_str(), p)).collect();

    let mut all_findings = Vec::new();
    let mut all_coverage = Vec::new();
    let mut all_dismissed: Vec<DismissedReviewFinding> = Vec::new();
    let mut total_candidates = 0usize;
    let mut metrics = RunnerExecutionMetrics::default();

    for batch in batches {
        let prompt = build_batch_review_prompt(
            project_root,
            mode,
            merge_base,
            head_ref,
            batch,
            &playbooks_map,
            linked_specs,
            content_cache,
        )?;
        metrics.finder_prompt_bytes += prompt.len();
        let finder_start = Instant::now();
        let raw_out = invoke_review_runner(project_root, agent, model, effort, "finder", &prompt)?;
        metrics.finder_elapsed_ms += finder_start.elapsed().as_millis() as u64;
        metrics.finder_response_bytes += raw_out.len();

        let (mut batch_findings, batch_ledger) = parse_runner_findings_json(&raw_out)
            .map_err(|e| anyhow::anyhow!("batch {} runner output error: {e}", batch.batch_index))?;

        if let Some((challenger_agent, challenger_model, challenger_effort)) = challenger {
            total_candidates += batch_findings.len();
            if !batch_findings.is_empty() {
                let challenger_prompt = build_adversarial_challenger_prompt(
                    batch,
                    &batch_findings,
                    linked_specs,
                    content_cache,
                );
                metrics.challenger_prompt_bytes += challenger_prompt.len();
                let chall_start = Instant::now();
                let raw_challenger = invoke_review_runner(
                    project_root,
                    challenger_agent,
                    challenger_model.as_deref(),
                    challenger_effort.as_deref(),
                    "challenger",
                    &challenger_prompt,
                )?;
                metrics.challenger_elapsed_ms += chall_start.elapsed().as_millis() as u64;
                metrics.challenger_response_bytes += raw_challenger.len();

                let (confirmed, dismissed) =
                    apply_adversarial_challenger_with_dismissed(batch_findings, &raw_challenger)
                        .map_err(|e| {
                            anyhow::anyhow!(
                                "batch {} adversarial challenger error: {e}",
                                batch.batch_index
                            )
                        })?;
                batch_findings = confirmed;
                all_dismissed.extend(dismissed);
            }
        }

        all_findings.extend(batch_findings);
        all_coverage.extend(batch_ledger.files);
    }

    let total_dismissed = all_dismissed.len();
    let adv_summary = challenger.map(|(challenger_agent, challenger_model, challenger_effort)| {
        AdversarialReviewSummary {
            finder_agent: agent.to_string(),
            finder_model: model.map(str::to_string),
            finder_effort: effort.map(str::to_string),
            challenger_agent: challenger_agent.clone(),
            challenger_model: challenger_model.clone(),
            challenger_effort: challenger_effort.clone(),
            candidate_findings: total_candidates,
            confirmed_findings: all_findings.len(),
            dismissed_findings: total_dismissed,
            detected_author_agents: Vec::new(),
            dismissed_details: all_dismissed,
        }
    });

    Ok((
        all_findings,
        CoverageLedger {
            files: all_coverage,
        },
        adv_summary,
        metrics,
    ))
}

fn build_adversarial_challenger_prompt(
    batch: &ReviewBatch,
    candidates: &[RawReviewFinding],
    linked_specs: &[LinkedSpecAcceptance],
    content_cache: &BTreeMap<String, String>,
) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are an AIDA Blue-Team Skeptical Challenger in a two-pass adversarial code review.\n",
    );
    prompt.push_str("Your job is to challenge every candidate finding from Pass 1 (Red-Team Finder) against the actual file contents, type guarantees, and surrounding control flow.\n");
    prompt.push_str("Apply the three-condition validity rubric (True + Relevant + Non-Trivial):\n");
    prompt.push_str("1. **Factually True**: The defect is genuinely reachable in the code and not already prevented by surrounding validation, caller invariants, or compiler/type/borrow checks.\n");
    prompt.push_str("2. **Change-Relevant**: The issue is directly introduced, exposed, or exacerbated by the reviewed code (including adding new filesystem/network/exec calls on an unvalidated parameter in a touched function, ordering hazards with sibling calls in the same function, or violating project security/architecture rules in `AGENTS.md`/`CLAUDE.md` or a linked AIDA `## Acceptance` criterion).\n");
    prompt.push_str("3. **Actionable & Non-Trivial**: The finding surfaces a substantive correctness, security, concurrency, error-handling, contract, or test defect — not stylistic bikeshedding or linter noise.\n");
    prompt.push_str("- Mark a finding `\"dismissed\"` if it fails ANY of the three conditions above, and state which condition failed in `reason`.\n");
    prompt.push_str("- Mark a finding `\"confirmed\"` only if it satisfies ALL THREE conditions (`True + Relevant + Non-Trivial`).\n\n");
    prompt.push_str("Return ONLY a valid JSON object matching this exact schema:\n");
    prompt.push_str("{\n  \"verdicts\": [\n    {\n      \"index\": 0,\n      \"verdict\": \"confirmed|dismissed\",\n      \"reason\": \"Concise evidence from the code explaining why the finding holds or fails\",\n      \"refined_message\": \"Optional sharpened message when confirmed\"\n    }\n  ]\n}\n\n");

    if !linked_specs.is_empty() {
        prompt.push_str("## Linked AIDA Requirement Acceptance Criteria\n\n");
        for spec in linked_specs {
            prompt.push_str(&format!(
                "### {} — {}\n{}\n\n",
                aida_core::node_alias::display_spec_id(&spec.spec_id),
                spec.title,
                spec.acceptance_markdown
            ));
        }
    }

    prompt.push_str("## Candidate Findings to Challenge\n\n");
    for (idx, f) in candidates.iter().enumerate() {
        prompt.push_str(&format!(
            "- **Index `{idx}`**: `[{}]` `{}` (`{}`) — {}\n",
            f.severity, f.path, f.category, f.message
        ));
        if let Some(snippet) = &f.existing_code {
            prompt.push_str(&format!(
                "  Quoted snippet:\n  ```text\n  {snippet}\n  ```\n"
            ));
        }
    }
    prompt.push('\n');

    prompt.push_str(&format!(
        "## Batch {} (`{}`) — Full File Context\n\n",
        batch.batch_index, batch.label
    ));
    for path in &batch.files {
        if let Some(body) = content_cache.get(path) {
            prompt.push_str(&format!("### File: `{path}`\n```text\n{body}"));
            if !body.ends_with('\n') {
                prompt.push('\n');
            }
            prompt.push_str("```\n\n");
        }
    }

    prompt
}

fn build_batch_review_prompt(
    project_root: &Path,
    mode: &str,
    merge_base: Option<&str>,
    head_ref: Option<&str>,
    batch: &ReviewBatch,
    playbooks_map: &BTreeMap<&str, &ResolvedPlaybook>,
    linked_specs: &[LinkedSpecAcceptance],
    content_cache: &BTreeMap<String, String>,
) -> Result<String> {
    let mut prompt = String::new();
    prompt.push_str("You are an AIDA deterministic batch code reviewer. Review every file in this batch against the linked AIDA acceptance criteria and active playbooks.\n\n");
    prompt.push_str(
        "Return ONLY a valid JSON object matching this exact schema (no extra commentary):\n",
    );
    prompt.push_str("{\n  \"findings\": [\n    {\n      \"path\": \"relative/path.rs\",\n      \"severity\": \"critical|high|medium|low\",\n      \"category\": \"fail-open|absent-evidence-reads-as-good|incomplete-fix|untested-path|contract-drift|race|perf|portability|security|bug\",\n      \"message\": \"Concise explanation of the defect and its runtime/contract impact\",\n      \"existing_code\": \"Exact verbatim lines from the new file where the defect occurs\",\n      \"suggestion_code\": \"Optional replacement code\"\n    }\n  ],\n  \"files\": [\n    { \"path\": \"relative/path.rs\", \"status\": \"reviewed\" }\n  ]\n}\n\n");

    if !linked_specs.is_empty() {
        prompt.push_str("## Linked AIDA Requirement Acceptance Criteria\n\n");
        for spec in linked_specs {
            prompt.push_str(&format!(
                "### {} — {}\n{}\n\n",
                aida_core::node_alias::display_spec_id(&spec.spec_id),
                spec.title,
                spec.acceptance_markdown
            ));
        }
    }

    prompt.push_str("## Active Review Playbooks for This Batch\n\n");
    for pb_id in &batch.playbook_ids {
        if let Some(pb) = playbooks_map.get(pb_id.as_str()) {
            prompt.push_str(&format!(
                "### {} (`{}`)\n{}\n\n",
                pb.title, pb.id, pb.checklist
            ));
        }
    }

    let range_arg = match (mode, merge_base, head_ref) {
        ("commit" | "range", Some(mb), Some(head)) => Some(format!("{mb}..{head}")),
        _ => None,
    };

    prompt.push_str(&format!(
        "## Batch {} (`{}`) — Files Under Review\n\n",
        batch.batch_index, batch.label
    ));

    for path in &batch.files {
        prompt.push_str(&format!("### File: `{path}`\n\n"));
        if mode == "scan" {
            if let Some(body) = content_cache.get(path) {
                prompt.push_str("```text\n");
                prompt.push_str(body);
                if !body.ends_with('\n') {
                    prompt.push('\n');
                }
                prompt.push_str("```\n\n");
            }
        } else {
            let patch = if let Some(r) = &range_arg {
                git_text(
                    project_root,
                    &[
                        "diff",
                        "--no-ext-diff",
                        "--no-color",
                        "--end-of-options",
                        r,
                        "--",
                        path,
                    ],
                )
                .unwrap_or_default()
            } else {
                git_text(
                    project_root,
                    &["diff", "--no-ext-diff", "--no-color", "HEAD", "--", path],
                )
                .unwrap_or_default()
            };
            if !patch.trim().is_empty() {
                prompt.push_str("```diff\n");
                prompt.push_str(&patch);
                if !patch.ends_with('\n') {
                    prompt.push('\n');
                }
                prompt.push_str("```\n\n");
                if let Some(body) = content_cache.get(path) {
                    if body.len() <= 32_768 {
                        prompt.push_str(&format!(
                            "#### Full Target File Context (`{path}` at target revision)\n```text\n"
                        ));
                        prompt.push_str(body);
                        if !body.ends_with('\n') {
                            prompt.push('\n');
                        }
                        prompt.push_str("```\n\n");
                    }
                }
            } else if let Some(body) = content_cache.get(path) {
                prompt.push_str("```text\n");
                prompt.push_str(body);
                if !body.ends_with('\n') {
                    prompt.push('\n');
                }
                prompt.push_str("```\n\n");
            }
        }
    }

    Ok(prompt)
}

fn invoke_review_runner(
    project_root: &Path,
    agent: &str,
    model_override: Option<&str>,
    effort_override: Option<&str>,
    pass_role: &str,
    prompt: &str,
) -> Result<String> {
    use std::io::Write;
    use std::process::Stdio;

    if let Ok(custom_cmd) = std::env::var("AIDA_REVIEW_RUNNER_CMD") {
        let trimmed = custom_cmd.trim();
        if !trimmed.is_empty() {
            let mut cmd = Command::new(trimmed);
            cmd.current_dir(project_root)
                .env("AIDA_REVIEW_RESOLVED_AGENT", agent)
                .env("AIDA_REVIEW_PASS", pass_role);
            if let Some(m) = model_override {
                cmd.env("AIDA_REVIEW_RESOLVED_MODEL", m);
            }
            if let Some(e) = effort_override {
                cmd.env("AIDA_REVIEW_RESOLVED_EFFORT", e);
            }
            let mut child = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .with_context(|| format!("failed to spawn AIDA_REVIEW_RUNNER_CMD `{trimmed}`"))?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(prompt.as_bytes())?;
            }
            let out = child.wait_with_output()?;
            anyhow::ensure!(
                out.status.success(),
                "AIDA_REVIEW_RUNNER_CMD exited non-zero: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return Ok(String::from_utf8(out.stdout)?);
        }
    }

    let tuning = aida_core::agents_config::resolve_agent_tuning(
        project_root,
        agent,
        aida_core::agents_config::AgentSeat::Reviewer,
    );
    let effective_model = model_override.map(str::to_string).or(tuning.model);
    let effective_effort = effort_override.map(str::to_string).or(tuning.effort);

    let (program, mut argv) = match agent {
        "codex" => {
            let mut args = vec![
                "exec".to_string(),
                "--sandbox".to_string(),
                "read-only".to_string(),
                "--skip-git-repo-check".to_string(),
            ];
            if let Some(model) = effective_model {
                args.push("--model".to_string());
                args.push(model);
            }
            if let Some(effort) = effective_effort {
                args.push("-c".to_string());
                args.push(format!("model_reasoning_effort={effort}"));
            }
            args.push("--".to_string());
            args.push(prompt.to_string());
            ("codex", args)
        }
        "agy" => {
            let mut args = Vec::new();
            if let Some(model) = effective_model {
                args.push("--model".to_string());
                args.push(model);
            }
            if let Some(effort) = effective_effort {
                args.push("--effort".to_string());
                args.push(effort);
            }
            args.push("-p".to_string());
            args.push(prompt.to_string());
            ("agy", args)
        }
        "gemini" => {
            let mut args = Vec::new();
            if let Some(model) = effective_model {
                args.push("--model".to_string());
                args.push(model);
            }
            args.push("-p".to_string());
            args.push(prompt.to_string());
            ("gemini", args)
        }
        _ => {
            let mut args = vec!["--print".to_string()];
            if let Some(model) = effective_model {
                args.push("--model".to_string());
                args.push(model);
            }
            if let Some(effort) = effective_effort {
                args.push("--effort".to_string());
                args.push(effort);
            }
            args.push("-p".to_string());
            args.push(prompt.to_string());
            ("claude", args)
        }
    };

    let out = Command::new(program)
        .current_dir(project_root)
        .args(&mut argv)
        .output()
        .with_context(|| {
            format!(
                "failed to run `{program}` review runner (use `--agent <claude|codex|agy|gemini>`, configure `[agents] vendor` / `aida init`, or set `AIDA_REVIEW_RUNNER_CMD`)"
            )
        })?;
    anyhow::ensure!(
        out.status.success(),
        "`{program}` review runner failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8(out.stdout)?)
}

// trace:TASK-3862 | ai:codex
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_review_rules_errors() {
        let root = tempfile::tempdir().unwrap();
        let config_dir = root.path().join(".aida");
        std::fs::create_dir(&config_dir).unwrap();
        std::fs::write(config_dir.join("review-rules.toml"), "[unterminated").unwrap();
        let _guard = crate::test_env::AmbientGuard::hermetic(root.path(), None);
        let store = Storage::new(root.path().join("requirements.yaml"));
        let error = handle_review_prepare(&ReviewPrepareArgs::default(), &store).unwrap_err();
        assert_eq!(error.to_string(), "load .aida/review-rules.toml");
    }

    #[test]
    fn option_like_endpoints_refuse_before_git() {
        let root = tempfile::tempdir().unwrap();
        for (args, flag) in [
            (
                ReviewPrepareArgs {
                    commit: Some("--help".into()),
                    ..Default::default()
                },
                "--commit",
            ),
            (
                ReviewPrepareArgs {
                    from: Some("-p".into()),
                    ..Default::default()
                },
                "--from",
            ),
            (
                ReviewPrepareArgs {
                    from: Some("HEAD".into()),
                    to: Some("--stat".into()),
                    ..Default::default()
                },
                "--to",
            ),
        ] {
            let error = resolve_comparison_endpoints(root.path(), &args).unwrap_err();
            assert!(error.to_string().contains(flag), "{error}");
        }
    }

    #[test]
    fn write_confines_new_and_existing_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let outside = temp.path().join("outside");
        std::fs::write(&outside, "original").unwrap();
        std::os::unix::fs::symlink(temp.path(), root.join("link")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("leaf")).unwrap();
        git_text(&root, &["init", "-q"]).unwrap();
        std::fs::rename(root.join(".git"), root.join("metadata")).unwrap();
        std::fs::write(root.join(".git"), "gitdir: metadata").unwrap();
        for path in [
            Path::new(".git"),
            Path::new("new/.git/config"),
            Path::new("metadata"),
            Path::new("metadata/config"),
            Path::new("../outside"),
            &outside,
            Path::new("link/outside"),
            Path::new("leaf"),
        ] {
            assert!(write_output(&root, path, "changed").is_err());
        }
        assert!(!root.join("new").exists());
        assert_eq!(
            std::fs::read_to_string(root.join(".git")).unwrap(),
            "gitdir: metadata"
        );
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "original");
        write_output(&root, Path::new("nested/report"), "ok").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("nested/report")).unwrap(),
            "ok"
        );
    }

    #[test]
    fn root_commit_preserves_unusual_git_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git_text(root, &["init", "-q"]).unwrap();
        let path = " tab\tand newline\n.rs";
        std::fs::write(root.join(path), "fn main() {}\n").unwrap();
        git_text(root, &["add", "--", path]).unwrap();
        let original = crate::test_git::commit_fixture(root);
        git_text(root, &["rm", "--", path]).unwrap();
        let replacement = crate::test_git::commit_fixture(root);
        git_text(root, &["replace", &original, &replacement]).unwrap();
        let args = ReviewPrepareArgs {
            commit: Some(original),
            ..Default::default()
        };
        let (mode, _, head, base) = resolve_comparison_endpoints(root, &args).unwrap();
        let entries =
            collect_git_diff_entries(root, &mode, base.as_deref(), head.as_deref(), &[]).unwrap();
        let store = Storage::new(root.join("requirements.yaml"));
        assert!(
            resolve_linked_specs(root, &store, &args, base.as_deref(), head.as_deref()).is_ok()
        );
        assert!(
            resolve_linked_specs(root, &store, &args, base.as_deref(), Some("missing")).is_err()
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, path);
        assert_eq!(
            read_file_at_revision(root, head.as_deref(), path)
                .unwrap()
                .as_deref(),
            Some("fn main() {}\n")
        );
    }

    #[test]
    fn prepare_excludes_credentials_before_revision_reads() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _guard = crate::test_env::AmbientGuard::hermetic(root, None);
        let store = Storage::new(root.join("requirements.yaml"));
        git_text(root, &["init", "-q"]).unwrap();
        std::fs::create_dir(root.join(".aws")).unwrap();
        std::fs::write(root.join(".aws/token"), [0xff]).unwrap();
        git_text(root, &["add", "-f", ".aws/token"]).unwrap();
        let base = crate::test_git::commit_fixture(root);
        git_text(root, &["mv", ".aws/token", "ordinary.rs"]).unwrap();
        std::fs::create_dir(root.join(".ssh")).unwrap();
        std::fs::write(root.join(".ssh/key"), [0xfe]).unwrap();
        git_text(root, &["add", "-f", ".ssh/key"]).unwrap();
        crate::test_git::commit_fixture(root);
        let dest = root.join("bundle.json");
        let args = ReviewPrepareArgs {
            from: Some(base),
            json: true,
            write: Some(dest.clone()),
            ..Default::default()
        };
        handle_review_prepare(&args, &store).unwrap(); // Either content read would fail UTF-8 decoding.
        let bundle: ReviewPreparationBundle =
            serde_json::from_slice(&std::fs::read(dest).unwrap()).unwrap();
        assert!(bundle.inventory.reviewable_files.is_empty());
        let paths: Vec<_> = bundle
            .inventory
            .excluded_files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(paths, [".ssh/key", "ordinary.rs"]);
    }

    #[test]
    fn forge_head_and_shallow_parent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let base = crate::test_git::commit_fixture(root);
        git_text(root, &["update-ref", "refs/remotes/origin/base", &base]).unwrap();
        git_text(root, &["branch", "topic", &base]).unwrap();
        let head = crate::test_git::commit_fixture(root);
        git_text(
            root,
            &[
                "remote",
                "add",
                "origin",
                "https://gitlab.com/fixture/repo.git",
            ],
        )
        .unwrap();
        let glab = root.join("glab");
        let metadata = serde_json::json!({"iid":1,"source_branch":"topic","target_branch":"base","state":"opened","title":"t","sha":head});
        crate::test_exec::write_executable(
            &glab,
            &format!("#!/bin/sh\nprintf '%s\\n' '{metadata}'\n"),
        );
        let mut env = crate::test_env::EnvVarsGuard::set(&[(
            "AIDA_TEST_GLAB_BINARY",
            glab.to_str().unwrap(),
        )]);
        let pr = ReviewPrepareArgs {
            pr: Some(1),
            ..Default::default()
        };
        let (_, _, selected, mb) = resolve_comparison_endpoints(root, &pr).unwrap();
        assert_eq!((selected, mb), (Some(head.clone()), Some(base)));
        assert!(pr_comparison(root, "base", &"f".repeat(40)).is_err());
        git_text(
            root,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/fixture/repo.git",
            ],
        )
        .unwrap();
        let gh = root.join("gh");
        crate::test_exec::write_executable(
            &gh,
            &format!(
                r#"#!/bin/sh
[ -z "${{GH_REPO+x}}" ] && [ "$6 $7" = "-R fixture/repo" ] || exit 1
echo '{{"headRefOid":"{head}","baseRefName":"base"}}'
"#
            ),
        );
        env.set_key("GH_REPO", "wrong/repo");
        env.set_key("AIDA_TEST_GH_BINARY", &gh);
        assert_eq!(
            resolve_comparison_endpoints(root, &pr).unwrap().2,
            Some(head.clone())
        );
        let clone = temp.path().join("shallow");
        git_text(
            root,
            &[
                "clone",
                "--depth=1",
                &format!("file://{}", root.display()),
                "shallow",
            ],
        )
        .unwrap();
        let args = ReviewPrepareArgs {
            commit: Some(head),
            ..Default::default()
        };
        assert!(resolve_comparison_endpoints(&clone, &args)
            .unwrap_err()
            .to_string()
            .contains("parent unavailable"));
    }

    #[test]
    fn git_inventory_and_content_errors_propagate() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for (result, operation) in [
            (
                collect_git_diff_entries(root, "workspace", None, None, &[]).map(|_| ()),
                "diff",
            ),
            (
                collect_git_scan_entries(root, &[], &[]).map(|_| ()),
                "ls-files",
            ),
            (
                read_file_at_revision(root, Some("HEAD"), "missing").map(|_| ()),
                "show",
            ),
        ] {
            assert!(result.unwrap_err().to_string().contains(operation));
        }
    }

    // trace:STORY-3120 | ai:antigravity
    #[test]
    fn turnkey_run_review_sarif_and_post_comment_end_to_end() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // Mock runner script:
        // - In Pass 1 (`AIDA_REVIEW_PASS=finder`), returns 2 candidate findings (1 real `.unwrap()`, 1 false positive) + 100% file coverage.
        // - In Pass 2 (`AIDA_REVIEW_PASS=challenger`), confirms index 0 and dismisses index 1.
        let runner = root.join("mock-runner.sh");
        let finder_json = serde_json::json!({
            "findings": [
                {
                    "path": "lib.rs",
                    "severity": "high",
                    "category": "fail-open",
                    "message": "Avoid .unwrap() in production path.",
                    "existing_code": "    let danger = Some(1).unwrap();",
                    "suggestion_code": "    let danger = Some(1)?;"
                },
                {
                    "path": "lib.rs",
                    "severity": "low",
                    "category": "bug",
                    "message": "False positive nitpick.",
                    "existing_code": "    let _ = danger;"
                }
            ],
            "files": [
                { "path": "lib.rs", "status": "reviewed" }
            ]
        });
        let challenger_json = serde_json::json!({
            "verdicts": [
                { "index": 0, "verdict": "confirmed", "reason": "Unchecked unwrap in production." },
                { "index": 1, "verdict": "dismissed", "reason": "Harmless binding." }
            ]
        });
        crate::test_exec::write_executable(
            &runner,
            &format!(
                "#!/bin/sh\ncat >/dev/null\nif [ \"$AIDA_REVIEW_PASS\" = \"challenger\" ]; then\n  printf '%s\\n' '{challenger_json}'\nelse\n  printf '%s\\n' '{finder_json}'\nfi\n"
            ),
        );

        let _guard = crate::test_env::AmbientGuard::hermetic_and(
            root,
            None,
            &[("AIDA_REVIEW_RUNNER_CMD", Some(runner.to_str().unwrap()))],
        );
        let store = Storage::new(root.join("requirements.yaml"));

        git_text(root, &["init", "-q"]).unwrap();
        std::fs::write(
            root.join("lib.rs"),
            "// trace:STORY-3120 | ai:antigravity\npub fn compute() {\n    let danger = Some(1).unwrap();\n    let _ = danger;\n}\n",
        )
        .unwrap();
        git_text(root, &["add", "lib.rs"]).unwrap();
        let head = crate::test_git::commit_fixture(root);
        git_text(root, &["update-ref", "refs/remotes/origin/main", &head]).unwrap();
        git_text(
            root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/fixture/repo.git",
            ],
        )
        .unwrap();

        let args = ReviewPrepareArgs {
            scan: true,
            path: vec!["lib.rs".into()],
            run_review: true,
            adversarial: true,
            file_bugs: true,
            record_telemetry: true,
            ab_arm: Some("ab-adversarial".into()),
            model: Some("gpt-5.3-codex".into()),
            effort: Some("medium".into()),
            challenger_model: Some("o3".into()),
            challenger_effort: Some("high".into()),
            sarif: Some(PathBuf::from("review.sarif.json")),
            write: Some(PathBuf::from("bundle.json")),
            json: true,
            ..Default::default()
        };
        handle_review_prepare(&args, &store).unwrap();

        let bundle: ReviewPreparationBundle =
            serde_json::from_slice(&std::fs::read(root.join("bundle.json")).unwrap()).unwrap();
        assert!(bundle.coverage_verification.as_ref().unwrap().passed);
        assert_eq!(bundle.detected_author_agents, vec!["agy".to_string()]);
        assert_eq!(bundle.filed_bug_ids.len(), 1);
        let tel = bundle.telemetry.as_ref().unwrap();
        assert_eq!(tel.ab_arm, "ab-adversarial");
        assert_eq!(tel.pass1_candidate_findings, 2);
        assert_eq!(tel.confirmed_findings, 1);
        assert_eq!(tel.dismissed_findings, 1);
        assert_eq!(tel.false_positive_suppression_pct, 50);
        assert_eq!(tel.exact_anchor_rate_pct, 100);
        assert!(tel.estimated_input_tokens > 0);
        assert!(tel.estimated_output_tokens > 0);
        assert!(tel.estimated_cost_usd > 0.0);
        let sarif_archive_rel = tel.sarif_archive_path.as_deref().unwrap();
        assert!(root.join(sarif_archive_rel).is_file());
        assert!(root.join(".aida/review-telemetry/reviews.jsonl").is_file());

        // Verify `--telemetry-report` aggregates `.aida/review-telemetry/reviews.jsonl`
        // and dynamically evaluates live Online Addressed Rate against current files:
        let report_args = ReviewPrepareArgs {
            telemetry_report: true,
            write: Some(PathBuf::from("telemetry-report.md")),
            ..Default::default()
        };
        handle_review_prepare(&report_args, &store).unwrap();
        let report_md = std::fs::read_to_string(root.join("telemetry-report.md")).unwrap();
        assert!(report_md.contains("A/B Arm Summary"));
        assert!(report_md.contains("`ab-adversarial`"));
        assert!(report_md.contains("50% suppressed"));
        assert!(report_md.contains("0% (0)"), "{report_md}");
        assert!(report_md.contains("`aida.review.fail-open`"));

        // Simulate developer addressing the finding in `lib.rs` (replacing `.unwrap()` with `?`):
        let orig_lib = std::fs::read_to_string(root.join("lib.rs")).unwrap();
        std::fs::write(
            root.join("lib.rs"),
            "// trace:STORY-3120 | ai:antigravity\npub fn compute() -> Option<i32> {\n    let danger = Some(1)?;\n    Some(danger)\n}\n",
        )
        .unwrap();
        handle_review_prepare(&report_args, &store).unwrap();
        let report_after_fix = std::fs::read_to_string(root.join("telemetry-report.md")).unwrap();
        assert!(report_after_fix.contains("100% (1)"), "{report_after_fix}");
        assert!(
            report_after_fix.contains("1.00 / 1.00 / 1.00"),
            "{report_after_fix}"
        );
        std::fs::write(root.join("lib.rs"), orig_lib).unwrap();

        let loaded_store = store.load().unwrap();
        assert_eq!(loaded_store.requirements.len(), 1);
        assert_eq!(
            loaded_store.requirements[0].req_type,
            aida_core::RequirementType::Bug
        );

        // Verify `--fail-on high` exits non-zero when a high-severity finding is confirmed:
        let fail_args = ReviewPrepareArgs {
            scan: true,
            path: vec!["lib.rs".into()],
            run_review: true,
            adversarial: true,
            fail_on: Some("high".into()),
            ..Default::default()
        };
        let gate_err = handle_review_prepare(&fail_args, &store).unwrap_err();
        assert!(
            gate_err.to_string().contains("--fail-on high"),
            "{gate_err}"
        );

        let adv = bundle.adversarial_summary.as_ref().unwrap();
        assert_eq!(adv.candidate_findings, 2);
        assert_eq!(adv.confirmed_findings, 1);
        assert_eq!(adv.dismissed_findings, 1);
        assert_eq!(adv.dismissed_details.len(), 1);
        assert_eq!(
            adv.dismissed_details[0].dismissal_reason,
            "Harmless binding."
        );
        assert_eq!(adv.dismissed_details[0].finding.start_line, Some(4));
        assert_eq!(adv.finder_model.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(adv.finder_effort.as_deref(), Some("medium"));
        assert_eq!(adv.challenger_model.as_deref(), Some("o3"));
        assert_eq!(adv.challenger_effort.as_deref(), Some("high"));
        let anchored = bundle.anchored_findings.as_ref().unwrap();
        assert_eq!(anchored.len(), 1);
        assert_eq!(anchored[0].start_line, Some(3));
        assert_eq!(anchored[0].end_line, Some(3));
        assert_eq!(anchored[0].anchor_status.as_deref(), Some("exact"));

        let sarif: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("review.sarif.json")).unwrap())
                .unwrap();
        assert_eq!(sarif["version"], "2.1.0");
        assert_eq!(
            sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"]
                ["startLine"],
            3
        );
        assert_eq!(
            sarif["runs"][0]["results"][1]["suppressions"][0]["justification"],
            "Harmless binding."
        );
        assert_eq!(
            sarif["runs"][0]["properties"]["aidaTelemetry"]["ab_arm"],
            "ab-adversarial"
        );

        // Verify project-level default agent resolution & automatic commit-author opposition:
        // 1. When a project was initialized with only codex (`aida init --agents codex`),
        //    `resolve_review_runner_agent` defaults to `"codex"` automatically.
        crate::test_env::write_project_config_fixture(root, "[agents]\nenabled = [\"codex\"]\n");
        assert_eq!(
            resolve_review_runner_agent(
                root,
                &ReviewPrepareArgs::default(),
                &ProjectReviewRulesConfig::default()
            )
            .unwrap(),
            "codex"
        );
        // 1b. When both claude and codex are enabled and `[AI:claude]` authored the code,
        //     `resolve_review_runner_agent_with_authors` automatically opposes with `"codex"`.
        crate::test_env::write_project_config_fixture(
            root,
            "[agents]\nenabled = [\"claude\", \"codex\"]\n",
        );
        assert_eq!(
            resolve_review_runner_agent_with_authors(
                root,
                &ReviewPrepareArgs::default(),
                &ProjectReviewRulesConfig::default(),
                &["claude".to_string()],
            )
            .unwrap(),
            "codex"
        );
        // 2. `.aida/review-rules.toml` `default_agent` overrides `aida init` enabled list.
        let rules_with_gemini = ProjectReviewRulesConfig {
            default_agent: Some("gemini".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_review_runner_agent(root, &ReviewPrepareArgs::default(), &rules_with_gemini)
                .unwrap(),
            "gemini"
        );
        // 3. Explicit `--agent` flag overrides project config.
        let args_with_flag = ReviewPrepareArgs {
            agent: Some("antigravity".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_review_runner_agent(root, &args_with_flag, &rules_with_gemini).unwrap(),
            "agy"
        );
    }
}
