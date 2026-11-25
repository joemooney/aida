// trace:TASK-3862 | ai:codex
// trace:STORY-3076 | ai:antigravity
//! CLI handler for `aida review prepare` — deterministic review preparation,
//! path-scoped playbook grouping, semantic file batching, file-coverage verification,
//! and snippet-anchored line positioning.

use crate::{detect_forge_from_origin, extract_acceptance_section, find_project_root, Storage};
use aida_core::review_engine::{
    anchor_findings_with, bundle_review_batches, classify_diff_inventory, findings_to_sarif_json,
    format_forge_review_markdown, parse_runner_findings_json, verify_coverage_ledger,
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

    /// Export anchored findings as a SARIF 2.1.0 JSON file inside the repository
    /// (for GitHub Code Scanning / GitLab SAST).
    // trace:STORY-3120 | ai:antigravity
    #[clap(long, value_name = "SARIF_PATH")]
    pub sarif: Option<PathBuf>,

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
}

// trace:TASK-3862 | ai:codex
fn load_review_rules(project_root: &Path) -> Result<ProjectReviewRulesConfig> {
    ProjectReviewRulesConfig::load_from_root(project_root).context("load .aida/review-rules.toml")
}

/// Execute `aida review prepare`.
// trace:STORY-3076 | ai:antigravity
pub(crate) fn handle_review_prepare(args: &ReviewPrepareArgs, storage: &Storage) -> Result<()> {
    let project_root = find_project_root()?;
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

    // 7. Optional: execute automated batch review (`--run-review`), or load external
    //    coverage ledger (`--verify-coverage`) and findings (`--anchor-findings`).
    let mut runner_findings: Option<Vec<RawReviewFinding>> = None;
    let mut runner_ledger: Option<CoverageLedger> = None;
    if args.run_review {
        let resolved_agent = resolve_review_runner_agent(&project_root, args, &rules_cfg)?;
        let (f, l) = execute_automated_batch_reviews(
            &project_root,
            &resolved_agent,
            &mode,
            merge_base.as_deref(),
            head_ref.as_deref(),
            &batches,
            &playbooks,
            &linked_specs,
            &content_cache,
        )?;
        runner_findings = Some(f);
        runner_ledger = Some(l);
    }

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

    // 9. Optional: write SARIF 2.1.0 report (`--sarif <PATH>`).
    if let Some(sarif_path) = &args.sarif {
        let findings_slice = anchored_findings.as_deref().unwrap_or(&[]);
        let sarif_doc = findings_to_sarif_json(findings_slice);
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
        let comment_md = format_forge_review_markdown(
            &mode,
            head_ref.as_deref(),
            &inventory,
            coverage_verification.as_ref(),
            &spec_pairs,
            anchored_findings.as_deref().unwrap_or(&[]),
        );
        post_forge_review_comment(&project_root, pr_n, &comment_md)?;
    }

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

    out
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

/// Execute automated batch code reviews over each prepared batch using either
/// `AIDA_REVIEW_RUNNER_CMD` (invoked with prompt on stdin) or the resolved agent CLI.
// trace:STORY-3120 | ai:antigravity
fn execute_automated_batch_reviews(
    project_root: &Path,
    agent: &str,
    mode: &str,
    merge_base: Option<&str>,
    head_ref: Option<&str>,
    batches: &[ReviewBatch],
    playbooks: &[ResolvedPlaybook],
    linked_specs: &[LinkedSpecAcceptance],
    content_cache: &BTreeMap<String, String>,
) -> Result<(Vec<RawReviewFinding>, CoverageLedger)> {
    let playbooks_map: BTreeMap<&str, &ResolvedPlaybook> =
        playbooks.iter().map(|p| (p.id.as_str(), p)).collect();

    let mut all_findings = Vec::new();
    let mut all_coverage = Vec::new();

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
        let raw_out = invoke_review_runner(project_root, agent, &prompt)?;
        let (batch_findings, batch_ledger) = parse_runner_findings_json(&raw_out)
            .map_err(|e| anyhow::anyhow!("batch {} runner output error: {e}", batch.batch_index))?;
        all_findings.extend(batch_findings);
        all_coverage.extend(batch_ledger.files);
    }

    Ok((
        all_findings,
        CoverageLedger {
            files: all_coverage,
        },
    ))
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

fn invoke_review_runner(project_root: &Path, agent: &str, prompt: &str) -> Result<String> {
    use std::io::Write;
    use std::process::Stdio;

    if let Ok(custom_cmd) = std::env::var("AIDA_REVIEW_RUNNER_CMD") {
        let trimmed = custom_cmd.trim();
        if !trimmed.is_empty() {
            let mut child = Command::new(trimmed)
                .current_dir(project_root)
                .env("AIDA_REVIEW_RESOLVED_AGENT", agent)
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

    let (program, mut argv) = match agent {
        "codex" => {
            let mut args = vec![
                "exec".to_string(),
                "--sandbox".to_string(),
                "read-only".to_string(),
                "--skip-git-repo-check".to_string(),
            ];
            if let Some(model) = tuning.model {
                args.push("--model".to_string());
                args.push(model);
            }
            if let Some(effort) = tuning.effort {
                args.push("-c".to_string());
                args.push(format!("model_reasoning_effort={effort}"));
            }
            args.push("--".to_string());
            args.push(prompt.to_string());
            ("codex", args)
        }
        "agy" => ("agy", vec!["-p".to_string(), prompt.to_string()]),
        "gemini" => ("gemini", vec!["-p".to_string(), prompt.to_string()]),
        _ => {
            let mut args = vec!["--print".to_string()];
            if let Some(model) = tuning.model {
                args.push("--model".to_string());
                args.push(model);
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

        // Mock runner script that returns a finding with `existing_code` and a 100% file coverage row
        let runner = root.join("mock-runner.sh");
        let runner_json = serde_json::json!({
            "findings": [
                {
                    "path": "lib.rs",
                    "severity": "high",
                    "category": "fail-open",
                    "message": "Avoid .unwrap() in production path.",
                    "existing_code": "    let danger = Some(1).unwrap();",
                    "suggestion_code": "    let danger = Some(1)?;"
                }
            ],
            "files": [
                { "path": "lib.rs", "status": "reviewed" }
            ]
        });
        crate::test_exec::write_executable(
            &runner,
            &format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{runner_json}'\n"),
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
            sarif: Some(PathBuf::from("review.sarif.json")),
            write: Some(PathBuf::from("bundle.json")),
            json: true,
            ..Default::default()
        };
        handle_review_prepare(&args, &store).unwrap();

        let bundle: ReviewPreparationBundle =
            serde_json::from_slice(&std::fs::read(root.join("bundle.json")).unwrap()).unwrap();
        assert!(bundle.coverage_verification.as_ref().unwrap().passed);
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

        // Verify project-level default agent resolution:
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
