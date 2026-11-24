// trace:STORY-3076 | ai:antigravity
//! CLI handler for `aida review prepare` — deterministic review preparation,
//! path-scoped playbook grouping, semantic file batching, file-coverage verification,
//! and snippet-anchored line positioning.

use crate::{
    detect_forge_from_origin, extract_acceptance_section, find_project_root, git_log_messages,
    pr_base_head, ReviewForge, Storage,
};
use aida_core::review_engine::{
    anchor_findings_with, bundle_review_batches, classify_diff_inventory, verify_coverage_ledger,
    ClassifiedInventory, CoverageLedger, CoverageVerificationReport, FileChangeKind,
    PlaybookRegistry, ProjectReviewRulesConfig, RawDiffEntry, RawReviewFinding, ResolvedPlaybook,
    ReviewBatch, DEFAULT_BATCH_TOKEN_BUDGET, DEFAULT_MAX_FILES_PER_BATCH,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Clap arguments for `aida review prepare`.
// trace:STORY-3076 | ai:antigravity
#[derive(clap::Args, Debug, Clone)]
pub struct ReviewPrepareArgs {
    /// Comma-separated list of AIDA spec IDs whose `## Acceptance` criteria
    /// should be injected into the review preparation bundle.
    #[clap(long, value_name = "SPEC-IDS")]
    #[clap(value_parser = crate::spec_arg::spec_id_csv)]
    pub specs: Option<String>,

    /// Pull spec IDs and base/head refs from PR/MR number N.
    #[clap(long, value_name = "N")]
    pub pr: Option<u64>,

    /// Base Git revision for range comparison (e.g. `main`).
    #[clap(long, value_name = "REF")]
    pub from: Option<String>,

    /// Target Git revision for range comparison (defaults to `HEAD` when `--from` is set).
    #[clap(long, value_name = "REF")]
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

    /// Emit machine-readable JSON output.
    #[clap(long)]
    pub json: bool,

    /// Write output to PATH instead of stdout.
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
}

/// Execute `aida review prepare`.
// trace:STORY-3076 | ai:antigravity
pub(crate) fn handle_review_prepare(args: &ReviewPrepareArgs, storage: &Storage) -> Result<()> {
    let project_root = find_project_root()?;
    let rules_cfg = ProjectReviewRulesConfig::load_from_root(&project_root);
    let registry = PlaybookRegistry::new(rules_cfg.rules.clone());

    let mut excludes = rules_cfg.exclude.clone();
    excludes.extend(args.exclude.iter().cloned());

    // 1. Resolve comparison mode and git endpoints.
    let (mode, base_ref, head_ref, merge_base) = resolve_comparison_endpoints(&project_root, args)?;

    // 2. Collect raw diff or full-file scan entries from git.
    let mut raw_entries = if mode == "scan" {
        collect_git_scan_entries(&project_root, &args.path)?
    } else {
        collect_git_diff_entries(
            &project_root,
            &mode,
            merge_base.as_deref(),
            head_ref.as_deref(),
        )?
    };
    if mode != "scan" && !args.path.is_empty() {
        raw_entries.retain(|e| path_matches_any_scope(&e.path, &args.path));
    }

    // 3. Classify inventory and cache target revision file contents for trace + facet detection.
    let target_rev = head_ref.as_deref();
    let mut content_cache: BTreeMap<String, String> = BTreeMap::new();
    let inventory = classify_diff_inventory(&raw_entries, &registry, &excludes, |rel_path| {
        if let Some(content) = read_file_at_revision(&project_root, target_rev, rel_path) {
            let has_trace = content.contains("trace:");
            content_cache.insert(rel_path.to_string(), content);
            has_trace
        } else {
            false
        }
    });

    // 4. Bundle reviewable files into semantic, token-bounded batches.
    let token_budget = rules_cfg
        .batch_token_budget
        .unwrap_or(DEFAULT_BATCH_TOKEN_BUDGET);
    let max_files = rules_cfg
        .max_files_per_batch
        .unwrap_or(DEFAULT_MAX_FILES_PER_BATCH);
    let batches = bundle_review_batches(&inventory.reviewable_files, token_budget, max_files);

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
    );

    // 7. Optional: verify a coverage ledger file.
    let coverage_verification = if let Some(ledger_path) = &args.verify_coverage {
        let raw_ledger = std::fs::read_to_string(ledger_path)
            .with_context(|| format!("read coverage ledger {}", ledger_path.display()))?;
        let ledger: CoverageLedger = serde_json::from_str(&raw_ledger)
            .with_context(|| format!("parse coverage ledger JSON {}", ledger_path.display()))?;
        Some(verify_coverage_ledger(&inventory, &ledger))
    } else {
        None
    };

    // 8. Optional: anchor findings' `existing_code` snippets to exact 1-based line numbers.
    let anchored_findings = if let Some(findings_path) = &args.anchor_findings {
        let raw_findings = std::fs::read_to_string(findings_path)
            .with_context(|| format!("read findings JSON {}", findings_path.display()))?;
        let parsed: Vec<RawReviewFinding> = serde_json::from_str(&raw_findings)
            .with_context(|| format!("parse findings JSON {}", findings_path.display()))?;
        Some(anchor_findings_with(&parsed, |rel_path| {
            read_file_at_revision(&project_root, target_rev, rel_path)
        }))
    } else {
        None
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
    };

    let rendered = if args.json {
        serde_json::to_string_pretty(&bundle)?
    } else {
        render_bundle_markdown(&bundle)
    };

    if let Some(write_path) = &args.write {
        if let Some(parent) = write_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(write_path, &rendered).with_context(|| {
            format!(
                "write review preparation output to {}",
                write_path.display()
            )
        })?;
    } else {
        println!("{rendered}");
    }

    if let Some(cov) = &bundle.coverage_verification {
        if !cov.passed {
            anyhow::bail!(
                "file coverage verification failed ({}% accounted; missing: {:?}, skipped without reason: {:?})",
                cov.coverage_rate_pct,
                cov.unaccounted_files,
                cov.skipped_without_reason
            );
        }
    }

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
        let head_sha = git_rev_parse(project_root, commit)?;
        let parent_sha = git_rev_parse(project_root, &format!("{commit}^"))
            .unwrap_or_else(|_| "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string());
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
        if let Some(forge) = detect_forge_from_origin(project_root) {
            if let Ok((base, head)) = pr_base_head(project_root, forge, pr_n) {
                let head_sha = git_rev_parse(project_root, &head).unwrap_or(head);
                let mb =
                    git_merge_base(project_root, &base, &head_sha).unwrap_or_else(|_| base.clone());
                return Ok(("range".to_string(), Some(base), Some(head_sha), Some(mb)));
            }
        }
    }

    let head_sha = git_rev_parse(project_root, "HEAD").ok();
    Ok(("workspace".to_string(), head_sha, None, None))
}

fn git_rev_parse(project_root: &Path, rev: &str) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-parse",
            "--verify",
            crate::git_arg_guard::END_OF_OPTIONS,
            rev,
        ])
        .output()
        .context("failed to run git rev-parse")?;
    if !out.status.success() {
        anyhow::bail!("git rev-parse --verify {} failed", rev);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_merge_base(project_root: &Path, base: &str, head: &str) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "merge-base",
            crate::git_arg_guard::END_OF_OPTIONS,
            base,
            head,
        ])
        .output()
        .context("failed to run git merge-base")?;
    if !out.status.success() {
        anyhow::bail!("git merge-base {} {} failed", base, head);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// trace:STORY-3076 | ai:antigravity
fn collect_git_diff_entries(
    project_root: &Path,
    mode: &str,
    merge_base: Option<&str>,
    head_ref: Option<&str>,
) -> Result<Vec<RawDiffEntry>> {
    let mut numstat_map: BTreeMap<String, (usize, usize, bool)> = BTreeMap::new();

    let range_arg = match (mode, merge_base, head_ref) {
        ("commit" | "range", Some(mb), Some(head)) => Some(format!("{mb}..{head}")),
        _ => None,
    };

    // 1. Run `git diff --no-renames --numstat` to get additions, deletions, and binary flags
    //    keyed by clean file paths without `old => new` rename formatting.
    let mut numstat_cmd = Command::new("git");
    numstat_cmd.arg("-C").arg(project_root).args([
        "--no-pager",
        "diff",
        "--no-ext-diff",
        "--no-renames",
        "--numstat",
        crate::git_arg_guard::END_OF_OPTIONS,
    ]);
    if let Some(r) = &range_arg {
        numstat_cmd.arg(r);
    } else {
        numstat_cmd.arg("HEAD");
    }
    if let Ok(out) = numstat_cmd.output() {
        if !out.status.success() {
            anyhow::bail!(
                "git diff --numstat failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if let [add_str, del_str, .., path_str] = parts.as_slice() {
                let is_bin = *add_str == "-" || *del_str == "-";
                let add = add_str.parse::<usize>().unwrap_or(0);
                let del = del_str.parse::<usize>().unwrap_or(0);
                numstat_map.insert((*path_str).to_string(), (add, del, is_bin));
            }
        }
    } else {
        anyhow::bail!("failed to spawn git diff --numstat");
    }

    // 2. Run `git diff --name-status` to get file change kinds.
    let mut status_cmd = Command::new("git");
    status_cmd.arg("-C").arg(project_root).args([
        "--no-pager",
        "diff",
        "--no-ext-diff",
        "--name-status",
        crate::git_arg_guard::END_OF_OPTIONS,
    ]);
    if let Some(r) = &range_arg {
        status_cmd.arg(r);
    } else {
        status_cmd.arg("HEAD");
    }

    let mut entries = Vec::new();
    let mut seen_paths = BTreeSet::new();

    if let Ok(out) = status_cmd.output() {
        if !out.status.success() {
            anyhow::bail!(
                "git diff --name-status failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 2 {
                continue;
            }
            let code = parts[0];
            let (kind, path, old_path) = if code.starts_with('R') && parts.len() >= 3 {
                (
                    FileChangeKind::Renamed,
                    parts[2].to_string(),
                    Some(parts[1].to_string()),
                )
            } else if code.starts_with('A') {
                (FileChangeKind::Added, parts[1].to_string(), None)
            } else if code.starts_with('D') {
                (FileChangeKind::Deleted, parts[1].to_string(), None)
            } else {
                (FileChangeKind::Modified, parts[1].to_string(), None)
            };

            let (additions, deletions, is_binary) =
                numstat_map.get(&path).copied().unwrap_or((0, 0, false));
            let patch_bytes = (additions + deletions) * 48;
            seen_paths.insert(path.clone());
            entries.push(RawDiffEntry {
                path,
                old_path,
                kind,
                additions,
                deletions,
                is_binary,
                patch_bytes,
            });
        }
    } else {
        anyhow::bail!("failed to spawn git diff --name-status");
    }

    // 3. In workspace mode, also include untracked non-ignored files.
    if mode == "workspace" {
        if let Ok(out) = Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["ls-files", "--others", "--exclude-standard"])
            .output()
        {
            if out.status.success() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    let rel = line.trim();
                    if rel.is_empty() || seen_paths.contains(rel) {
                        continue;
                    }
                    let full = project_root.join(rel);
                    let meta_len = std::fs::metadata(&full)
                        .map(|m| m.len() as usize)
                        .unwrap_or(0);
                    let (additions, is_binary) = match std::fs::read(&full) {
                        Ok(bytes) => {
                            let is_bin = bytes.iter().take(8000).any(|&b| b == 0);
                            let lines = if is_bin {
                                0
                            } else {
                                bytes.iter().filter(|&&b| b == b'\n').count() + 1
                            };
                            (lines, is_bin)
                        }
                        Err(_) => (0, false),
                    };
                    entries.push(RawDiffEntry {
                        path: rel.to_string(),
                        old_path: None,
                        kind: FileChangeKind::Untracked,
                        additions,
                        deletions: 0,
                        is_binary,
                        patch_bytes: meta_len,
                    });
                }
            }
        }
    }

    Ok(entries)
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
fn collect_git_scan_entries(project_root: &Path, paths: &[String]) -> Result<Vec<RawDiffEntry>> {
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

    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(project_root).args([
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
    ]);
    if !paths.is_empty() {
        cmd.arg("--");
        for p in paths {
            cmd.arg(p);
        }
    }

    let out = cmd
        .output()
        .context("failed to spawn git ls-files for --scan")?;
    if !out.status.success() {
        anyhow::bail!(
            "git ls-files for --scan failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let rel = line.trim();
        if rel.is_empty() || !seen.insert(rel.to_string()) {
            continue;
        }
        let full = project_root.join(rel);
        if !full.is_file() {
            continue;
        }
        let meta_len = std::fs::metadata(&full)
            .map(|m| m.len() as usize)
            .unwrap_or(0);
        let (lines, is_binary) = match std::fs::read(&full) {
            Ok(bytes) => {
                let is_bin = bytes.iter().take(8000).any(|&b| b == 0);
                let l = if is_bin {
                    0
                } else {
                    bytes.iter().filter(|&&b| b == b'\n').count() + 1
                };
                (l, is_bin)
            }
            Err(_) => (0, false),
        };
        entries.push(RawDiffEntry {
            path: rel.to_string(),
            old_path: None,
            kind: FileChangeKind::Scanned,
            additions: lines,
            deletions: 0,
            is_binary,
            patch_bytes: meta_len,
        });
    }

    Ok(entries)
}

fn read_file_at_revision(project_root: &Path, rev: Option<&str>, rel_path: &str) -> Option<String> {
    if let Some(r) = rev {
        let spec = format!("{r}:{rel_path}");
        let out = Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args([
                "--no-pager",
                "show",
                crate::git_arg_guard::END_OF_OPTIONS,
                &spec,
            ])
            .output()
            .ok()?;
        if out.status.success() {
            return String::from_utf8(out.stdout).ok();
        }
        return None;
    }
    // Workspace mode: prevent path traversal outside project_root.
    if Path::new(rel_path).components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::RootDir
        )
    }) {
        return None;
    }
    std::fs::read_to_string(project_root.join(rel_path)).ok()
}

fn resolve_linked_specs(
    project_root: &Path,
    storage: &Storage,
    args: &ReviewPrepareArgs,
    merge_base: Option<&str>,
    head_ref: Option<&str>,
) -> Vec<LinkedSpecAcceptance> {
    let Ok(store) = storage.load() else {
        return Vec::new();
    };

    let mut spec_ids: Vec<String> = Vec::new();
    if let Some(csv) = &args.specs {
        spec_ids.extend(
            csv.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    } else if let (Some(mb), Some(head)) = (merge_base, head_ref) {
        if let Ok(messages) = git_log_messages(project_root, mb, head) {
            let cov = crate::review_gates::review_coverage_from_commits(&messages);
            spec_ids.extend(cov.delivered);
            spec_ids.extend(cov.slice_refs);
        }
    }

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
                spec_id: req.display_id().to_string(),
                title: req.title.clone(),
                status: req.status.to_string(),
                acceptance_markdown: acceptance,
            });
        }
    }
    out
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
                spec.spec_id, spec.title, spec.status, spec.acceptance_markdown
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

    out
}
