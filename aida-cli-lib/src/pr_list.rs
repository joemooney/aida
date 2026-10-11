use crate::*;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PrListEntry {
    /// Spec the PR maps to; `None` (JSON `null`) when no title, branch,
    /// body or lease reference was found.
    pub spec_id: Option<String>,
    /// Where `spec_id` came from: `title`, `branch`, `body`, `lease`, or
    /// `none`.
    // trace:BUG-1927 | ai:claude
    pub mapping_source: String,
    pub pr_number: u64,
    pub url: String,
    pub branch: String,
    pub title: String,
    pub status: String,
    pub ci: String,
    pub rebase_needed: bool,
    pub updated_at: String,
}

/// List open PRs/MRs. Every open PR gets a row; `mapped_only` restores the
/// old behaviour of dropping PRs that map to no spec.
// trace:BUG-1927 | ai:claude
pub(crate) fn pr_list_handler(json: bool, mapped_only: bool) -> Result<()> {
    let project_root = find_main_worktree_root()?;
    let forge = crate::forge::resolve_forge_kind(&project_root);
    let lease_scopes: Vec<(String, String)> = list_leases(&project_root)
        .into_iter()
        .map(|l| (l.branch, l.scope))
        .collect();

    let entries = match forge {
        crate::forge::ForgeKind::GitHub => pr_list_github(&project_root, &lease_scopes)?,
        crate::forge::ForgeKind::GitLab => pr_list_gitlab(&project_root, &lease_scopes)?,
        crate::forge::ForgeKind::None => {
            anyhow::bail!("pure-git has no PR concept — `aida list pr` requires GitHub or GitLab")
        }
    };
    let entries = filter_mapped(entries, mapped_only);

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if entries.is_empty() {
        if mapped_only {
            println!("No open PRs with mapped specs found.");
        } else {
            println!("No open PRs found.");
        }
        return Ok(());
    }

    use colored::Colorize;

    println!(
        "{} {} {} {} {} {} {}",
        format!("{:<18}", "Spec ID").bold(),
        format!("{:<8}", "PR").bold(),
        format!("{:<12}", "Status").bold(),
        format!("{:<12}", "CI").bold(),
        format!("{:<15}", "Rebase Needed?").bold(),
        format!("{:<10}", "Updated").bold(),
        "Title".bold()
    );

    for e in entries {
        let rebase_str = if e.rebase_needed {
            format!("{:<15}", "Yes").red()
        } else {
            format!("{:<15}", "No").green()
        };
        let ci_str = match e.ci.as_str() {
            // trace:BUG-1935 | ai:claude — "Passing (N informational red)" is still green.
            ci if ci == "SUCCESS" || ci.starts_with("Passing") => format!("{:<12}", e.ci).green(),
            "Failing" | "FAILURE" => format!("{:<12}", e.ci).red(),
            "Pending" | "PENDING" | "IN_PROGRESS" => format!("{:<12}", e.ci).yellow(),
            _ => format!("{:<12}", e.ci).normal(),
        };
        println!(
            "{} {} {} {} {} {} {}",
            format!("{:<18}", spec_column(&e)).bold(),
            format!("{:<8}", format!("#{}", e.pr_number)),
            format!("{:<12}", e.status),
            ci_str,
            rebase_str,
            format!("{:<10}", e.updated_at),
            e.title
        );
    }

    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    number: u64,
    url: String,
    head_ref_name: String,
    title: String,
    #[serde(default)]
    body: String,
    mergeable: String,
    review_decision: String,
    updated_at: String,
    status_check_rollup: Option<Vec<serde_json::Value>>,
}

fn pr_list_github(
    project_root: &std::path::Path,
    lease_scopes: &[(String, String)],
) -> Result<Vec<PrListEntry>> {
    let out = std::process::Command::new("gh")
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--json",
            "number,url,headRefName,title,body,mergeable,reviewDecision,updatedAt,statusCheckRollup",
        ])
        .output()
        .context("running gh pr list")?;

    anyhow::ensure!(out.status.success(), "gh pr list failed");

    let prs: Vec<GhPr> = serde_json::from_slice(&out.stdout)?;
    let ci_cfg = crate::ci_gate::read_ci_gate_config(project_root); // trace:BUG-1935 | ai:claude
    Ok(github_entries(prs, lease_scopes, &ci_cfg))
}

/// Build exactly one row per PR: every mapped spec ID comma-joined, or a
/// single unmapped row. CI is classified against the `[ci]` informational
/// allow-list.
// trace:BUG-1927 | ai:claude
// trace:BUG-1935 | ai:claude
fn github_entries(
    prs: Vec<GhPr>,
    lease_scopes: &[(String, String)],
    ci_cfg: &crate::ci_gate::CiGateConfig,
) -> Vec<PrListEntry> {
    let mut entries = Vec::new();

    for pr in prs {
        let (specs, source) = map_pr_specs(&pr.title, &pr.head_ref_name, &pr.body, lease_scopes);
        let status = if pr.review_decision.is_empty() {
            "Open".to_string()
        } else {
            pr.review_decision.clone()
        };
        let rebase_needed = pr.mergeable == "CONFLICTING" || pr.mergeable == "UNKNOWN"; // GH often says UNKNOWN if it hasn't checked recently, but CONFLICTING is the main one. We can also check if behind? gh pr list doesn't give behind count easily without fields that cause n+1. We'll stick to CONFLICTING.

        // trace:BUG-1935 | ai:claude
        let ci_status = match &pr.status_check_rollup {
            Some(rollup) => github_rollup_ci_status(rollup, ci_cfg),
            None => "Unknown".to_string(),
        };

        for spec_id in row_spec_ids(specs) {
            entries.push(PrListEntry {
                spec_id,
                mapping_source: source.as_str().to_string(),
                pr_number: pr.number,
                url: pr.url.clone(),
                branch: pr.head_ref_name.clone(),
                title: pr.title.clone(),
                status: status.clone(),
                ci: ci_status.clone(),
                rebase_needed,
                updated_at: parse_and_format_date(&pr.updated_at),
            });
        }
    }

    collapse_by_pr(entries) // trace:BUG-1935 | ai:claude
}

fn pr_list_gitlab(
    project_root: &std::path::Path,
    lease_scopes: &[(String, String)],
) -> Result<Vec<PrListEntry>> {
    let out = std::process::Command::new("glab")
        .current_dir(project_root)
        .args([
            "api",
            "-X",
            "GET",
            "projects/:id/merge_requests?state=opened",
        ])
        .output()
        .context("running glab api")?;

    anyhow::ensure!(out.status.success(), "glab api failed");

    #[derive(Deserialize)]
    struct GlabMr {
        iid: u64,
        web_url: String,
        source_branch: String,
        title: String,
        #[serde(default)]
        description: Option<String>,
        merge_status: String,
        updated_at: String,
        head_pipeline: Option<serde_json::Value>,
    }

    let mrs: Vec<GlabMr> = serde_json::from_slice(&out.stdout)?;
    let mut entries = Vec::new();

    for mr in mrs {
        // trace:BUG-1927 | ai:claude
        let (specs, source) = map_pr_specs(
            &mr.title,
            &mr.source_branch,
            mr.description.as_deref().unwrap_or(""),
            lease_scopes,
        );
        let rebase_needed = mr.merge_status != "can_be_merged";

        let mut ci_status = "None".to_string();
        if let Some(pipeline) = &mr.head_pipeline {
            if let Some(status) = pipeline.get("status").and_then(|s| s.as_str()) {
                ci_status = match status {
                    "success" => "Passing".to_string(),
                    "failed" => "Failing".to_string(),
                    "running" | "pending" | "created" => "Pending".to_string(),
                    _ => "Unknown".to_string(),
                };
            }
        }

        for spec_id in row_spec_ids(specs) {
            entries.push(PrListEntry {
                spec_id,
                mapping_source: source.as_str().to_string(),
                pr_number: mr.iid,
                url: mr.web_url.clone(),
                branch: mr.source_branch.clone(),
                title: mr.title.clone(),
                status: "Open".to_string(), // Could fetch approvals but that's N+1 calls
                ci: ci_status.clone(),
                rebase_needed,
                updated_at: parse_and_format_date(&mr.updated_at),
            });
        }
    }

    Ok(collapse_by_pr(entries)) // trace:BUG-1935 | ai:claude
}

/// BUG-1935: classify a GitHub `statusCheckRollup` through the same
/// informational-aware matcher `aida pr ship` uses, so "Failing" means a red
/// check that gates the merge. Branch-protection required checks are not
/// fetched here (one extra call per PR); with an empty required set the
/// `[ci]` informational allow-list alone decides, exactly as in `ci_gate`.
// trace:BUG-1935 | ai:claude
fn github_rollup_ci_status(
    rollup: &[serde_json::Value],
    cfg: &crate::ci_gate::CiGateConfig,
) -> String {
    if rollup.is_empty() {
        return "None".to_string();
    }
    let rows: Vec<crate::ci_gate::CheckRow> = rollup.iter().map(rollup_check_row).collect();
    let refined = crate::ci_gate::classify_red(&rows, &[], cfg, false, false);
    if refined.is_real() {
        "Failing".to_string()
    } else if refined.has_pending() {
        "Pending".to_string()
    } else if refined.ignored_informational.is_empty() {
        "Passing".to_string()
    } else {
        format!(
            "Passing ({} informational red)",
            refined.ignored_informational.len()
        )
    }
}

/// Map one rollup entry (a `CheckRun` or a commit `StatusContext`) onto the
/// `gh pr checks` bucket vocabulary `ci_gate` classifies. Only the outcomes the
/// listing always treated as red are red; a cancelled run is not.
// trace:BUG-1935 | ai:claude
fn rollup_check_row(check: &serde_json::Value) -> crate::ci_gate::CheckRow {
    let field = |key: &str| {
        check
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let name = if check.get("context").is_some() {
        field("context")
    } else {
        field("name")
    };
    let status = field("status");
    let conclusion = field("conclusion");
    let state = field("state");
    let bucket = if matches!(
        conclusion.as_str(),
        "FAILURE" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE"
    ) || matches!(state.as_str(), "FAILURE" | "ERROR")
    {
        "fail"
    } else if matches!(
        status.as_str(),
        "IN_PROGRESS" | "QUEUED" | "PENDING" | "WAITING" | "REQUESTED"
    ) || matches!(state.as_str(), "PENDING" | "EXPECTED")
    {
        "pending"
    } else {
        "pass"
    };
    crate::ci_gate::CheckRow {
        name,
        workflow: field("workflowName"),
        bucket: bucket.to_string(),
        run_id: None,
    }
}

/// BUG-1935: one row per PR. A PR whose title names several specs used to be
/// listed once per spec; merge rows sharing a PR number, joining the spec IDs
/// with `,` in first-seen order (unmapped `None` IDs are skipped, so a PR
/// stays `None` only when no row carried an ID).
// trace:BUG-1935 | ai:claude
fn collapse_by_pr(entries: Vec<PrListEntry>) -> Vec<PrListEntry> {
    let mut out: Vec<PrListEntry> = Vec::new();
    for entry in entries {
        match out.iter_mut().find(|e| e.pr_number == entry.pr_number) {
            Some(existing) => {
                let Some(id) = entry.spec_id else {
                    continue;
                };
                match &mut existing.spec_id {
                    None => existing.spec_id = Some(id),
                    Some(joined) => {
                        if !joined.split(',').any(|s| s == id) {
                            joined.push(',');
                            joined.push_str(&id);
                        }
                    }
                }
            }
            None => out.push(entry),
        }
    }
    out
}

fn extract_specs(title: &str, branch: &str) -> Vec<String> {
    // Regex or simple parsing. In AIDA, specs are usually like (TASK-123) or branch is pr-123-TASK-123 etc.
    // Let's use a simple regex via regex crate if available, or just string matching.
    let mut specs = Vec::new();
    let re = regex::Regex::new(r"(?i)\b([A-Z]+-\d+)\b").unwrap();

    // Check title trailer usually `(TASK-123 BUG-456)`
    for cap in re.captures_iter(title) {
        let s = cap[1].to_uppercase();
        if !specs.contains(&s) {
            specs.push(s);
        }
    }

    if specs.is_empty() {
        for cap in re.captures_iter(branch) {
            let s = cap[1].to_uppercase();
            if !specs.contains(&s) {
                specs.push(s);
            }
        }
    }
    specs
}

/// Where a PR's spec mapping came from, in lookup order.
// trace:BUG-1927 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MappingSource {
    Title,
    Branch,
    Body,
    Lease,
    None,
}

impl MappingSource {
    fn as_str(self) -> &'static str {
        match self {
            MappingSource::Title => "title",
            MappingSource::Branch => "branch",
            MappingSource::Body => "body",
            MappingSource::Lease => "lease",
            MappingSource::None => "none",
        }
    }
}

/// Map a PR to its spec IDs: title, then branch (every ID found), then the
/// first ID named in the body, then the scope of a lease riding the PR's
/// branch. An empty list with `MappingSource::None` means "unmapped" — the
/// PR still gets a row.
// trace:BUG-1927 | ai:claude
fn map_pr_specs(
    title: &str,
    branch: &str,
    body: &str,
    lease_scopes: &[(String, String)],
) -> (Vec<String>, MappingSource) {
    let from_title = extract_specs(title, "");
    if !from_title.is_empty() {
        return (from_title, MappingSource::Title);
    }
    let from_branch = extract_specs("", branch);
    if !from_branch.is_empty() {
        return (from_branch, MappingSource::Branch);
    }
    if let Some(id) = first_strict_spec_id(body) {
        return (vec![id], MappingSource::Body);
    }
    let lease_id = lease_scopes
        .iter()
        .filter(|(lease_branch, _)| !branch.is_empty() && lease_branch == branch)
        .find_map(|(_, scope)| first_strict_spec_id(scope));
    if let Some(id) = lease_id {
        return (vec![id], MappingSource::Lease);
    }
    (Vec::new(), MappingSource::None)
}

/// First uppercase spec-shaped ID in free text (PR body, lease scope).
/// Case-sensitive so prose like "utf-8" never maps, and forge refs or
/// common non-spec tokens are skipped.
// trace:BUG-1927 | ai:claude
fn first_strict_spec_id(text: &str) -> Option<String> {
    let re =
        regex::Regex::new(r"\b([A-Z][A-Z0-9]*(?:-[A-Z0-9]+)*-\d+)\b").expect("valid spec id regex");
    let found = re
        .captures_iter(text)
        .map(|cap| cap[1].to_string())
        .find(|id| {
            let prefix = id.split('-').next().unwrap_or("");
            !matches!(
                prefix,
                "PR" | "MR" | "GH" | "GL" | "UTF" | "SHA" | "ISO" | "RFC" | "CVE"
            )
        });
    found
}

/// One `Some(id)` per mapped spec, or a single `None` for an unmapped PR.
// trace:BUG-1927 | ai:claude
fn row_spec_ids(specs: Vec<String>) -> Vec<Option<String>> {
    if specs.is_empty() {
        vec![None]
    } else {
        specs.into_iter().map(Some).collect()
    }
}

/// `--mapped-only` drops the rows that map to no spec.
// trace:BUG-1927 | ai:claude
fn filter_mapped(entries: Vec<PrListEntry>, mapped_only: bool) -> Vec<PrListEntry> {
    if mapped_only {
        entries
            .into_iter()
            .filter(|e| e.spec_id.is_some())
            .collect()
    } else {
        entries
    }
}

/// Spec ID column text: `—` when unmapped, `body:ID` / `lease:ID` when the
/// mapping came from a secondary source, the bare ID otherwise.
// trace:BUG-1927 | ai:claude
fn spec_column(e: &PrListEntry) -> String {
    match (&e.spec_id, e.mapping_source.as_str()) {
        (None, _) => "—".to_string(),
        (Some(id), "body" | "lease") => format!("{}:{}", e.mapping_source, id),
        (Some(id), _) => id.clone(),
    }
}

fn parse_and_format_date(iso: &str) -> String {
    if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
        let now = Utc::now();
        let diff = now.signed_duration_since(dt.with_timezone(&Utc));
        if diff.num_days() > 0 {
            format!("{}d ago", diff.num_days())
        } else if diff.num_hours() > 0 {
            format!("{}h ago", diff.num_hours())
        } else {
            format!("{}m ago", diff.num_minutes())
        }
    } else {
        iso.to_string()
    }
}

#[cfg(test)]
mod tests {
    // trace:BUG-1927 | ai:claude
    // trace:BUG-1935 | ai:claude
    use super::*;
    use crate::ci_gate::CiGateConfig;

    fn gh_prs(json: &str) -> Vec<GhPr> {
        serde_json::from_str(json).expect("valid gh pr list fixture")
    }

    fn gh_pr_json(number: u64, title: &str, branch: &str, body: &str) -> String {
        serde_json::json!({
            "number": number,
            "url": format!("https://github.com/o/r/pull/{number}"),
            "headRefName": branch,
            "title": title,
            "body": body,
            "mergeable": "CONFLICTING",
            "reviewDecision": "",
            "updatedAt": "2026-10-10T00:00:00Z",
            "statusCheckRollup": [
                {"conclusion": "FAILURE", "status": "COMPLETED"}
            ]
        })
        .to_string()
    }

    #[test]
    fn pr_list_keeps_unmapped_pr() {
        let fixture = format!(
            "[{}]",
            gh_pr_json(
                2480,
                "refactor: split lib.rs into modules",
                "split-stack",
                ""
            )
        );
        let entries = github_entries(gh_prs(&fixture), &[], &CiGateConfig::default());
        let rows: Vec<&PrListEntry> = entries.iter().filter(|e| e.pr_number == 2480).collect();
        assert_eq!(rows.len(), 1, "unmapped PR must appear exactly once");
        let row = rows[0];
        assert_eq!(row.spec_id, None);
        assert_eq!(row.mapping_source, "none");
        assert_eq!(row.ci, "Failing");
        assert!(row.rebase_needed);
        assert_eq!(row.status, "Open");
        assert_eq!(spec_column(row), "—");
    }

    #[test]
    fn pr_list_maps_body_only_reference() {
        let fixture = format!(
            "[{}]",
            gh_pr_json(
                2480,
                "refactor: split lib.rs into modules",
                "split-stack",
                "Slice of TASK-1538, uses UTF-8 output.\n"
            )
        );
        let entries = github_entries(gh_prs(&fixture), &[], &CiGateConfig::default());
        let rows: Vec<&PrListEntry> = entries.iter().filter(|e| e.pr_number == 2480).collect();
        assert_eq!(rows.len(), 1, "body-mapped PR must appear exactly once");
        assert_eq!(rows[0].spec_id.as_deref(), Some("TASK-1538"));
        assert_eq!(rows[0].mapping_source, "body");
        assert_eq!(spec_column(rows[0]), "body:TASK-1538");
    }

    #[test]
    fn pr_list_title_mapping_wins_over_body() {
        let fixture = format!(
            "[{}]",
            gh_pr_json(
                7,
                "[AI:claude] fix(x): y (BUG-1927)",
                "bug-1927-work",
                "see TASK-1"
            )
        );
        let entries = github_entries(gh_prs(&fixture), &[], &CiGateConfig::default());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].spec_id.as_deref(), Some("BUG-1927"));
        assert_eq!(entries[0].mapping_source, "title");
        assert_eq!(spec_column(&entries[0]), "BUG-1927");
    }

    #[test]
    fn pr_list_maps_through_lease_on_branch() {
        let leases = vec![
            ("other".to_string(), "TASK-9".to_string()),
            ("split-stack".to_string(), "STORY-1488".to_string()),
        ];
        let (specs, source) = map_pr_specs("refactor: split", "split-stack", "", &leases);
        assert_eq!(specs, vec!["STORY-1488".to_string()]);
        assert_eq!(source, MappingSource::Lease);
    }

    #[test]
    fn pr_list_body_ignores_lowercase_and_non_spec_tokens() {
        assert_eq!(
            first_strict_spec_id("uses utf-8 and SHA-256, see PR-12"),
            None
        );
        assert_eq!(
            first_strict_spec_id("STORY-1488/TASK-1538"),
            Some("STORY-1488".to_string())
        );
    }

    #[test]
    fn pr_list_mapped_only_drops_unmapped_rows() {
        let fixture = format!(
            "[{},{}]",
            gh_pr_json(1, "refactor: no id", "no-id", ""),
            gh_pr_json(2, "fix: thing (BUG-5)", "bug-5", "")
        );
        let all = github_entries(gh_prs(&fixture), &[], &CiGateConfig::default());
        assert_eq!(all.len(), 2);
        let mapped = filter_mapped(all, true);
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].pr_number, 2);
    }

    #[test]
    fn pr_list_json_unmapped_spec_id_is_null() {
        let fixture = format!("[{}]", gh_pr_json(3, "refactor: no id", "no-id", ""));
        let entries = github_entries(gh_prs(&fixture), &[], &CiGateConfig::default());
        let value = serde_json::to_value(&entries).expect("serialize entries");
        assert!(value[0]["spec_id"].is_null());
        assert_eq!(value[0]["mapping_source"], "none");
    }

    fn check_run(name: &str, workflow: &str, status: &str, conclusion: &str) -> serde_json::Value {
        serde_json::json!({
            "__typename": "CheckRun",
            "name": name,
            "workflowName": workflow,
            "status": status,
            "conclusion": conclusion,
        })
    }

    fn entry(spec_id: &str, pr_number: u64) -> PrListEntry {
        PrListEntry {
            spec_id: Some(spec_id.to_string()),
            mapping_source: "title".to_string(),
            pr_number,
            url: format!("https://example.invalid/pull/{pr_number}"),
            branch: format!("branch-{pr_number}"),
            title: format!("title {pr_number}"),
            status: "Open".to_string(),
            ci: "Passing".to_string(),
            rebase_needed: false,
            updated_at: "1h ago".to_string(),
        }
    }

    #[test]
    fn ci_column_ignores_informational_checks() {
        let rollup = vec![
            check_run("Build (ubuntu-latest)", "CI", "COMPLETED", "SUCCESS"),
            check_run("merge-hold-gate", "merge-hold-gate", "COMPLETED", "SUCCESS"),
            check_run(
                "Build (windows-latest)",
                "Cross-platform (nightly)",
                "COMPLETED",
                "FAILURE",
            ),
        ];
        assert_eq!(
            github_rollup_ci_status(&rollup, &CiGateConfig::default()),
            "Passing (1 informational red)"
        );
        // A configured informational check name is honoured the same way.
        let cfg =
            crate::ci_gate::parse_ci_gate_config("[ci]\ninformational_checks = [\"nightly-*\"]\n");
        let rollup = vec![
            check_run("Build (ubuntu-latest)", "CI", "COMPLETED", "SUCCESS"),
            check_run("nightly-soak", "Nightly", "COMPLETED", "TIMED_OUT"),
        ];
        assert_eq!(
            github_rollup_ci_status(&rollup, &cfg),
            "Passing (1 informational red)"
        );
    }

    #[test]
    fn ci_column_fails_on_required_red() {
        let rollup = vec![
            check_run("Build (ubuntu-latest)", "CI", "COMPLETED", "FAILURE"),
            check_run(
                "Build (windows-latest)",
                "Cross-platform (nightly)",
                "COMPLETED",
                "FAILURE",
            ),
        ];
        assert_eq!(
            github_rollup_ci_status(&rollup, &CiGateConfig::default()),
            "Failing"
        );
        // A red commit status context (no workflow) is a real failure too.
        let rollup = vec![serde_json::json!({
            "__typename": "StatusContext",
            "context": "ci/external",
            "state": "ERROR",
        })];
        assert_eq!(
            github_rollup_ci_status(&rollup, &CiGateConfig::default()),
            "Failing"
        );
    }

    #[test]
    fn ci_column_pending_ignores_informational_pending() {
        let cfg = CiGateConfig::default();
        let rollup = vec![
            check_run("Build (ubuntu-latest)", "CI", "COMPLETED", "SUCCESS"),
            check_run(
                "Build (macos-latest)",
                "Cross-platform (nightly)",
                "IN_PROGRESS",
                "",
            ),
        ];
        assert_eq!(github_rollup_ci_status(&rollup, &cfg), "Passing");
        let rollup = vec![check_run("Build (ubuntu-latest)", "CI", "QUEUED", "")];
        assert_eq!(github_rollup_ci_status(&rollup, &cfg), "Pending");
        assert_eq!(github_rollup_ci_status(&[], &cfg), "None");
    }

    #[test]
    fn multi_spec_pr_is_one_row() {
        let specs = extract_specs("feat(x): retire legs (ADR-72) (TASK-1723)", "branch");
        let entries: Vec<PrListEntry> = specs
            .iter()
            .map(|s| entry(s, 2487))
            .chain(std::iter::once(entry("BUG-1", 2483)))
            .collect();
        let rows = collapse_by_pr(entries);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pr_number, 2487);
        assert_eq!(rows[0].spec_id.as_deref(), Some("ADR-72,TASK-1723"));
        assert_eq!(rows[1].pr_number, 2483);
        assert_eq!(rows[1].spec_id.as_deref(), Some("BUG-1"));
        // JSON shape: one object per PR, spec_id carries every ID comma-joined.
        let json = serde_json::to_value(&rows).unwrap();
        assert_eq!(json[0]["spec_id"], "ADR-72,TASK-1723");
        assert_eq!(json.as_array().unwrap().len(), 2);
    }

    #[test]
    fn github_entries_lists_multi_spec_pr_once_beside_unmapped() {
        // trace:BUG-1935 | ai:claude — collapse composes with BUG-1927 mapping.
        let fixture = format!(
            "[{},{}]",
            gh_pr_json(2487, "feat(x): retire legs (ADR-72) (TASK-1723)", "b", ""),
            gh_pr_json(2480, "refactor: split lib.rs", "split-stack", "")
        );
        let rows = github_entries(gh_prs(&fixture), &[], &CiGateConfig::default());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pr_number, 2487);
        assert_eq!(rows[0].spec_id.as_deref(), Some("ADR-72,TASK-1723"));
        assert_eq!(rows[0].mapping_source, "title");
        assert_eq!(rows[1].pr_number, 2480);
        assert_eq!(rows[1].spec_id, None);
        assert_eq!(rows[1].mapping_source, "none");
        let mapped = filter_mapped(rows, true);
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].pr_number, 2487);
    }
}
