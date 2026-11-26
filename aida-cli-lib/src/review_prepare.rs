// trace:TASK-3862 | ai:codex
// trace:STORY-3076 | ai:antigravity
//! CLI handler for `aida review prepare` — deterministic review preparation,
//! path-scoped playbook grouping, semantic file batching, file-coverage verification,
//! and snippet-anchored line positioning.

use crate::launch::review_challenger::{self as challenger, AdversarialSummary, RefutedFinding};
use crate::launch::review_runner::{RunnerCall, STDERR_CAP, STDOUT_CAP};
use crate::launch::review_tuning::{resolve_for_run, trusted_review_base};
use crate::vendors::review_runner::{review_vendor, ReviewVendor, DEFAULT_REVIEW_VENDOR};
use crate::{detect_forge_from_origin, extract_acceptance_section, find_project_root, Storage};
use aida_core::exec_context::Environment;
use aida_core::review_engine::{
    anchor_findings_with, bundle_review_batches, classify_with_exclusions, verify_coverage_ledger,
    ClassifiedInventory, CoverageLedger, CoverageVerificationReport, FileChangeKind,
    PlaybookRegistry, ProjectReviewRulesConfig, RawDiffEntry, RawReviewFinding, ResolvedPlaybook,
    ReviewBatch, DEFAULT_BATCH_TOKEN_BUDGET, DEFAULT_MAX_FILES_PER_BATCH,
};
use aida_core::review_engine::{classify_compiled, compile_exclusions, CompiledGlob};
use aida_core::review_engine::{FileCoverageEntry, FileCoverageStatus};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Clap arguments for `aida review prepare`.
// trace:STORY-3076 | ai:antigravity
#[derive(clap::Args, Debug, Clone, Default)]
#[clap(group(clap::ArgGroup::new("findings_source").args(["anchor_findings", "run_review"])))]
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

    /// Review every batch with an agent and anchor its findings. Needs `--pr`
    /// or `--from`; review settings come from the merge base.
    // trace:STORY-3120 | ai:claude
    #[clap(long, conflicts_with_all = ["anchor_findings", "scan", "commit", "verify_coverage"])]
    pub run_review: bool,

    /// Agent for `--run-review`: `claude` or `codex` (or set AIDA_REVIEW_RUNNER_CMD).
    #[clap(long, value_name = "AGENT", requires = "run_review")]
    pub agent: Option<String>,

    /// Reasoning effort for `--run-review`: low, medium, high, xhigh (claude: max).
    #[clap(long, value_name = "LEVEL", requires = "run_review")]
    pub effort: Option<String>,

    /// Model for `--run-review`.
    #[clap(long, value_name = "MODEL", requires = "run_review")]
    pub model: Option<String>,

    /// Have a second agent reproduce or refute each finding. Refuted findings
    /// are listed apart and marked suppressed in SARIF, but still count for
    /// `--fail-on`. It prefers a vendor that did not write the change.
    // trace:STORY-3120 | ai:claude
    #[clap(long, requires = "run_review")]
    pub adversarial: bool,

    /// Reasoning effort for the `--adversarial` challenger.
    #[clap(long, value_name = "LEVEL", requires = "adversarial")]
    pub challenger_effort: Option<String>,

    /// Write a SARIF 2.1.0 report inside the repository.
    #[clap(long, value_name = "PATH", requires = "findings_source")]
    pub sarif: Option<PathBuf>,

    /// Exit with an error when any finding meets or exceeds this severity.
    #[clap(long, value_name = "SEVERITY", requires = "findings_source")]
    pub fail_on: Option<String>,

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
    pub adversarial: Option<AdversarialSummary>,
}

// trace:TASK-3862 | ai:codex
fn load_review_rules(project_root: &Path) -> Result<ProjectReviewRulesConfig> {
    ProjectReviewRulesConfig::load_from_root(project_root).context("load .aida/review-rules.toml")
}

/// Execute `aida review prepare`.
// trace:STORY-3076 | ai:antigravity
pub(crate) fn handle_review_prepare(args: &ReviewPrepareArgs, storage: &Storage) -> Result<()> {
    let project_root = find_project_root()?;
    // 1. Resolve comparison mode and git endpoints.
    let (mode, base_ref, head_ref, merge_base) = resolve_comparison_endpoints(&project_root, args)?;
    // trace:STORY-3120 | ai:claude
    let trusted = if args.run_review {
        let base = run_review_base(&mode, merge_base.as_deref())?;
        let head = head_ref.as_deref();
        Some(trusted_review_base(&project_root, base, head)?)
    } else {
        None
    };
    let rules_cfg = match &trusted {
        Some(trusted) => trusted.rules.clone(),
        None => load_review_rules(&project_root)?,
    };
    let registry = PlaybookRegistry::new(rules_cfg.rules.clone())?;

    let mut excludes = rules_cfg.exclude.clone();
    excludes.extend(args.exclude.iter().cloned());

    let excludes = compile_exclusions(&excludes)?;

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
    let inventory = classify_with_exclusions(
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
    let mut playbooks_by_id: BTreeMap<String, (ResolvedPlaybook, Vec<(&str, &str)>)> =
        BTreeMap::new();
    for rf in &inventory.reviewable_files {
        let entry = playbooks_by_id
            .entry(rf.playbook_id.clone())
            .or_insert_with(|| (registry.resolve(&rf.path), Vec::new()));
        if let Some(body) = content_cache.get(&rf.path) {
            entry.1.push((&rf.path, body.as_str()));
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

    // 7. Optional: verify a coverage ledger file.
    let mut coverage_verification = if let Some(ledger_path) = &args.verify_coverage {
        let raw_ledger = std::fs::read_to_string(ledger_path)
            .with_context(|| format!("read coverage ledger {}", ledger_path.display()))?;
        let ledger: CoverageLedger = serde_json::from_str(&raw_ledger)
            .with_context(|| format!("parse coverage ledger JSON {}", ledger_path.display()))?;
        Some(verify_coverage_ledger(&inventory, &ledger))
    } else {
        None
    };

    // 8. Optional: anchor findings' `existing_code` snippets to exact 1-based line numbers.
    let mut adversarial = None;
    let anchored_findings = if let Some(findings_path) = &args.anchor_findings {
        let raw_findings = std::fs::read_to_string(findings_path)
            .with_context(|| format!("read findings JSON {}", findings_path.display()))?;
        // trace:STORY-3120 | ai:codex
        let (parsed, _) = aida_core::review_engine::parse_runner_findings_json(&raw_findings)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("parse findings JSON {}", findings_path.display()))?;
        ensure_in_inventory(&inventory, &parsed)?;
        Some(anchor_findings_with(&parsed, |path| {
            content_cache.get(path).cloned()
        }))
    } else if let Some(trusted) = &trusted {
        // trace:STORY-3120 | ai:claude
        let parent_env = Environment::capture().to_pairs();
        let agent = run_review_agent(args.agent.as_deref(), &project_root, &parent_env)?;
        let mut call = RunnerCall {
            agent,
            checkout: &project_root,
            parent_env: &parent_env,
            pass: "finder",
            deadline: RUN_DEADLINE,
            stdout_cap: STDOUT_CAP,
            stderr_cap: STDERR_CAP,
            tuning: Default::default(),
        };
        let flags = (args.effort.as_deref(), args.model.as_deref());
        call.tuning = resolve_for_run(call.vendor(), &parent_env, flags, trusted)?;
        let (base, head) = (merge_base.as_deref(), head_ref.as_deref());
        let range = format!("{}..{}", base.unwrap_or_default(), head.unwrap_or_default());
        // A credential file's old content never reaches a runner, renamed or not.
        let renames: BTreeMap<&str, &str> = (inventory.reviewable_files.iter())
            .filter_map(|f| Some((f.path.as_str(), f.old_path.as_deref()?)))
            .filter(|(_, old)| !aida_core::review_engine::is_credential_path(old))
            .collect();
        let prompt = |batch: &ReviewBatch| {
            batch_prompt(
                &project_root,
                &range,
                batch,
                &renames,
                &playbooks,
                &linked_specs,
            )
        };
        // trace:STORY-3120 | ai:claude
        let challenge = if args.adversarial {
            let log = [
                "log",
                "--no-color",
                "--format=%B",
                "--end-of-options",
                &range,
            ];
            let authors = challenger::detect_ai_authors(&git_text(&project_root, &log)?);
            let effort = args.challenger_effort.as_deref();
            let (pass, summary) = challenger::challenger_call(&call, &authors, effort, trusted)?;
            if let Some(note) = &summary.note {
                eprintln!("adversarial review: {note}");
            }
            Some((pass, summary))
        } else {
            None
        };
        let pass = challenge.as_ref().map(|(pass, _)| pass);
        let (found, ledger, refuted) = review_batches(&call, pass, &batches, prompt, &inventory)?;
        let read = |path: &str| content_cache.get(path).cloned();
        // Refuted findings are anchored too: they still gate and reach SARIF.
        let rows: Vec<_> = refuted.iter().map(|r| r.finding.clone()).collect();
        let refuted = (refuted.into_iter().zip(anchor_findings_with(&rows, read)))
            .map(|(r, finding)| RefutedFinding { finding, ..r })
            .collect();
        adversarial = challenge.map(|(_, summary)| AdversarialSummary { refuted, ..summary });
        // A runner that does not report every reviewable file is incomplete.
        coverage_verification = Some(verify_coverage_ledger(&inventory, &ledger));
        Some(anchor_findings_with(&found, read))
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
        adversarial,
    };

    let rendered = if args.json {
        serde_json::to_string_pretty(&bundle)?
    } else {
        render_bundle_markdown(&bundle)
    };

    // trace:STORY-3120 | ai:codex
    let findings = bundle.anchored_findings.as_deref().unwrap_or_default();
    // Fail closed: a challenger's refutation never removes a finding from
    // `--fail-on`, so a wrong or prompt-injected challenger cannot turn the
    // gate green; SARIF keeps it as a suppressed result. trace:STORY-3120 | ai:claude
    let suppressed: Vec<_> = (bundle.adversarial.iter().flat_map(|a| &a.refuted))
        .map(|r| {
            (
                r.finding.clone(),
                format!("refuted by challenger: {}", r.reason),
            )
        })
        .collect();
    let gated: Vec<_> = (findings.iter().cloned())
        .chain(suppressed.iter().map(|(f, _)| f.clone()))
        .collect();
    let exceeded = args
        .fail_on
        .as_deref()
        .map(|threshold| {
            aida_core::review_engine::findings_exceeding_threshold(&gated, threshold)
                .map_err(anyhow::Error::msg)
        })
        .transpose()?;

    if let Some(write_path) = &args.write {
        write_output(&project_root, write_path, &rendered)?;
    } else {
        println!("{rendered}");
    }

    if let Some(path) = &args.sarif {
        let report =
            aida_core::review_engine::findings_to_sarif_json_with_suppressed(findings, &suppressed);
        write_output(&project_root, path, &serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("write SARIF {}", path.display()))?;
    }
    if let (Some(threshold), Some(matches)) = (&args.fail_on, exceeded) {
        anyhow::ensure!(
            matches.is_empty(),
            "{} findings meet or exceed --fail-on {threshold}",
            matches.len()
        );
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

/// Per-batch runner deadline for `--run-review`.
const RUN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// The trusted base for `--run-review`: the merge base of the target (the
/// forge's with `--pr`, `--from` otherwise) and the reviewed head. Workspace,
/// scan and commit modes have no base the change cannot choose, so they refuse.
// trace:STORY-3120 | ai:claude
fn run_review_base<'a>(mode: &str, merge_base: Option<&'a str>) -> Result<&'a str> {
    match (mode, merge_base) {
        ("range", Some(base)) => Ok(base),
        _ => anyhow::bail!("--run-review needs --pr or --from"),
    }
}

fn ensure_in_inventory(inventory: &ClassifiedInventory, found: &[RawReviewFinding]) -> Result<()> {
    let allowed: BTreeSet<_> = inventory
        .reviewable_files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    for finding in found {
        // trace:TASK-3320 STORY-3120 | ai:claude
        anyhow::ensure!(
            !aida_core::review_engine::is_credential_path(&finding.path),
            "finding cites a credential path, which review never reads: {:?}",
            finding.path
        );
        anyhow::ensure!(
            allowed.contains(finding.path.as_str()),
            "finding path outside reviewable inventory: {:?}",
            finding.path
        );
    }
    Ok(())
}

const PROMPT_HEAD: &str = "Review this batch of a code change for defects. Report only \
files listed below. Reply with JSON only: {\"findings\":[{\"path\":\"<listed file>\",\
\"severity\":\"critical|high|medium|low\",\"category\":\"bug|security|race|perf|untested-path\",\
\"message\":\"...\",\"existing_code\":\"verbatim lines from the new file\"}],\
\"files\":[{\"path\":\"<listed file>\",\"status\":\"reviewed\"}]}\n\n";

/// One batch's context: acceptance criteria, playbooks and each file's diff,
/// from its old path for a rename. A failed diff is an error, never replaced
/// by the whole file. `review_batches` prepends each pass's instructions.
// trace:STORY-3120 | ai:claude
fn batch_prompt(
    root: &Path,
    range: &str,
    batch: &ReviewBatch,
    renames: &BTreeMap<&str, &str>,
    playbooks: &[ResolvedPlaybook],
    specs: &[LinkedSpecAcceptance],
) -> Result<String> {
    let mut prompt = String::new();
    for spec in specs {
        let id = aida_core::node_alias::display_spec_id(&spec.spec_id);
        let (title, acceptance) = (&spec.title, &spec.acceptance_markdown);
        prompt.push_str(&format!("## {id} — {title}\n{acceptance}\n\n"));
    }
    for pb in playbooks
        .iter()
        .filter(|pb| batch.playbook_ids.contains(&pb.id))
    {
        let (title, id, checklist) = (&pb.title, &pb.id, &pb.checklist);
        prompt.push_str(&format!("## Playbook {title} (`{id}`)\n{checklist}\n\n"));
    }
    for path in &batch.files {
        let mut args = vec!["diff", "--no-ext-diff", "--no-color", "--find-renames"];
        args.extend(["--end-of-options", range, "--", path]);
        args.extend(renames.get(path.as_str()));
        let patch = git_text(root, &args)?;
        let patch = if patch.trim().is_empty() {
            "(no textual change)\n"
        } else {
            patch.as_str()
        };
        prompt.push_str(&format!("### File: `{path}`\n````diff\n{patch}````\n\n"));
    }
    Ok(prompt)
}

/// One-line runner error naming who ran, the pass, the batch and the whole
/// cause chain: agent-mode errors keep only the first line. trace:BUG-2775 | ai:claude
fn runner_failure(call: &RunnerCall<'_>, batch: &str, e: anyhow::Error) -> anyhow::Error {
    let (who, pass) = (call.label(), call.pass);
    anyhow::anyhow!("{who} {pass} pass, {batch}: {e:#}")
}

/// The finder's vendor: `--agent`, else none under `AIDA_REVIEW_RUNNER_CMD`,
/// else `[agents] vendor` (claude or codex only), else claude, stated on stderr. trace:BUG-2775 | ai:claude
fn run_review_agent(
    flag: Option<&str>,
    root: &Path,
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> Result<Option<&'static ReviewVendor>> {
    if let Some(name) = flag {
        return review_vendor(name).map(Some);
    }
    let custom = env
        .iter()
        .any(|(k, v)| k == "AIDA_REVIEW_RUNNER_CMD" && !v.is_empty());
    if custom {
        return Ok(None);
    }
    let (vendor, from) = match aida_core::agents_config::resolve_default_vendor(root) {
        Some(name) => (review_vendor(&name)?, "[agents] vendor"),
        None => (
            DEFAULT_REVIEW_VENDOR,
            "the built-in default (no [agents] vendor)",
        ),
    };
    eprintln!("run review: using {} from {from}", vendor.program);
    Ok(Some(vendor))
}

/// Run each batch prompt; any runner error, unparsable output, finding outside
/// the inventory or its batch, or ledger row outside its batch refuses the
/// whole run. The
/// returned ledger keeps only files the runner reports `reviewed`, so a skip
/// or silence leaves the file unaccounted when the caller verifies coverage.
/// With a `challenger`, each batch's findings must then be reproduced (kept)
/// or refuted (returned apart) by it.
// trace:STORY-3120 | ai:claude
fn review_batches(
    call: &RunnerCall<'_>,
    challenge: Option<&RunnerCall<'_>>,
    batches: &[ReviewBatch],
    mut prompt: impl FnMut(&ReviewBatch) -> Result<String>,
    inventory: &ClassifiedInventory,
) -> Result<(Vec<RawReviewFinding>, CoverageLedger, Vec<RefutedFinding>)> {
    let (mut found, mut ledger, mut refuted) = (Vec::new(), CoverageLedger::default(), Vec::new());
    for (i, batch) in batches.iter().enumerate() {
        let context = prompt(batch)?;
        let fail = |pass: &RunnerCall<'_>, what: &str, e: anyhow::Error| {
            runner_failure(pass, &format!("{what} {}", i + 1), e)
        };
        let out = call
            .run(&format!("{PROMPT_HEAD}{context}"))
            .map_err(|e| fail(call, "review batch", e))?;
        let (rows, files) = aida_core::review_engine::parse_runner_findings_json(&out)
            .map_err(anyhow::Error::msg)
            .context("runner output rejected")
            .map_err(|e| fail(call, "review batch", e))?;
        ensure_in_inventory(inventory, &rows)?;
        // A batch's runner saw only its own files. trace:STORY-3120 | ai:claude
        for row in &rows {
            anyhow::ensure!(
                batch.files.contains(&row.path),
                "review batch {} cites a finding outside its batch: {:?}",
                i + 1,
                row.path
            );
        }
        for entry in &files.files {
            anyhow::ensure!(
                batch.files.contains(&entry.path),
                "review batch {} reports coverage for a file outside it: {:?}",
                i + 1,
                entry.path
            );
        }
        let rows = match challenge {
            Some(pass) if !rows.is_empty() => {
                let out = pass
                    .run(&challenger::challenge_prompt(&rows, &context)?)
                    .map_err(|e| fail(pass, "challenge batch", e))?;
                let (kept, dropped) = challenger::apply_verdicts(rows, &out)
                    .context("runner output rejected")
                    .map_err(|e| fail(pass, "challenge batch", e))?;
                refuted.extend(dropped);
                kept
            }
            _ => rows,
        };
        found.extend(rows);
        let reviewed = |e: &FileCoverageEntry| e.status == FileCoverageStatus::Reviewed;
        ledger
            .files
            .extend(files.files.into_iter().filter(reviewed));
    }
    Ok((found, ledger, refuted))
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
    excludes: &[(&str, CompiledGlob)],
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

/// Variables `git_text` keeps; GIT_DIR, GIT_CONFIG*, GIT_INDEX_FILE and the
/// rest of the caller's environment never reach git. trace:STORY-3120 | ai:claude
fn git_env_kept(key: &std::ffi::OsStr) -> bool {
    let key = key.to_string_lossy();
    ["PATH", "HOME", "LANG", "XDG_CONFIG_HOME"].contains(&&*key) || key.starts_with("LC_")
}

/// Host git (seat-clone refusal, fsmonitor off, store pins) with only the
/// `git_env_kept` part of the caller's environment. trace:STORY-3120 | ai:claude
fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = aida_core::git_ops::host_git_command(root);
    // The builder's own pins survive the clear; inherited variables do not.
    let pins: Vec<_> = cmd
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_owned(), v?.to_owned())))
        .collect();
    cmd.env_clear()
        .envs(
            Environment::capture()
                .to_pairs()
                .into_iter()
                .filter(|(k, _)| git_env_kept(k)),
        )
        .envs(pins)
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .current_dir(root);
    // No textconv driver from the reviewed tree's attributes ever runs.
    match args.split_first() {
        Some((sub @ (&"diff" | &"show" | &"log"), rest)) => {
            cmd.arg(sub).arg("--no-textconv").args(rest)
        }
        _ => cmd.args(args),
    };
    let out = cmd
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
    excludes: &[(&str, CompiledGlob)],
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
    excludes: &[(&str, CompiledGlob)],
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
        if classify_compiled(&entry, excludes).is_none() {
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

    // trace:STORY-3120 | ai:claude
    if let Some(adv) = &bundle.adversarial {
        let agent = adv.challenger_agent.as_deref().unwrap_or("custom runner");
        out.push_str(&format!(
            "## Adversarial Challenge\n\n- **Challenger**: `{agent}`\n"
        ));
        if let Some(note) = &adv.note {
            out.push_str(&format!("- **Note**: {note}\n"));
        }
        for r in &adv.refuted {
            let (path, msg) = (&r.finding.path, &r.finding.message);
            out.push_str(&format!("- **Refuted** `{path}`: {msg} ({})\n", r.reason));
        }
        out.push('\n');
    }

    out
}

// trace:TASK-3862 | ai:codex
#[cfg(test)]
mod tests {
    use super::*;

    // trace:STORY-3120 | ai:codex
    #[test]
    fn s2_export_gate_checks_changed_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _guard = crate::test_env::AmbientGuard::hermetic(root, None);
        let store = Storage::new(root.join("requirements.yaml"));
        git_text(root, &["init", "-q"]).unwrap();
        std::fs::write(root.join("unchanged.rs"), "old").unwrap();
        git_text(root, &["add", "unchanged.rs"]).unwrap();
        let base = crate::test_git::commit_fixture(root);
        std::fs::write(root.join("changed.rs"), "new").unwrap();
        git_text(root, &["add", "changed.rs"]).unwrap();
        crate::test_git::commit_fixture(root);
        let input = root.join("findings.json");
        let output = root.join("report.sarif");
        let mut args = ReviewPrepareArgs {
            from: Some(base),
            anchor_findings: Some(input.clone()),
            sarif: Some(output.clone()),
            fail_on: Some("high".into()),
            ..Default::default()
        };
        let finding = |path| {
            serde_json::json!({"path":path,"severity":"high",
            "category":"bug","message":"example","existing_code":"new"})
        };
        let valid = finding("changed.rs");
        std::fs::write(
            &input,
            serde_json::json!([valid, finding("unchanged.rs")]).to_string(),
        )
        .unwrap();
        let error = handle_review_prepare(&args, &store).unwrap_err();
        assert!(error.to_string().contains("outside reviewable inventory"));
        assert!(!output.exists());
        std::fs::write(
            &input,
            serde_json::json!({"findings":[valid],"files":[]}).to_string(),
        )
        .unwrap();
        let error = handle_review_prepare(&args, &store).unwrap_err();
        assert!(error.to_string().contains("1 findings meet or exceed"));
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let result = &report["runs"][0]["results"][0];
        assert_eq!(
            result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "changed.rs"
        );
        args.fail_on = Some("critical".into());
        handle_review_prepare(&args, &store).unwrap();
    }

    // trace:TASK-3320 | ai:claude
    #[test]
    fn root_credential_dirs_excluded_reported_and_refused_as_findings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let _guard = crate::test_env::AmbientGuard::hermetic(root, None);
        let store = Storage::new(root.join("requirements.yaml"));
        git_text(root, &["init", "-q"]).unwrap();
        std::fs::write(root.join("seed.rs"), "seed").unwrap();
        git_text(root, &["add", "seed.rs"]).unwrap();
        let base = crate::test_git::commit_fixture(root);
        let creds = [
            ".aws/credentials",
            ".gnupg/secring.gpg",
            ".config/gcloud/credentials.db",
            ".docker/config.json",
            ".netrc",
        ];
        let mut expected = Vec::new();
        for rel in creds {
            for path in [rel.to_string(), format!("nested/{rel}")] {
                let full = root.join(&path);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(&full, "SENTINEL_SECRET_3320\n").unwrap();
                git_text(root, &["add", "-f", &path]).unwrap();
                expected.push(path);
            }
        }
        std::fs::write(root.join("ordinary.rs"), "fn ok() {}\n").unwrap();
        git_text(root, &["add", "ordinary.rs"]).unwrap();
        crate::test_git::commit_fixture(root);
        let dest = root.join("bundle.json");
        let mut args = ReviewPrepareArgs {
            from: Some(base),
            json: true,
            write: Some(dest.clone()),
            ..Default::default()
        };
        handle_review_prepare(&args, &store).unwrap();
        let rendered = std::fs::read_to_string(&dest).unwrap();
        assert!(!rendered.contains("SENTINEL_SECRET_3320"));
        let bundle: ReviewPreparationBundle = serde_json::from_str(&rendered).unwrap();
        let reviewable: Vec<_> = bundle
            .inventory
            .reviewable_files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(reviewable, ["ordinary.rs"]);
        expected.sort();
        let excluded: Vec<_> = bundle
            .inventory
            .excluded_files
            .iter()
            .inspect(|f| assert_eq!(f.reason, "secret_sensitive_path", "{}", f.path))
            .map(|f| f.path.clone())
            .collect();
        assert_eq!(excluded, expected);

        let input = root.join("findings.json");
        args.anchor_findings = Some(input.clone());
        for cited in [".netrc", "nested/.docker/config.json"] {
            let finding = serde_json::json!([{"path":cited,"severity":"low",
                "category":"bug","message":"m","existing_code":"SENTINEL_SECRET_3320"}]);
            std::fs::write(&input, finding.to_string()).unwrap();
            let error = handle_review_prepare(&args, &store).unwrap_err();
            assert!(error.to_string().contains("credential path"), "{error:#}");
        }
    }

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
        let pattern = format!("{{{}z}}", "x,".repeat(256));
        std::fs::write(
            config_dir.join("review-rules.toml"),
            format!("[[rules]]\nid = 'custom-check'\npattern = '{pattern}'\nchecklist = 'inspect'"),
        )
        .unwrap();
        let error = handle_review_prepare(&ReviewPrepareArgs::default(), &store)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("custom-check") && error.contains(&pattern),
            "{error}"
        );
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

    /// git_text goes through the host builder: a reviewed repo's
    /// core.fsmonitor hook, which raw git runs, never runs.
    // trace:STORY-3120 | ai:claude
    #[test]
    fn s3fu_git_text_uses_host_git_hardening() {
        let temp = tempfile::tempdir().unwrap();
        let (root, hook) = (temp.path().join("repo"), temp.path().join("fsmonitor"));
        let hit = temp.path().join("hit");
        std::fs::create_dir(&root).unwrap();
        git_text(&root, &["init", "-q"]).unwrap();
        std::fs::write(root.join("a.rs"), "x\n").unwrap();
        git_text(&root, &["add", "a.rs"]).unwrap();
        let src = temp.path().join("src");
        std::fs::write(&src, format!("#!/bin/sh\ntouch '{}'\n", hit.display())).unwrap();
        let installed = Command::new("install")
            .arg("-m755")
            .args([&src, &hook])
            .status();
        assert!(installed.unwrap().success());
        git_text(&root, &["config", "core.fsmonitor", hook.to_str().unwrap()]).unwrap();
        let raw = Command::new("git")
            .current_dir(&root)
            .args(["status", "--porcelain"])
            .output();
        assert!(raw.unwrap().status.success());
        assert!(hit.exists(), "control: raw git runs the fsmonitor hook");
        std::fs::remove_file(&hit).unwrap();
        assert!(git_text(&root, &["status", "--porcelain"])
            .unwrap()
            .contains("a.rs"));
        assert!(
            !hit.exists(),
            "git_text ran the reviewed repo's fsmonitor hook"
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

    // trace:STORY-3120 | ai:claude
    #[test]
    fn s3a2_run_review_refuses_findings_outside_the_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let entry = |path: &str| RawDiffEntry {
            path: path.into(),
            old_path: None,
            kind: FileChangeKind::Modified,
            additions: 1,
            deletions: 0,
            is_binary: false,
            patch_bytes: 48,
        };
        let registry = PlaybookRegistry::new(Vec::new()).unwrap();
        let budget = DEFAULT_BATCH_TOKEN_BUDGET;
        let inventory = classify_with_exclusions(
            &[entry("a.rs"), entry("b.rs")],
            &registry,
            &[],
            budget,
            |_| false,
        );
        let finding = |path: &str| {
            let row =
                serde_json::json!({"path":path,"severity":"high","category":"bug","message":"m"});
            serde_json::json!({ "findings": [row] }).to_string()
        };
        let run = |out: String| {
            let (src, cmd) = (temp.path().join("src"), temp.path().join("runner"));
            std::fs::write(&src, format!("#!/bin/sh\ncat >/dev/null\necho '{out}'\n")).unwrap();
            // install(1): no write fd on the executable for a parallel fork to inherit.
            let installed = Command::new("install")
                .arg("-m755")
                .args([&src, &cmd])
                .status();
            assert!(installed.unwrap().success());
            let env: [(std::ffi::OsString, _); 2] = [
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("AIDA_REVIEW_RUNNER_CMD".into(), cmd.into()),
            ];
            let checkout = tempfile::tempdir().unwrap();
            let call = RunnerCall {
                agent: None,
                checkout: checkout.path(),
                parent_env: &env,
                pass: "finder",
                deadline: std::time::Duration::from_secs(20),
                stdout_cap: STDOUT_CAP,
                stderr_cap: STDERR_CAP,
                tuning: Default::default(),
            };
            let batch = ReviewBatch {
                batch_index: 1,
                label: "b".into(),
                playbook_ids: Vec::new(),
                files: vec!["a.rs".into()],
                estimated_tokens: 1,
            };
            let batches = [batch.clone(), batch];
            review_batches(&call, None, &batches, |_| Ok("p".to_owned()), &inventory)
        };
        assert_eq!(run(finding("a.rs")).unwrap().0.len(), 2);
        // Coverage rows: only `reviewed` counts, and only for the batch's own files.
        let files = |path: &str, status: &str| {
            let row = serde_json::json!({"path":path,"status":status,"reason":"r"});
            serde_json::json!({ "findings": [], "files": [row] }).to_string()
        };
        assert_eq!(run(files("a.rs", "reviewed")).unwrap().1.files.len(), 2);
        assert!(run(files("a.rs", "skipped")).unwrap().1.files.is_empty());
        let err = run(files("b.rs", "reviewed")).unwrap_err().to_string();
        assert!(err.contains("coverage for a file outside it"), "{err}");
        // b.rs is in the inventory but not in this batch.
        let err = run(finding("b.rs")).unwrap_err().to_string();
        assert!(err.contains("finding outside its batch"), "{err}");
        for outside in ["c.rs", "../a.rs", "/etc/passwd", "A.rs"] {
            let err = run(finding(outside)).unwrap_err().to_string();
            assert!(
                err.contains("outside reviewable inventory"),
                "{outside}: {err}"
            );
        }
        // trace:TASK-3320 STORY-3120 | ai:claude
        for cited in [".netrc", ".aws/credentials", "nested/.docker/config.json"] {
            let err = run(finding(cited)).unwrap_err().to_string();
            assert!(err.contains("credential path"), "{cited}: {err}");
        }
        assert!(run("not json".into()).is_err());
    }

    /// Every runner failure is one line (agent mode keeps only the first)
    /// naming the runner, the pass, the batch and the concrete cause.
    // trace:BUG-2775 | ai:claude
    #[test]
    fn bug2775_runner_errors_name_runner_pass_batch_and_cause() {
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let entry = RawDiffEntry {
            path: "a.rs".into(),
            old_path: None,
            kind: FileChangeKind::Modified,
            additions: 1,
            deletions: 0,
            is_binary: false,
            patch_bytes: 48,
        };
        let registry = PlaybookRegistry::new(Vec::new()).unwrap();
        let budget = DEFAULT_BATCH_TOKEN_BUDGET;
        let inventory = classify_with_exclusions(&[entry], &registry, &[], budget, |_| false);
        let batch = ReviewBatch {
            batch_index: 1,
            label: "b".into(),
            playbook_ids: Vec::new(),
            files: vec!["a.rs".into()],
            estimated_tokens: 1,
        };
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        let first_line = |name: &str, body: &str, custom: Option<&str>, cap: usize| {
            let (src, cmd) = (bin.path().join("src"), bin.path().join(name));
            std::fs::write(&src, format!("#!/bin/sh\ncat >/dev/null\n{body}\n")).unwrap();
            let installed = Command::new("install")
                .arg("-m755")
                .args([&src, &cmd])
                .status();
            assert!(installed.unwrap().success());
            let mut env: Vec<(std::ffi::OsString, std::ffi::OsString)> =
                vec![("PATH".into(), path.clone().into())];
            if let Some(runner) = custom {
                env.push(("AIDA_REVIEW_RUNNER_CMD".into(), runner.into()));
            }
            let call = RunnerCall {
                agent: review_vendor("claude").ok(),
                checkout: checkout.path(),
                parent_env: &env,
                pass: "finder",
                deadline: std::time::Duration::from_secs(2),
                stdout_cap: cap,
                stderr_cap: STDERR_CAP,
                tuning: Default::default(),
            };
            let batches = [batch.clone()];
            let res = review_batches(&call, None, &batches, |_| Ok("p".into()), &inventory);
            let err = format!("{:?}", res.unwrap_err());
            err.lines().next().unwrap().to_owned()
        };
        let runner = bin.path().join("custom").display().to_string();
        let exit3 = "echo first >&2; echo auth needed >&2; exit 3";
        let cases = [
            ("claude", "sleep 30", None, "exceeded its 2s deadline"),
            ("claude", "head -c 4096 /dev/zero", None, "cap (64 bytes"),
            (
                "claude",
                exit3,
                None,
                "exit status: 3); stderr: first / auth needed",
            ),
            ("claude", "echo not json", None, "runner output rejected"),
            (
                "custom",
                "exit 0",
                Some("rel/runner"),
                "must be an absolute path",
            ),
            ("custom", "exit 4", Some(runner.as_str()), "exit status: 4"),
        ];
        for (name, body, custom, cause) in cases {
            let line = first_line(name, body, custom, 64);
            let who = custom.map_or("claude", |_| "AIDA_REVIEW_RUNNER_CMD");
            let head = format!("{who} finder pass, review batch 1: ");
            assert!(line.starts_with(&head) && line.contains(cause), "{line}");
        }
    }

    // trace:BUG-2775 | ai:claude
    #[test]
    fn bug2775_run_review_agent_resolves_and_states_the_default() {
        let root = tempfile::tempdir().unwrap();
        let env = |pairs: &[(&str, &str)]| -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
            pairs.iter().map(|(k, v)| (k.into(), v.into())).collect()
        };
        let pick = |flag, e: &[(std::ffi::OsString, std::ffi::OsString)]| {
            run_review_agent(flag, root.path(), e).map(|v| v.map(|v| v.program))
        };
        let custom = env(&[("AIDA_REVIEW_RUNNER_CMD", "/bin/true")]);
        assert_eq!(pick(None, &custom).unwrap(), None);
        assert_eq!(pick(Some("codex"), &custom).unwrap(), Some("codex"));
        let std_dir = root.path().join(".aida");
        std::fs::create_dir_all(&std_dir).unwrap();
        std::fs::write(
            std_dir.join("agents.toml"),
            "[agents]\nvendor = \"codex\"\n",
        )
        .unwrap();
        assert_eq!(pick(None, &env(&[])).unwrap(), Some("codex"));
    }

    /// `--adversarial` end to end: the finder reports two findings, the
    /// challenger refutes the high one and reproduces the low one. The refuted
    /// one still fails `--fail-on high` and is a suppressed SARIF result; a
    /// renamed file's diff carries its old path.
    // trace:STORY-3120 | ai:claude
    #[test]
    fn s4_adversarial_run_suppresses_but_gates_refuted_findings() {
        let temp = tempfile::tempdir().unwrap();
        let (root, bin) = (temp.path().join("repo"), temp.path().join("bin"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir(&bin).unwrap();
        let runner = bin.join("runner");
        let extra = [("AIDA_REVIEW_RUNNER_CMD", runner.to_str())];
        let _guard = crate::test_env::AmbientGuard::hermetic_and(&root, None, &extra);
        let store = Storage::new(root.join("requirements.yaml"));
        git_text(&root, &["init", "-q"]).unwrap();
        std::fs::write(root.join("old.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
        git_text(&root, &["add", "-A"]).unwrap();
        let base = crate::test_git::commit_fixture(&root);
        git_text(&root, &["mv", "old.rs", "a.rs"]).unwrap();
        std::fs::write(root.join("a.rs"), "fn a() {}\nfn b() {}\nfn c() { x }\n").unwrap();
        git_text(&root, &["add", "-A"]).unwrap();
        crate::test_git::commit_fixture(&root);
        let prompt = temp.path().join("finder-prompt");
        let row = |m: &str, sev: &str| {
            format!(r#"{{"path":"a.rs","severity":"{sev}","category":"bug","message":"{m}"}}"#)
        };
        let finder = format!(
            r#"{{"findings":[{},{}],"files":[{{"path":"a.rs","status":"reviewed"}}]}}"#,
            row("real", "low"),
            row("bogus", "high")
        );
        let verdicts = r#"{"verdicts":[{"index":0,"verdict":"reproduced"},{"index":1,"verdict":"refuted","reason":"no"}]}"#;
        let body = format!(
            "#!/bin/sh\nif [ \"$AIDA_REVIEW_PASS\" = challenger ]; then cat >/dev/null; echo '{verdicts}'; \
             else cat >{}; echo '{finder}'; fi\n",
            prompt.display()
        );
        let src = temp.path().join("src");
        std::fs::write(&src, body).unwrap();
        let status = Command::new("install")
            .arg("-m755")
            .args([&src, &runner])
            .status();
        assert!(status.unwrap().success());
        let args = ReviewPrepareArgs {
            from: Some(base),
            run_review: true,
            adversarial: true,
            json: true,
            write: Some("out.json".into()),
            sarif: Some("out.sarif".into()),
            fail_on: Some("high".into()),
            ..Default::default()
        };
        // Fail closed: the only high finding was refuted, and still fails the gate.
        let err = handle_review_prepare(&args, &store)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("1 findings meet or exceed --fail-on high"),
            "{err}"
        );
        let sarif: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("out.sarif")).unwrap()).unwrap();
        let results = sarif["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].get("suppressions").is_none());
        assert_eq!(
            (&results[1]["message"]["text"], &results[1]["suppressions"]),
            (
                &"bogus".into(),
                &serde_json::json!([{"kind":"external","justification":"refuted by challenger: no"}])
            )
        );
        let out: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("out.json")).unwrap()).unwrap();
        let kept = out["anchored_findings"].as_array().unwrap();
        assert_eq!((kept.len(), &kept[0]["message"]), (1, &"real".into()));
        assert_eq!(
            out["adversarial"]["refuted"][0]["finding"]["message"],
            "bogus"
        );
        assert!(out["adversarial"]["note"]
            .as_str()
            .unwrap()
            .contains("AIDA_REVIEW_RUNNER_CMD"));
        let sent = std::fs::read_to_string(prompt).unwrap();
        assert!(
            sent.contains("rename from old.rs") && sent.contains("+fn c() { x }"),
            "{sent}"
        );
    }

    /// End to end: `--run-review` takes its rules from the merge base (the
    /// worktree's uncommitted rules would exclude every file) and a runner that
    /// reports no coverage fails the run instead of passing clean.
    // trace:STORY-3120 | ai:claude
    #[test]
    fn s3a2_run_review_end_to_end_uses_base_rules_and_requires_coverage() {
        let temp = tempfile::tempdir().unwrap();
        let (root, bin) = (temp.path().join("repo"), temp.path().join("bin"));
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::create_dir(&bin).unwrap();
        let runner = bin.join("runner");
        let runner_str = runner.to_str().unwrap();
        let extra = [("AIDA_REVIEW_RUNNER_CMD", Some(runner_str))];
        let _guard = crate::test_env::AmbientGuard::hermetic_and(&root, None, &extra);
        let store = Storage::new(root.join("requirements.yaml"));
        git_text(&root, &["init", "-q"]).unwrap();
        std::fs::write(root.join("a.rs"), "old\n").unwrap();
        git_text(&root, &["add", "-A"]).unwrap();
        let base = crate::test_git::commit_fixture(&root);
        std::fs::write(root.join("a.rs"), "new\n").unwrap();
        git_text(&root, &["add", "-A"]).unwrap();
        crate::test_git::commit_fixture(&root);
        // Uncommitted worktree rules: if read, nothing is reviewable and it passes.
        let rules = root.join(".aida/review-rules.toml");
        std::fs::write(rules, "exclude = [\"*.rs\"]\n").unwrap();
        let marker = temp.path().join("ran");
        let install = |out: &str| {
            let src = temp.path().join("src");
            let body = format!(
                "#!/bin/sh\ncat >/dev/null\ntouch {}\necho '{out}'\n",
                marker.display()
            );
            std::fs::write(&src, body).unwrap();
            let mut cmd = Command::new("install");
            let status = cmd.arg("-m755").args([&src, &runner]).status();
            assert!(status.unwrap().success());
        };
        let args = ReviewPrepareArgs {
            from: Some(base),
            run_review: true,
            fail_on: Some("high".into()),
            json: true,
            write: Some("out.json".into()),
            ..Default::default()
        };
        install(r#"{"findings":[]}"#);
        let err = handle_review_prepare(&args, &store)
            .unwrap_err()
            .to_string();
        assert!(err.contains("coverage verification failed"), "{err}");
        assert!(err.contains("a.rs"), "{err}");
        assert!(
            marker.exists(),
            "runner never ran: worktree rules were used"
        );
        let out: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("out.json")).unwrap()).unwrap();
        assert_eq!(out["inventory"]["reviewable_files"][0]["path"], "a.rs");
        assert_eq!(out["coverage_verification"]["passed"], false);
        install(r#"{"findings":[],"files":[{"path":"a.rs","status":"reviewed"}]}"#);
        handle_review_prepare(&args, &store).unwrap();
    }

    // trace:STORY-3120 | ai:claude
    #[test]
    fn s3a2_run_review_base_and_sanitized_diff() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let marker = root.join("textconv-ran");
        let write = |path: &str, body: &str| std::fs::write(root.join(path), body).unwrap();
        git_text(root, &["init", "-q"]).unwrap();
        let textconv = format!("touch {}; cat", marker.display());
        git_text(root, &["config", "diff.evil.textconv", &textconv]).unwrap();
        std::fs::create_dir(root.join(".aida")).unwrap();
        write(".aida/review-rules.toml", "default_effort = \"medium\"\n");
        write(".gitattributes", "*.rs diff=evil\n");
        write("a.rs", "old\n");
        git_text(root, &["add", "-A"]).unwrap();
        let base = crate::test_git::commit_fixture(root);
        git_text(root, &["branch", "topic"]).unwrap();
        // The target advances with its own rules after the PR branched.
        write(".aida/review-rules.toml", "default_effort = \"low\"\n");
        git_text(root, &["add", "-A"]).unwrap();
        let tip = crate::test_git::commit_fixture(root);
        git_text(root, &["update-ref", "refs/remotes/origin/target", &tip]).unwrap();
        git_text(root, &["checkout", "-q", "topic"]).unwrap();
        write("a.rs", "new\n");
        git_text(root, &["add", "-A"]).unwrap();
        let head = crate::test_git::commit_fixture(root);
        let (reviewed, mb) = pr_comparison(root, "target", &head).unwrap();
        assert_eq!(mb, base);
        let trusted = trusted_review_base(
            root,
            run_review_base("range", Some(&mb)).unwrap(),
            Some(&reviewed),
        );
        assert_eq!(
            trusted.unwrap().rules.default_effort.as_deref(),
            Some("medium")
        );
        // The target tip is not the base: it would read the wrong rules and refuse.
        assert!(trusted_review_base(root, &tip, Some(&reviewed)).is_err());
        // The checkout's HEAD (PR-supplied) never becomes the base.
        for (mode, supplied) in [
            ("workspace", None),
            ("scan", None),
            ("commit", Some(head.as_str())),
        ] {
            assert!(run_review_base(mode, supplied).is_err(), "{mode}");
        }
        let batch = ReviewBatch {
            batch_index: 1,
            label: "b".into(),
            playbook_ids: Vec::new(),
            files: vec!["a.rs".into()],
            estimated_tokens: 1,
        };
        let none = BTreeMap::new();
        let prompt = batch_prompt(root, &format!("{mb}..{head}"), &batch, &none, &[], &[]).unwrap();
        assert!(prompt.contains("-old\n+new\n"), "{prompt}");
        assert!(!marker.exists(), "textconv driver ran");
        let missing = format!("{mb}..{}", "f".repeat(40));
        assert!(batch_prompt(root, &missing, &batch, &none, &[], &[]).is_err());
        for key in ["GIT_DIR", "GIT_CONFIG_GLOBAL", "GIT_INDEX_FILE", "GH_TOKEN"] {
            assert!(!git_env_kept(key.as_ref()), "{key}");
        }
        assert!(git_env_kept("PATH".as_ref()) && git_env_kept("LC_ALL".as_ref()));
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
}
