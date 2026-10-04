// trace:BUG-1800 | ai:antigravity
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn handle_dismiss(path: Option<&str>, category: Option<&str>) -> Result<()> {
    let project_root = crate::find_project_root()?;

    if path.is_none() && category.is_none() {
        bail!("Must specify either a <path> or --category <category> to dismiss");
    }
    if let Some(c) = category {
        if c != "batched-undecidable" {
            bail!("Only --category batched-undecidable is supported currently");
        }
    }

    // Identify worktrees to dismiss
    let mut targets: Vec<PathBuf> = Vec::new();

    if let Some(p) = path {
        let full = project_root.join(p);
        let canon = full.canonicalize().unwrap_or(full);
        targets.push(canon);
    } else if let Some(_) = category {
        // Run the scan
        let scan = crate::doctor_cmd::scan_merged_agent_worktrees(&project_root);
        for finding in scan {
            if finding.action.contains("batched content is undecidable")
                || finding.summary.contains("batched content is undecidable")
            {
                targets.push(PathBuf::from(finding.id));
            }
        }
    }

    if targets.is_empty() {
        eprintln!("No worktrees matched for dismissal.");
        return Ok(());
    }

    let actor = std::env::var("USER").unwrap_or_else(|_| "operator".to_string());
    let now = chrono::Utc::now().to_rfc3339();

    for target in targets {
        eprintln!("Dismissing worktree: {}", target.display());
        // 1. Find branch tip SHA
        // We can get branch from git worktree list
        let wt_list = Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args(["worktree", "list", "--porcelain"])
            .output()?;
        let porcelain = String::from_utf8_lossy(&wt_list.stdout);
        let mut branch = None;
        let mut head = None;

        let mut current_wt = None;
        for line in porcelain.lines() {
            if let Some(wt) = line.strip_prefix("worktree ") {
                current_wt = Some(PathBuf::from(wt));
            } else if let Some(h) = line.strip_prefix("HEAD ") {
                if current_wt.as_ref() == Some(&target) {
                    head = Some(h.to_string());
                }
            } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                if current_wt.as_ref() == Some(&target) {
                    branch = Some(b.to_string());
                }
            }
        }

        let Some(branch_name) = branch else {
            eprintln!("  Skipping: not on a local branch (detached or store)");
            continue;
        };
        let Some(sha) = head else {
            eprintln!("  Skipping: no HEAD found");
            continue;
        };

        // Archive the branch tip
        let archive_ref = format!("refs/archive/{}", branch_name);
        Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args(["update-ref", &archive_ref, &sha])
            .output()
            .context("Failed to archive branch tip")?;

        eprintln!("  Archived tip {} to {}", sha, archive_ref);

        // Remove worktree + branch
        Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args(["worktree", "remove", "--force", target.to_str().unwrap()])
            .output()
            .context("Failed to remove worktree")?;

        Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args(["branch", "-D", &branch_name])
            .output()
            .context("Failed to delete branch")?;

        // Ledger who decided
        let ledger_line = format!(
            "{{\"timestamp\":\"{}\",\"actor\":\"{}\",\"action\":\"worktree_dismiss\",\"worktree\":\"{}\",\"branch\":\"{}\",\"sha\":\"{}\",\"archive_ref\":\"{}\"}}",
            now, actor, target.display(), branch_name, sha, archive_ref
        );
        let ledger_path = project_root.join(".aida/discipline/worktree_dismiss_ledger.jsonl");
        if let Some(parent) = ledger_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&ledger_path)
        {
            let _ = writeln!(f, "{}", ledger_line);
        }
    }

    Ok(())
}
