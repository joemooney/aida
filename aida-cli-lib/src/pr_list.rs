use crate::*;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PrListEntry {
    pub spec_id: String,
    pub pr_number: u64,
    pub url: String,
    pub branch: String,
    pub title: String,
    pub status: String,
    pub ci: String,
    pub rebase_needed: bool,
    pub updated_at: String,
}

pub(crate) fn pr_list_handler(json: bool) -> Result<()> {
    let project_root = find_main_worktree_root()?;
    let forge = crate::forge::resolve_forge_kind(&project_root);

    let entries = match forge {
        crate::forge::ForgeKind::GitHub => pr_list_github(&project_root)?,
        crate::forge::ForgeKind::GitLab => pr_list_gitlab(&project_root)?,
        crate::forge::ForgeKind::None => {
            anyhow::bail!("pure-git has no PR concept — `aida list pr` requires GitHub or GitLab")
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("No open PRs with mapped specs found.");
        return Ok(());
    }

    use colored::Colorize;

    println!(
        "{} {} {} {} {} {} {}",
        format!("{:<15}", "Spec ID").bold(),
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
            "Passing" | "SUCCESS" => format!("{:<12}", e.ci).green(),
            "Failing" | "FAILURE" => format!("{:<12}", e.ci).red(),
            "Pending" | "PENDING" | "IN_PROGRESS" => format!("{:<12}", e.ci).yellow(),
            _ => format!("{:<12}", e.ci).normal(),
        };
        println!(
            "{} {} {} {} {} {} {}",
            format!("{:<15}", e.spec_id).bold(),
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

fn pr_list_github(project_root: &std::path::Path) -> Result<Vec<PrListEntry>> {
    let out = std::process::Command::new("gh")
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--json",
            "number,url,headRefName,title,mergeable,reviewDecision,updatedAt,statusCheckRollup",
        ])
        .output()
        .context("running gh pr list")?;

    anyhow::ensure!(out.status.success(), "gh pr list failed");

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GhPr {
        number: u64,
        url: String,
        head_ref_name: String,
        title: String,
        mergeable: String,
        review_decision: String,
        updated_at: String,
        status_check_rollup: Option<Vec<serde_json::Value>>,
    }

    let prs: Vec<GhPr> = serde_json::from_slice(&out.stdout)?;
    let mut entries = Vec::new();

    for pr in prs {
        let specs = extract_specs(&pr.title, &pr.head_ref_name);
        let status = if pr.review_decision.is_empty() {
            "Open".to_string()
        } else {
            pr.review_decision.clone()
        };
        let rebase_needed = pr.mergeable == "CONFLICTING" || pr.mergeable == "UNKNOWN"; // GH often says UNKNOWN if it hasn't checked recently, but CONFLICTING is the main one. We can also check if behind? gh pr list doesn't give behind count easily without fields that cause n+1. We'll stick to CONFLICTING.

        let mut ci_status = "Unknown".to_string();
        if let Some(rollup) = &pr.status_check_rollup {
            if rollup.is_empty() {
                ci_status = "None".to_string();
            } else {
                let mut has_failure = false;
                let mut has_pending = false;
                for check in rollup {
                    if let Some(conclusion) = check.get("conclusion").and_then(|c| c.as_str()) {
                        if conclusion == "FAILURE"
                            || conclusion == "TIMED_OUT"
                            || conclusion == "ACTION_REQUIRED"
                        {
                            has_failure = true;
                        }
                    }
                    if let Some(status) = check.get("status").and_then(|s| s.as_str()) {
                        if status == "IN_PROGRESS" || status == "QUEUED" {
                            has_pending = true;
                        }
                    }
                }
                if has_failure {
                    ci_status = "Failing".to_string();
                } else if has_pending {
                    ci_status = "Pending".to_string();
                } else {
                    ci_status = "Passing".to_string();
                }
            }
        }

        for spec in specs {
            entries.push(PrListEntry {
                spec_id: spec,
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

    Ok(entries)
}

fn pr_list_gitlab(project_root: &std::path::Path) -> Result<Vec<PrListEntry>> {
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
        merge_status: String,
        updated_at: String,
    }

    let mrs: Vec<GlabMr> = serde_json::from_slice(&out.stdout)?;
    let mut entries = Vec::new();

    for mr in mrs {
        let specs = extract_specs(&mr.title, &mr.source_branch);
        let rebase_needed = mr.merge_status != "can_be_merged";

        for spec in specs {
            entries.push(PrListEntry {
                spec_id: spec,
                pr_number: mr.iid,
                url: mr.web_url.clone(),
                branch: mr.source_branch.clone(),
                title: mr.title.clone(),
                status: "Open".to_string(), // Could fetch approvals but that's N+1 calls
                ci: "Unknown".to_string(),
                rebase_needed,
                updated_at: parse_and_format_date(&mr.updated_at),
            });
        }
    }

    Ok(entries)
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
