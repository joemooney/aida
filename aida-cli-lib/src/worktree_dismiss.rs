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
        // TASK-2053: a seat clone is not in `git worktree list` and host git
        // never runs in it; it has its own teardown.
        // trace:TASK-2053 | ai:claude
        if crate::seat_clone::seat_clone_root(&target).as_deref() == Some(target.as_path())
            || crate::seat_clone::is_seat_clone_location(&project_root, &target)
        {
            dismiss_seat_clone(&project_root, &target, &actor, &now);
            continue;
        }
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

        // A live process inside the tree (cwd, open file, running program)
        // keeps it: skip before archiving or deleting anything.
        // trace:BUG-1947 | ai:claude
        if let Err(in_use) = aida_core::worktree_holders::check_worktree_removal(&target) {
            eprintln!(
                "  Skipping: {}. Stop those processes, then re-run.",
                in_use.summary()
            );
            continue;
        }

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

        // trace:BUG-2211 | ai:claude
        aida_core::git_ops::output_healing_config_lock(
            &project_root,
            || {
                let mut cmd = Command::new("git");
                cmd.arg("-C").arg(&project_root);
                cmd
            },
            &["branch", "-D", &branch_name],
        )
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

/// Dismiss a contained seat's private clone: salvage whatever origin does not
/// have, remove the clone, its cargo target directory and its base pin, then
/// release any `clone` lease naming it. There is no archive ref: the clone's
/// commits are not in main, so the salvage patch is the archive.
// trace:TASK-2053 | ai:claude
fn dismiss_seat_clone(project_root: &Path, target: &Path, actor: &str, now: &str) {
    eprintln!("Dismissing seat clone: {}", target.display());
    if let Err(in_use) = aida_core::worktree_holders::check_worktree_removal(target) {
        eprintln!(
            "  Skipping: {}. Stop those processes, then re-run.",
            in_use.summary()
        );
        return;
    }
    let salvage = match crate::seat_clone::destroy(
        project_root,
        target,
        crate::seat_clone::DestroyMode::SalvageUnshipped,
        None,
    ) {
        Ok(crate::seat_clone::Destroyed::Removed { salvage }) => salvage,
        Ok(crate::seat_clone::Destroyed::Kept { reason, .. }) => {
            eprintln!("  Skipping: {reason}");
            return;
        }
        Err(error) => {
            eprintln!("  Skipping: {error:#}");
            return;
        }
    };
    if let Some(path) = &salvage {
        eprintln!("  Salvaged unshipped work to {}", path.display());
    }
    eprintln!("  Removed the clone and released its base pin");
    let leases: Vec<_> = crate::list_leases(project_root)
        .into_iter()
        .filter(|lease| {
            lease.tree_kind == crate::seat_clone::TreeKind::Clone
                && lease.worktree_path.file_name() == target.file_name()
        })
        .collect();
    let branch = leases
        .first()
        .map(|lease| lease.branch.clone())
        .unwrap_or_default();
    for lease in &leases {
        crate::session_reap::release_lease_files(project_root, &lease.id);
        eprintln!("  Released lease {}", lease.id);
    }
    let ledger_line = serde_json::json!({
        "timestamp": now,
        "actor": actor,
        "action": "seat_clone_dismiss",
        "worktree": target.display().to_string(),
        "branch": branch,
        "salvage": salvage.map(|p| p.display().to_string()),
    });
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
        let _ = writeln!(f, "{ledger_line}");
    }
}
