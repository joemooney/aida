//! session prune/show/handoff/manifest and queue-work launch helpers (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

pub(crate) fn session_prune_orphans(dry_run: bool, yes: bool) -> Result<()> {
    let home = crate::home_dir().context("HOME not set; cannot locate Claude project dir")?;
    let projects = home.join(".claude/projects");
    if !projects.is_dir() {
        println!("(no ~/.claude/projects dir found)");
        return Ok(());
    }

    struct Orphan {
        path: std::path::PathBuf,
        decoded_cwd: String,
        jsonl_count: usize,
    }

    let mut orphans: Vec<Orphan> = Vec::new();
    let mut active = 0usize;
    let mut errored = 0usize;
    for entry in std::fs::read_dir(&projects)?.filter_map(|e| e.ok()) {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        // Read one jsonl event to recover the authoritative cwd. Skip if
        // the dir has no jsonls — it's empty cruft, treat as orphan.
        let mut found_cwd: Option<String> = None;
        let mut jsonl_count = 0;
        if let Ok(read) = std::fs::read_dir(&dir) {
            for jsonl in read.filter_map(|e| e.ok()) {
                if jsonl.path().extension().and_then(|s| s.to_str()) != Some("jsonl") {
                    continue;
                }
                jsonl_count += 1;
                if found_cwd.is_none() {
                    if let Ok(text) = std::fs::read_to_string(jsonl.path()) {
                        for line in text.lines().take(20) {
                            if let Some(idx) = line.find("\"cwd\":\"") {
                                let after = &line[idx + 7..];
                                if let Some(end) = after.find('"') {
                                    found_cwd = Some(after[..end].to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        let cwd_str = found_cwd.unwrap_or_else(|| {
            // Lossy fallback decode: `-home-joe-ai-aida` → `/home/joe/ai/aida`.
            let name = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            name.replace('-', "/")
        });
        let cwd_path = std::path::Path::new(&cwd_str);
        if cwd_path.is_dir() {
            active += 1;
            continue;
        }
        if cwd_str.is_empty() {
            errored += 1;
            continue;
        }
        orphans.push(Orphan {
            path: dir,
            decoded_cwd: cwd_str,
            jsonl_count,
        });
    }

    if orphans.is_empty() {
        println!(
            "(no orphan project dirs; {} active dir{} under {})",
            active,
            if active == 1 { "" } else { "s" },
            projects.display()
        );
        if errored > 0 {
            println!(
                "  ({} dir{} couldn't be inspected — left in place)",
                errored,
                if errored == 1 { "" } else { "s" }
            );
        }
        return Ok(());
    }

    println!(
        "Found {} orphan Claude Code project dir{} (cwd missing):",
        orphans.len(),
        if orphans.len() == 1 { "" } else { "s" }
    );
    println!();
    for o in &orphans {
        println!("  {}", o.path.display().to_string().dimmed());
        println!(
            "      cwd: {}    {} jsonl{}",
            o.decoded_cwd.yellow(),
            o.jsonl_count,
            if o.jsonl_count == 1 { "" } else { "s" }
        );
    }
    println!();

    if dry_run {
        println!("{} (--dry-run; nothing removed)", "Dry run".dimmed());
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        eprint!("Remove these {} orphan dir(s)? [y/N] ", orphans.len());
        std::io::stderr().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mut removed = 0usize;
    for o in &orphans {
        match std::fs::remove_dir_all(&o.path) {
            Ok(_) => removed += 1,
            Err(e) => eprintln!(
                "{} couldn't remove {}: {}",
                "Warning:".yellow().bold(),
                o.path.display(),
                e
            ),
        }
    }
    println!(
        "{} removed {} orphan project dir{}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        removed,
        if removed == 1 { "" } else { "s" }
    );

    // BUG-80: also sweep planned-cluster manifests for leases that no
    // longer exist in this project. Same `--orphans` flag, same dry-run /
    // yes semantics. trace:BUG-80 | ai:claude
    if let Ok(project_root) = find_project_root() {
        let manifests = session_manifest::list_all_with_paths(&project_root);
        let sessions_dir = project_root.join(".aida").join("sessions");
        let orphan_manifests: Vec<_> = manifests
            .into_iter()
            .filter(|(_, m)| !sessions_dir.join(format!("{}.toml", m.session_id)).exists())
            .collect();
        if !orphan_manifests.is_empty() {
            println!();
            println!(
                "Found {} orphan session manifest{}:",
                orphan_manifests.len(),
                if orphan_manifests.len() == 1 { "" } else { "s" }
            );
            for (p, _) in &orphan_manifests {
                println!("  {}", p.display().to_string().dimmed());
            }
            if !dry_run {
                let mut mremoved = 0usize;
                for (p, _) in &orphan_manifests {
                    if std::fs::remove_file(p).is_ok() {
                        mremoved += 1;
                    }
                }
                println!(
                    "{} removed {} orphan manifest{}",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    mremoved,
                    if mremoved == 1 { "" } else { "s" }
                );
            }
        }
    }

    Ok(())
}

/// TASK-358: explicit pruner for `--escalate-blocks` worktrees whose
/// underlying spec has been triaged out of `NeedsAttention`. The auto-clean
/// in `edit_requirement_cli` covers the happy path; this verb is the
/// recovery surface for cases where the auto-clean didn't fire (an older
/// triage that pre-dates this code, a write that errored, a sibling
/// worktree at edit time). A lease with the marker but whose spec is still
/// in `NeedsAttention` is left alone — the human hasn't triaged yet.
/// trace:TASK-358 | ai:claude
pub(crate) fn session_prune_escalations(dry_run: bool, yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let leases = list_leases(&project_root);

    // Two buckets: the prune-eligible (marker set AND spec out of
    // NeedsAttention) and the still-parked (marker set AND spec still in
    // NeedsAttention — left alone, surfaced as info).
    let storage = Storage::new(project_root.join(".aida-store"));
    let store = storage.load().ok();
    let lookup_status = |spec: &str| -> Option<RequirementStatus> {
        store.as_ref().and_then(|s| {
            s.requirements
                .iter()
                .find(|r| {
                    r.spec_id
                        .as_deref()
                        .map(|sid| sid.eq_ignore_ascii_case(spec))
                        .unwrap_or(false)
                })
                .map(|r| r.status.clone())
        })
    };

    let mut eligible: Vec<(SessionLease, Option<RequirementStatus>)> = Vec::new();
    let mut still_parked: Vec<SessionLease> = Vec::new();
    for lease in leases {
        if lease.escalated_to_human.is_none() {
            continue;
        }
        let status = lookup_status(&lease.scope);
        match &status {
            Some(RequirementStatus::NeedsAttention) => still_parked.push(lease),
            _ => eligible.push((lease, status)),
        }
    }

    if !still_parked.is_empty() {
        println!(
            "Skipping {} escalated worktree{} — spec still in Needs Attention:",
            still_parked.len(),
            if still_parked.len() == 1 { "" } else { "s" }
        );
        for l in &still_parked {
            println!(
                "  {}  {}  ({})",
                l.scope.cyan(),
                l.worktree_path.display().to_string().dimmed(),
                "awaiting human triage".dimmed(),
            );
        }
        println!();
    }

    if eligible.is_empty() {
        println!(
            "(no prune-eligible escalated worktrees{})",
            if still_parked.is_empty() {
                ""
            } else {
                " — all marked sessions are still in Needs Attention"
            }
        );
        return Ok(());
    }

    println!(
        "Found {} escalated worktree{} whose spec has been triaged:",
        eligible.len(),
        if eligible.len() == 1 { "" } else { "s" }
    );
    for (l, status) in &eligible {
        let status_label = status
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "  {}  {}  → {}",
            l.scope.cyan(),
            l.worktree_path.display().to_string().dimmed(),
            status_label.green(),
        );
    }
    println!();

    if dry_run {
        println!("{} (--dry-run; nothing removed)", "Dry run".dimmed());
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        eprint!(
            "Remove these {} escalated worktree(s)? [y/N] ",
            eligible.len()
        );
        std::io::stderr().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mut removed = 0usize;
    for (l, _) in &eligible {
        if force_cleanup_lease(&project_root, l) {
            removed += 1;
        }
    }
    println!(
        "{} removed {} escalated worktree{}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        removed,
        if removed == 1 { "" } else { "s" }
    );
    Ok(())
}

pub(crate) fn session_prune(
    days: u32,
    dry_run: bool,
    yes: bool,
    orphans: bool,
    escalations: bool,
) -> Result<()> {
    // TASK-70: orphan-sweep mode short-circuits the per-file age path.
    // Iterates every project dir under ~/.claude/projects and removes any
    // whose recorded cwd no longer exists on disk (stranded by an
    // earlier `aida session end` that didn't `--purge-cc`).
    // trace:TASK-70 | ai:claude
    if orphans {
        return session_prune_orphans(dry_run, yes);
    }
    // TASK-358: escalations-sweep mode — find lingering `--escalate-blocks`
    // worktrees whose spec has been triaged out of NeedsAttention and clean
    // them up. The auto-clean in `edit_requirement_cli` covers the happy
    // path; this is the explicit recovery surface.
    // trace:TASK-358 | ai:claude
    if escalations {
        return session_prune_escalations(dry_run, yes);
    }

    let cwd = std::env::current_dir().context("could not determine cwd")?;
    let project_root = find_project_root().unwrap_or_else(|_| cwd.clone());

    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(u64::from(days) * 86_400))
        .ok_or_else(|| anyhow::anyhow!("--days {} is too large", days))?;
    let now = std::time::SystemTime::now();

    // Project dirs to walk: the current cwd's encoded dir, plus the parent
    // project's (when run from inside a session worktree). De-dup since
    // they collapse to the same dir for plain non-worktree usage.
    let mut search_dirs: Vec<std::path::PathBuf> = Vec::new();
    let push_dir = |dirs: &mut Vec<std::path::PathBuf>, p: std::path::PathBuf| {
        if p.is_dir() && !dirs.iter().any(|d| d == &p) {
            dirs.push(p);
        }
    };
    if let Ok(d) = session::claude_project_dir(&cwd) {
        push_dir(&mut search_dirs, d);
    }
    if let Some(parent) = parent_project_root_for_session(&cwd) {
        if let Ok(d) = session::claude_project_dir(&parent) {
            push_dir(&mut search_dirs, d);
        }
    }
    // Also include encoded dirs for every active lease's worktree path —
    // they often live OUTSIDE the parent project root (sibling worktrees),
    // so the parent-walk doesn't catch them. We'll skip these in the
    // candidate loop (see active_dirs below) but we still need to know
    // about them for the "skipped active" tally. trace:STORY-60 | ai:claude
    let active_leases = list_leases(&project_root);
    let mut active_dirs: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    for l in &active_leases {
        if let Ok(d) = session::claude_project_dir(&l.worktree_path) {
            active_dirs.insert(d.clone());
            push_dir(&mut search_dirs, d);
        }
    }

    if search_dirs.is_empty() {
        println!("(no Claude Code session directories found for this project)");
        return Ok(());
    }

    let mut candidates: Vec<PruneCandidate> = Vec::new();
    let mut skipped_active = 0usize;
    for dir in &search_dirs {
        if active_dirs.contains(dir) {
            // Defensive: anything in here belongs to an in-progress
            // session, regardless of mtime. Skip wholesale.
            // trace:STORY-60 | ai:claude
            if let Ok(read) = std::fs::read_dir(dir) {
                skipped_active += read
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
                    .count();
            }
            continue;
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in read.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let Ok(mtime) = metadata.modified() else {
                continue;
            };
            if mtime >= cutoff {
                continue;
            }
            let age_seconds = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);
            candidates.push(PruneCandidate {
                path,
                size: metadata.len(),
                age_seconds,
            });
        }
    }

    if candidates.is_empty() {
        println!(
            "(no .jsonl files older than {} day{} in {} session director{})",
            days,
            if days == 1 { "" } else { "s" },
            search_dirs.len(),
            if search_dirs.len() == 1 { "y" } else { "ies" },
        );
        if skipped_active > 0 {
            println!(
                "  ({} file{} skipped — belong to {} active session{})",
                skipped_active,
                if skipped_active == 1 { "" } else { "s" },
                active_dirs.len(),
                if active_dirs.len() == 1 { "" } else { "s" },
            );
        }
        return Ok(());
    }

    // Sort oldest-first so the user sees the most-stale entries up top.
    candidates.sort_by(|a, b| b.age_seconds.cmp(&a.age_seconds));

    let total_size: u64 = candidates.iter().map(|c| c.size).sum();
    let count = candidates.len();
    println!(
        "Found {} .jsonl file{} older than {} day{} ({} total):",
        count,
        if count == 1 { "" } else { "s" },
        days,
        if days == 1 { "" } else { "s" },
        humanize_size(total_size)
    );
    println!();
    for c in &candidates {
        let id = c.path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
        let id_short = &id[..id.len().min(8)];
        println!(
            "  {:>6}  {:>10}  {}  {}",
            humanize_age_secs(c.age_seconds),
            humanize_size(c.size),
            id_short.dimmed(),
            c.path.display()
        );
    }
    println!();
    if skipped_active > 0 {
        println!(
            "{} {} file{} from {} active session{} excluded.",
            "Note:".yellow().bold(),
            skipped_active,
            if skipped_active == 1 { "" } else { "s" },
            active_dirs.len(),
            if active_dirs.len() == 1 { "" } else { "s" },
        );
    }

    if dry_run {
        println!("{}", "(--dry-run; nothing deleted)".dimmed());
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        print!(
            "Delete {} file{}? [y/N] ",
            count,
            if count == 1 { "" } else { "s" }
        );
        std::io::stdout().flush().ok();
        let mut ans = String::new();
        if std::io::stdin().read_line(&mut ans).is_err()
            || !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes")
        {
            println!("Aborted.");
            return Ok(());
        }
    }

    let log_path = project_root.join(".aida").join("session-prune.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut log_handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    let mut deleted = 0usize;
    let mut bytes_freed = 0u64;
    let mut errors = 0usize;
    let now_iso = chrono::Utc::now().to_rfc3339();
    for c in &candidates {
        match std::fs::remove_file(&c.path) {
            Ok(_) => {
                deleted += 1;
                bytes_freed += c.size;
                if let Some(f) = log_handle.as_mut() {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "{}\t{}\t{}\t{}",
                        now_iso,
                        c.size,
                        c.age_seconds,
                        c.path.display()
                    );
                }
            }
            Err(e) => {
                errors += 1;
                eprintln!(
                    "{} could not delete {}: {}",
                    "Warning:".yellow().bold(),
                    c.path.display(),
                    e
                );
            }
        }
    }

    println!(
        "{} Deleted {} file{} ({} freed){}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        deleted,
        if deleted == 1 { "" } else { "s" },
        humanize_size(bytes_freed),
        if errors > 0 {
            format!("; {} error{}", errors, if errors == 1 { "" } else { "s" })
        } else {
            String::new()
        }
    );
    if log_handle.is_some() {
        println!("  log: {}", log_path.display().to_string().dimmed());
    }
    Ok(())
}

/// STORY-68: detail view for one session lease. Resolution: explicit id
/// → cwd-based lease → ancestor-PID match (same chain as session end's
/// resolution chain, minus the single-active prompt — `show` should be
/// non-interactive). Errors with the active-lease listing if nothing
/// resolves. trace:STORY-68 | ai:claude
pub(crate) fn session_show(id: Option<&str>, plan: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let leases = list_leases(&project_root);
    if leases.is_empty() {
        println!("(no active sessions)");
        println!();
        println!(
            "Start one with: {} {}",
            "aida session start --owns".cyan(),
            "<scope>".dimmed()
        );
        return Ok(());
    }

    let lease = if let Some(q) = id {
        find_lease_by_id_prefix(q, &leases)?
    } else if let Some(l) = std::env::current_dir().ok().and_then(|cwd| {
        let canon = cwd.canonicalize().unwrap_or(cwd);
        leases
            .iter()
            .find(|&l| lease_covers_cwd(l, &canon))
            .cloned()
    }) {
        l
    } else {
        // Ancestor-PID fallback (no env-var lookup here — `show` is for
        // human inspection, not env mutation, so we want a deterministic
        // best-effort match without relying on the export side channel).
        let chain: std::collections::HashSet<u32> =
            process_probe::walk_ancestor_pids(std::process::id())
                .into_iter()
                .collect();
        let candidates: Vec<&SessionLease> = leases
            .iter()
            .filter(|l| l.creator_pid.map(|p| chain.contains(&p)).unwrap_or(false))
            .collect();
        match candidates.len() {
            1 => candidates[0].clone(),
            _ => {
                let mut msg = String::from(
                    "no session lease covers cwd or this shell's ancestors — pass an id explicitly\n\nActive sessions:",
                );
                for l in &leases {
                    msg.push_str(&format!(
                        "\n  {} {} ({})",
                        &l.id[..8],
                        l.scope,
                        l.worktree_path.display()
                    ));
                }
                anyhow::bail!(msg)
            }
        }
    };

    let lease_file = lease_path(&project_root, &lease.id);
    let started_local = lease.started_at.with_timezone(&chrono::Local);
    let now = chrono::Utc::now();
    let age = now.signed_duration_since(lease.started_at);
    let age_human = humanize_relative(lease.started_at);

    println!("{} {}", "Session".bold(), (&lease.id[..8]).yellow());
    println!("  {}: {}", "scope".bold(), lease.scope.cyan());
    println!("  {}: {}", "branch".bold(), lease.branch.cyan());
    println!("  {}: {}", "worktree".bold(), lease.worktree_path.display());
    if let Some(role) = &lease.role {
        println!(
            "  {}: {} {}",
            "role".bold(),
            role.cyan(),
            "(inherited from shell at start time)".dimmed()
        );
    }
    println!(
        "  {}: {} ({})",
        "started_at".bold(),
        started_local.format("%Y-%m-%d %H:%M %Z"),
        age_human.dimmed()
    );
    if age.num_seconds() < 0 {
        // Future-dated lease (clock skew or hand-edited TOML) — flag it.
        eprintln!(
            "  {} lease started_at is in the future — clock skew?",
            "Warning:".yellow().bold()
        );
    }
    println!("  {}: {}", "owner".bold(), lease.owner);
    println!("  {}: {}", "hostname".bold(), lease.hostname);
    // STORY-71 / TASK-51: when this is a review session (--owns PR-N or
    // MR-N), session_start captured the head/base SHAs and base ref.
    // Render them on one line so the reviewer can see what diff range
    // they're working against. Omit the line entirely for non-PR
    // sessions so non-review output stays clean.
    // trace:STORY-71, TASK-51 | ai:claude
    if lease.pr_head_sha.is_some() || lease.pr_base_sha.is_some() || lease.pr_base_ref.is_some() {
        let head = lease
            .pr_head_sha
            .as_deref()
            .map(|s| s[..s.len().min(12)].to_string())
            .unwrap_or_else(|| "?".to_string());
        let base = match (lease.pr_base_ref.as_deref(), lease.pr_base_sha.as_deref()) {
            (Some(r), Some(b)) => format!("{} ({})", r, &b[..b.len().min(12)]),
            (Some(r), None) => r.to_string(),
            (None, Some(b)) => b[..b.len().min(12)].to_string(),
            (None, None) => "?".to_string(),
        };
        println!(
            "  {}: head {}  base {}",
            "PR / MR".bold(),
            head.cyan(),
            base.cyan()
        );
    }
    if let Some(pid) = lease.creator_pid {
        let descendant = {
            let chain: std::collections::HashSet<u32> =
                process_probe::walk_ancestor_pids(std::process::id())
                    .into_iter()
                    .collect();
            chain.contains(&pid)
        };
        let suffix = if descendant {
            " (this shell is a descendant)".green().to_string()
        } else {
            String::new()
        };
        println!("  {}: {}{}", "creator_pid".bold(), pid, suffix);
    }
    println!(
        "  {}: {}",
        "lease file".bold(),
        lease_file.display().to_string().dimmed()
    );

    // Activity rows from the session-local log.
    let activity = load_session_activity(&project_root, &lease.id);
    println!();
    if activity.entries.is_empty() {
        println!("{}", "Activity in this session: (none yet)".bold());
    } else {
        println!("{}", "Activity in this session:".bold());
        // BUG-83: resolve activity-log spec_id keys to their preferred
        // display id (agreed_id when assigned). The log records whatever
        // form the user typed at the call site, so a row written under
        // legacy `STORY-2-076` should render here as the short
        // post-merge-gate id once one's been minted.
        // trace:BUG-83 | ai:claude
        let display_lookup = build_display_id_lookup(&project_root);
        let mut sorted = activity.entries.clone();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.at));
        for e in sorted.iter().take(10) {
            let display_id = display_lookup
                .get(&e.spec_id)
                .cloned()
                .unwrap_or_else(|| e.spec_id.clone());
            println!(
                "  {:<14} {:<10} {} ({})",
                display_id.cyan(),
                e.action,
                e.role.dimmed(),
                humanize_relative(e.at).dimmed()
            );
        }
        if activity.entries.len() > 10 {
            println!(
                "  {} {} more",
                "…".dimmed(),
                (activity.entries.len() - 10).to_string().dimmed()
            );
        }
    }

    // Liveness: is there a live claude inside this worktree right now?
    let live_in_wt = probe_live_claudes_in_worktree(&lease.worktree_path);
    println!();
    if live_in_wt.is_empty() {
        println!("{}: (no live claude detected)", "Claude Code".bold());
    } else {
        println!(
            "{}: {} live claude process{} in worktree",
            "Claude Code".bold(),
            live_in_wt.len().to_string().green(),
            if live_in_wt.len() == 1 { "" } else { "es" }
        );
        for s in &live_in_wt {
            let jsonl_note = match &s.jsonl {
                Some(p) => format!(
                    " (jsonl {})",
                    p.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("?")
                        .dimmed()
                ),
                None => String::new(),
            };
            println!("  pid {}{}", s.pid.to_string().green(), jsonl_note);
        }
    }

    let dangling = probe_dangling_claudes_at_path(&lease.worktree_path);
    if !dangling.is_empty() {
        println!();
        println!(
            "{} {} leaked claude process(es) with `(deleted)` cwd at this path:",
            "Warning:".yellow().bold(),
            dangling.len()
        );
        for s in &dangling {
            println!(
                "  pid {} — `kill {}` to clean up",
                s.pid.to_string().yellow(),
                s.pid
            );
        }
    }

    if plan {
        render_session_manifest(&project_root, &lease.id)?;
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionHandoffRecommendation {
    StartFresh,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionHandoffFacts {
    pub(crate) live_drain: Option<String>,
    pub(crate) stale_drain: Option<String>,
    pub(crate) live_session_leases: Vec<String>,
}

pub(crate) fn decide_session_handoff(facts: &SessionHandoffFacts) -> SessionHandoffRecommendation {
    if facts.live_drain.is_some() || !facts.live_session_leases.is_empty() {
        SessionHandoffRecommendation::Compact
    } else {
        SessionHandoffRecommendation::StartFresh
    }
}

pub(crate) fn active_handoff_leases(
    leases: &[SessionLease],
    cwd: Option<&std::path::Path>,
) -> Vec<String> {
    let mut out = Vec::new();
    for lease in leases {
        let covers_cwd = cwd.is_some_and(|cwd| lease_covers_cwd(lease, cwd));
        if covers_cwd {
            out.push(format!(
                "{} {} ({})",
                &lease.id[..8.min(lease.id.len())],
                lease.scope,
                lease.branch
            ));
        }
    }
    out.sort();
    out.dedup();
    out
}

pub(crate) fn collect_session_handoff_facts(project_root: &std::path::Path) -> SessionHandoffFacts {
    let (live_drain, stale_drain) = match drain_lock::probe_lock(project_root) {
        drain_lock::LockStatus::Running(lock) => (
            Some(format!(
                "pid {} on {}: {}",
                lock.pid, lock.host, lock.command
            )),
            None,
        ),
        drain_lock::LockStatus::Stale(lock) => (
            None,
            Some(format!(
                "pid {} on {} is not alive: {}",
                lock.pid, lock.host, lock.command
            )),
        ),
        drain_lock::LockStatus::None => (None, None),
    };
    let cwd = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.canonicalize().unwrap_or(cwd));
    let live_session_leases = active_handoff_leases(&list_leases(project_root), cwd.as_deref());
    SessionHandoffFacts {
        live_drain,
        stale_drain,
        live_session_leases,
    }
}

/// STORY-1464: write or show a seat handoff note (the rotation primitive).
// trace:STORY-1464 | ai:claude
pub(crate) fn session_handoff_seat(
    write: Option<&str>,
    _show: bool,
    seat: Option<&str>,
) -> Result<()> {
    let seat = match seat {
        Some(s) => s.to_string(),
        None => std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| canonical_role_name(&s))
            .ok_or_else(|| {
                anyhow::anyhow!("no seat given: pass --seat <seat> or set AIDA_SESSION_ROLE")
            })?,
    };
    let project_root = main_worktree_root_from(&find_project_root()?);
    if let Some(src) = write {
        let body = if src == "-" {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
            s
        } else {
            std::fs::read_to_string(src).with_context(|| format!("reading {src}"))?
        };
        let written =
            seat_rotation::write_handoff(&project_root, &seat, &body, chrono::Utc::now())?;
        println!(
            "Handoff written: {} ({} bytes)",
            written.path.display(),
            written.bytes
        );
        println!("Latest: {}", written.latest.display());
        println!("Next session: aida session handoff --seat {seat} --show");
        return Ok(());
    }
    match seat_rotation::read_latest_handoff(&project_root, &seat)? {
        Some(text) => print!("{text}"),
        None => println!("No handoff recorded for seat '{seat}'."),
    }
    Ok(())
}

pub(crate) fn session_handoff_check(_check: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let facts = collect_session_handoff_facts(&project_root);
    let recommendation = decide_session_handoff(&facts);
    // trace:STORY-1006 | ai:codex
    match recommendation {
        SessionHandoffRecommendation::StartFresh => {
            println!("Recommendation: START FRESH");
            println!("Reason: no live drain or current-session lease pins this conversation.");
        }
        SessionHandoffRecommendation::Compact => {
            println!("Recommendation: COMPACT");
            println!("Reason: live session-bound state is still attached here:");
            if let Some(drain) = &facts.live_drain {
                println!("  - live drain: {drain}");
            }
            for lease in &facts.live_session_leases {
                println!("  - live lease: {lease}");
            }
        }
    }
    if let Some(stale) = &facts.stale_drain {
        println!("Note: stale drain lock present but not a compact pin: {stale}");
    }
    println!(
        "CLI limit: conversation-only residue is unknowable here; run /aida-handoff or /aida-capture before ending the session."
    );
    Ok(())
}

#[cfg(test)]
mod session_handoff_tests {
    use super::*;

    fn facts() -> SessionHandoffFacts {
        SessionHandoffFacts {
            live_drain: None,
            stale_drain: None,
            live_session_leases: Vec::new(),
        }
    }

    #[test]
    fn handoff_recommends_start_fresh_when_substrate_has_no_live_pins() {
        assert_eq!(
            decide_session_handoff(&facts()),
            SessionHandoffRecommendation::StartFresh
        );
    }

    #[test]
    fn handoff_recommends_compact_for_live_drain() {
        let mut f = facts();
        f.live_drain = Some("pid 123: aida queue work --auto-complete".to_string());
        assert_eq!(
            decide_session_handoff(&f),
            SessionHandoffRecommendation::Compact
        );
    }

    #[test]
    fn handoff_recommends_compact_for_live_session_lease() {
        let mut f = facts();
        f.live_session_leases
            .push("abcd1234 STORY-1006 (story-1006)".to_string());
        assert_eq!(
            decide_session_handoff(&f),
            SessionHandoffRecommendation::Compact
        );
    }

    #[test]
    fn handoff_stale_drain_does_not_pin_compaction() {
        let mut f = facts();
        f.stale_drain = Some("pid 123 is not alive".to_string());
        assert_eq!(
            decide_session_handoff(&f),
            SessionHandoffRecommendation::StartFresh
        );
    }
}

/// Render the planned-cluster manifest for `session_id` as a status table.
/// Reads the requirement store once to resolve each item's current status
/// (so a spec the user flipped to "in progress" via /aida-pickup actually
/// shows as the in-flight glyph regardless of whether mark-started ran). trace:STORY-98
pub(crate) fn render_session_manifest(
    project_root: &std::path::Path,
    session_id: &str,
) -> Result<()> {
    let path = session_manifest::manifest_path(project_root, session_id);
    println!();
    if !path.exists() {
        println!(
            "{} (no planned-cluster manifest — /aida-pickup hasn't recorded one for this session)",
            "Plan:".bold()
        );
        println!(
            "  {} {}",
            "tip:".dimmed(),
            "/aida-pickup writes the manifest when it confirms a multi-item cluster".dimmed()
        );
        return Ok(());
    }
    let manifest = session_manifest::load(&path)?;

    // Resolve each item's current store status — expensive, so do it once.
    // Git-canonical first (the default backend now), falling back to the
    // legacy Storage path if no .aida-store/ is present.
    // trace:STORY-98 | ai:claude
    let store = load_store_for_lookup(project_root);

    let mut done = 0usize;
    let mut in_progress = 0usize;
    let mut pending = 0usize;
    let total = manifest.items.len();

    // Pre-compute per-item (status, title, display_id).
    // BUG-83: the manifest stores spec_id (canonical), but the table
    // should display agreed_id when present so the plan view matches
    // `aida list` / queue. trace:BUG-83 | ai:claude
    let mut rows: Vec<(
        session_manifest::ItemStatus,
        &session_manifest::ManifestItem,
        String, // title
        String, // display_id
    )> = Vec::with_capacity(total);
    for item in &manifest.items {
        let (status_str, title, display_id) = if let Some(store) = &store {
            let req = store
                .requirements
                .iter()
                .find(|r| r.spec_id.as_deref() == Some(item.spec_id.as_str()));
            (
                req.map(|r| format!("{}", r.status)),
                req.map(|r| r.title.clone())
                    .unwrap_or_else(|| "(not found)".to_string()),
                req.map(|r| r.display_id())
                    .unwrap_or_else(|| item.spec_id.clone()),
            )
        } else {
            (None, String::new(), item.spec_id.clone())
        };
        let st = session_manifest::classify_item(item, status_str.as_deref());
        match st {
            session_manifest::ItemStatus::Done => done += 1,
            session_manifest::ItemStatus::InProgress => in_progress += 1,
            session_manifest::ItemStatus::Pending => pending += 1,
        }
        rows.push((st, item, title, display_id));
    }

    println!(
        "{} {} items, {} done, {} in-progress, {} pending",
        "Plan:".bold(),
        total,
        done.to_string().green(),
        in_progress.to_string().yellow(),
        pending.to_string().dimmed(),
    );
    println!(
        "  {} {} · {} {}",
        "planned_at:".dimmed(),
        manifest
            .planned_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M %Z"),
        "source:".dimmed(),
        manifest.plan_source.cyan()
    );
    // TASK-272: surface the batch marker so /aida-pickup and /aida-pr can
    // detect batch context from this view. trace:TASK-272 | ai:claude
    if let Some(batch) = &manifest.batch_name {
        println!("  {} {}", "batch:".dimmed(), batch.cyan());
    }
    println!();
    println!(
        "  {:<3} {:<14} {:<14} {}",
        "#".dimmed(),
        "Status".dimmed(),
        "Spec".dimmed(),
        "Title".dimmed()
    );
    for (st, item, title, display_id) in &rows {
        let glyph = st.glyph();
        let label = st.label();
        let display = format!("{} {}", glyph, label);
        let colored = match st {
            session_manifest::ItemStatus::Done => display.green(),
            session_manifest::ItemStatus::InProgress => display.yellow(),
            session_manifest::ItemStatus::Pending => display.dimmed(),
        };
        println!(
            "  {:<3} {:<14} {:<14} {}",
            item.position,
            colored,
            display_id.bold(),
            title
        );
    }

    // TASK-95: the plan brief — Critical Files / Followups / Verification
    // pre-populated by `aida queue work` from the owning docs/plans/ file.
    // This is what /aida-pickup surfaces so the implementer gets their
    // blast radius and definition of done without grepping. trace:TASK-95
    if let Some(ctx) = &manifest.plan {
        println!();
        println!("{} {}", "Plan brief:".bold(), ctx.plan_file.cyan());
        if !ctx.critical_files.is_empty() {
            println!(
                "  {} ({})",
                "Critical files".dimmed(),
                ctx.critical_files.len()
            );
            for f in &ctx.critical_files {
                println!("    {}", f);
            }
        }
        if !ctx.followups.is_empty() {
            println!("  {} ({})", "Followups".dimmed(), ctx.followups.len());
            for f in &ctx.followups {
                println!("    - {}", f);
            }
        }
        if let Some(v) = &ctx.verification {
            println!("  {}", "Definition of done".dimmed());
            for l in v.lines() {
                println!("    {}", l.dimmed());
            }
        }
    }

    Ok(())
}

/// STORY-98: best-effort manifest-row flip on `aida edit --status`.
/// Resolves the active session lease via cwd / ancestor-PID match, then
/// stamps started_at or completed_at on the matching item. Silent no-op
/// when no lease covers this shell or the manifest doesn't list the
/// spec — most edits happen outside a planned cluster, so the path stays
/// quiet by design. trace:STORY-98 | ai:claude
pub(crate) fn update_manifest_for_status(spec_id: &str, canonical_status: &str) {
    let Ok(project_root) = find_project_root() else {
        return;
    };
    let leases = list_leases(&project_root);
    if leases.is_empty() {
        return;
    }
    let lease = match std::env::current_dir().ok() {
        Some(cwd) => {
            let canon = cwd.canonicalize().unwrap_or(cwd);
            leases
                .iter()
                .find(|&l| lease_covers_cwd(l, &canon))
                .cloned()
        }
        None => None,
    }
    .or_else(|| {
        let chain: std::collections::HashSet<u32> =
            process_probe::walk_ancestor_pids(std::process::id())
                .into_iter()
                .collect();
        let ancestor: Vec<&SessionLease> = leases
            .iter()
            .filter(|l| l.creator_pid.map(|p| chain.contains(&p)).unwrap_or(false))
            .collect();
        if ancestor.len() == 1 {
            Some(ancestor[0].clone())
        } else {
            None
        }
    });
    let Some(lease) = lease else {
        return;
    };
    let path = session_manifest::manifest_path(&project_root, &lease.id);
    if !path.exists() {
        return;
    }
    let lower = canonical_status.to_ascii_lowercase();
    let res = if lower == "in-progress" || lower == "in_progress" || lower == "in progress" {
        session_manifest::mark_started(&path, spec_id)
    } else if lower == "done" || lower == "completed" || lower == "rejected" {
        // STORY-86: `Done` checks the manifest item off the same way
        // `Completed` does. The cluster only cares "is this item visually
        // shipped?" — both states qualify (Done = on branch, Completed =
        // on main; either counts as "the work is no longer in flight").
        // trace:STORY-86 | ai:claude
        session_manifest::mark_completed(&path, spec_id)
    } else {
        return; // draft/approved transitions don't move the manifest needle
    };
    if let Err(e) = res {
        eprintln!(
            "{} session-manifest update failed for {} ({}): {}",
            "Warning:".yellow().bold(),
            spec_id,
            canonical_status,
            e
        );
    }
}

/// Build a map from "any id form a caller might have stored"
/// (spec_id or agreed_id) to the requirement's preferred display id
/// (agreed_id when assigned, else spec_id). Used by the session-show
/// activity rows so log entries written under either form render
/// consistently as the short post-merge-gate id once one's been minted.
///
/// Returns an empty map when the store can't be loaded — callers fall
/// back to the raw stored id. trace:BUG-83 | ai:claude
pub(crate) fn build_display_id_lookup(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, String> {
    let Some(store) = load_store_for_lookup(project_root) else {
        return std::collections::HashMap::new();
    };
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for r in &store.requirements {
        let display = r.display_id();
        if let Some(sid) = r.spec_id.as_deref() {
            map.insert(sid.to_string(), display.clone());
        }
        if let Some(aid) = r.agreed_id.as_deref() {
            map.entry(aid.to_string()).or_insert(display);
        }
    }
    map
}

/// `aida session manifest <subcommand>` dispatcher. trace:STORY-98
pub(crate) fn session_manifest_dispatch(cmd: &SessionManifestCommand) -> Result<()> {
    match cmd {
        SessionManifestCommand::Write {
            items,
            source,
            session,
        } => session_manifest_write(items, source, session.as_deref()),
        SessionManifestCommand::MarkStarted { spec_id, session } => {
            session_manifest_mark(spec_id, session.as_deref(), MarkKind::Started)
        }
        SessionManifestCommand::MarkCompleted { spec_id, session } => {
            session_manifest_mark(spec_id, session.as_deref(), MarkKind::Completed)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MarkKind {
    Started,
    Completed,
}

/// Resolve the target lease for a manifest operation: explicit id wins,
/// else the lease covering cwd, else ancestor-PID match, else error.
/// Centralized so all three manifest subcommands resolve identically.
pub(crate) fn resolve_manifest_target_lease(
    project_root: &std::path::Path,
    session_query: Option<&str>,
) -> Result<SessionLease> {
    let leases = list_leases(project_root);
    if leases.is_empty() {
        anyhow::bail!(
            "no active sessions — start one with `aida session start --owns <scope>` before writing a manifest"
        );
    }
    if let Some(q) = session_query {
        return find_lease_by_id_prefix(q, &leases);
    }
    if let Ok(cwd) = std::env::current_dir() {
        let canon = cwd.canonicalize().unwrap_or(cwd);
        if let Some(l) = leases.iter().find(|&l| lease_covers_cwd(l, &canon)) {
            return Ok(l.clone());
        }
    }
    let chain: std::collections::HashSet<u32> =
        process_probe::walk_ancestor_pids(std::process::id())
            .into_iter()
            .collect();
    let ancestor: Vec<&SessionLease> = leases
        .iter()
        .filter(|l| l.creator_pid.map(|p| chain.contains(&p)).unwrap_or(false))
        .collect();
    if ancestor.len() == 1 {
        return Ok(ancestor[0].clone());
    }
    anyhow::bail!(
        "could not resolve a single active session — pass --session <id> ({} active)",
        leases.len()
    );
}

pub(crate) fn session_manifest_write(
    items: &str,
    source: &str,
    session_query: Option<&str>,
) -> Result<()> {
    let project_root = find_project_root()?;
    let lease = resolve_manifest_target_lease(&project_root, session_query)?;

    let parsed: Vec<String> = items
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if parsed.is_empty() {
        anyhow::bail!("--items is empty — pass a comma-separated list of SPEC-IDs");
    }

    // Resolve current store statuses so status_at_plan reflects truth.
    let store = load_store_for_lookup(&project_root);

    let now = chrono::Utc::now();
    let mut manifest_items: Vec<session_manifest::ManifestItem> = Vec::with_capacity(parsed.len());
    // BUG-83: collect parallel display_ids so the confirmation echo
    // shows agreed_id when one's been assigned. The manifest itself
    // keeps spec_id (canonical) on disk so mark-started / mark-completed
    // continue to key off it. trace:BUG-83 | ai:claude
    let mut display_ids: Vec<String> = Vec::with_capacity(parsed.len());
    for (i, spec_id) in parsed.iter().enumerate() {
        let req = store.as_ref().and_then(|s| {
            s.requirements
                .iter()
                .find(|r| r.spec_id.as_deref() == Some(spec_id.as_str()))
        });
        let status = req
            .map(|r| format!("{}", r.status))
            .unwrap_or_else(|| "Unknown".to_string());
        let display_id = req
            .map(|r| r.display_id())
            .unwrap_or_else(|| spec_id.clone());
        manifest_items.push(session_manifest::ManifestItem {
            spec_id: spec_id.clone(),
            position: (i + 1) as u32,
            status_at_plan: status,
            started_at: None,
            completed_at: None,
            note: None,
        });
        display_ids.push(display_id);
    }

    // TASK-272: preserve a batch marker recorded by `aida queue work
    // --batch` — rewriting the planned cluster (e.g. /aida-pickup Step 3a
    // recording a user-confirmed multi-item batch) must not silently drop
    // the batch context /aida-pickup keys its next-steps menu off.
    // trace:TASK-272 | ai:claude
    let path = session_manifest::manifest_path(&project_root, &lease.id);
    let batch_name = session_manifest::load(&path)
        .ok()
        .and_then(|m| m.batch_name);

    let manifest = session_manifest::SessionManifest {
        session_id: lease.id.clone(),
        planned_at: now,
        plan_source: source.to_string(),
        claude_session_id: None,
        batch_name,
        plan: None,
        items: manifest_items,
    };

    session_manifest::save(&path, &manifest)?;

    println!(
        "{} {} for session {} ({} items)",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        "Wrote planned-cluster manifest".bold(),
        (&lease.id[..lease.id.len().min(8)]).yellow(),
        manifest.items.len()
    );
    for (it, display_id) in manifest.items.iter().zip(display_ids.iter()) {
        println!(
            "  {}. {}  [{}]",
            it.position.to_string().dimmed(),
            display_id.bold(),
            it.status_at_plan.dimmed()
        );
    }
    println!();
    println!(
        "  {} {} {}",
        "manifest:".dimmed(),
        path.display().to_string().dimmed(),
        "(view with `aida session show --plan`)".dimmed()
    );

    Ok(())
}

pub(crate) fn session_manifest_mark(
    spec_id: &str,
    session_query: Option<&str>,
    kind: MarkKind,
) -> Result<()> {
    let project_root = find_project_root()?;
    let lease = resolve_manifest_target_lease(&project_root, session_query)?;
    let path = session_manifest::manifest_path(&project_root, &lease.id);
    let updated = match kind {
        MarkKind::Started => session_manifest::mark_started(&path, spec_id)?,
        MarkKind::Completed => session_manifest::mark_completed(&path, spec_id)?,
    };
    let action = match kind {
        MarkKind::Started => "started",
        MarkKind::Completed => "completed",
    };
    if updated {
        println!(
            "{} marked {} as {} in session {} manifest",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            spec_id.bold(),
            action,
            (&lease.id[..lease.id.len().min(8)]).dimmed()
        );
    } else if !path.exists() {
        // Quiet no-op: most edits happen outside a planned cluster. We
        // emit a single dim line so manual `aida session manifest
        // mark-started` calls aren't silently lost, but we don't shout
        // when the hook path runs against every edit.
        println!(
            "{} no manifest for session {} — nothing to mark (run `aida session manifest write` first)",
            "→".dimmed(),
            (&lease.id[..lease.id.len().min(8)]).dimmed()
        );
    } else {
        println!(
            "{} {} not in session {} manifest — not marking",
            "→".dimmed(),
            spec_id,
            (&lease.id[..lease.id.len().min(8)]).dimmed()
        );
    }
    Ok(())
}

/// True when one SHA is a (case-insensitive, hex) prefix of the other.
/// Used by statusline to compare a cache-stored SHA (potentially full,
/// 40 chars) against the current `git rev-parse --short HEAD` output (7
/// chars) without flagging a spurious stale.
/// trace:TASK-1-045 | ai:claude
pub(crate) fn sha_prefix_match(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return a == b;
    }
    let min = a.len().min(b.len());
    a[..min].eq_ignore_ascii_case(&b[..min])
}

/// STORY-42: shell out to `aida db sync --pull`. We could refactor the
/// inline implementation in main(), but it's deeply tangled with the
/// dispatch and backend init — a child-process invocation is cheap
/// (sub-second when there's nothing to fetch) and lets queue work
/// reuse the canonical sync flow verbatim (conflict detection, cache
/// rebuild, the works). trace:STORY-42 | ai:claude
pub(crate) fn run_aida_db_sync_pull(store_path: &std::path::Path) -> Result<()> {
    eprintln!(
        "  {} pulling orphan-store via `aida db sync --pull` ...",
        "↻".cyan()
    );
    let mut cmd = std::process::Command::new(aida_exe_path());
    cmd.args(["db", "sync", "--pull"]);
    // Force the child to look at the same store as us — avoids ambiguity
    // when queue work runs from a sibling worktree.
    let _ = store_path; // signature future-proofing; current binary
                        // resolves the store via cwd's project root,
                        // which matches ours.
    let status = cmd
        .status_retrying_etxtbsy()
        .with_context(|| "spawn `aida db sync --pull`")?;
    if !status.success() {
        anyhow::bail!("`aida db sync --pull` exited with {}", status);
    }
    Ok(())
}

/// STORY-42: write the session manifest from a queue-work plan so
/// /aida-pickup can walk the cluster top-down on launch. Items carry
/// the spec_id + status_at_plan that the resolver already computed,
/// so this function does not re-load the store.
/// trace:STORY-42 STORY-98 | ai:claude
pub(crate) fn write_queue_work_manifest(
    project_root: &std::path::Path,
    lease: &SessionLease,
    plan: &QueueWorkPlan,
    plan_context: Option<session_manifest::PlanContext>,
    claude_session_id: Option<String>,
    batch_name: Option<&str>,
) -> Result<()> {
    use crate::session_manifest::{ManifestItem, SessionManifest};

    let items: Vec<ManifestItem> = plan
        .entries
        .iter()
        .enumerate()
        .map(|(idx, e)| ManifestItem {
            spec_id: e.spec_id.clone(),
            position: (idx + 1) as u32,
            status_at_plan: e.status_at_plan.clone(),
            started_at: None,
            completed_at: None,
            note: None,
        })
        .collect();
    let manifest = SessionManifest {
        session_id: lease.id.clone(),
        planned_at: chrono::Utc::now(),
        plan_source: "queue work".to_string(),
        claude_session_id,
        // TASK-272: record the batch when set up via `aida queue work
        // --batch NAME` so /aida-pickup detects batch context.
        batch_name: batch_name.map(str::to_string),
        plan: plan_context,
        items,
    };
    let path = session_manifest::manifest_path(project_root, &lease.id);
    session_manifest::save(&path, &manifest)?;
    Ok(())
}

/// TASK-112: how `aida queue work` should launch claude — a cold launch
/// with a freshly-minted session id, or a resume of a recorded one.
/// trace:TASK-112 | ai:claude
#[derive(Debug)]
pub(crate) enum QueueWorkLaunch {
    /// Cold launch; the `String` is a freshly-minted UUID passed as
    /// `claude --session-id` so the new conversation is itself resumable.
    Fresh(String),
    /// Resume a recorded conversation by its claude session id.
    Resume(String),
}

impl QueueWorkLaunch {
    pub(crate) fn session_id(&self) -> &str {
        match self {
            QueueWorkLaunch::Fresh(id) | QueueWorkLaunch::Resume(id) => id,
        }
    }
}

/// TASK-402: does `s` look like an AIDA *lease* id rather than a Claude
/// *session* UUID? A lease id is a hyphenless hex run (e.g. `019e45cfc559`,
/// the short form printed by `session … started` check line); a Claude session UUID
/// always carries hyphens (`019e45cf-acea-73c1-9476-…`). The orchestrator's
/// phase-1 banner prominently shows the lease id, so pasting *that* into
/// `--resume <id>` is the #1 recovery papercut — it never prefix-matches a
/// recorded session. Detect the shape so we can point at the right id form
/// instead of the generic "no recorded session" miss. Pure + heuristic.
/// trace:TASK-402 | ai:claude
pub(crate) fn looks_like_lease_id(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('-')
        && (8..=16).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// TASK-112: resolve a (possibly truncated) session id against the
/// recorded sessions for a scope. Returns the full id on a unique match;
/// a non-matching value of UUID-ish length is passed through verbatim
/// (the user pasted a full id we didn't index). trace:TASK-112 | ai:claude
pub(crate) fn resolve_resume_id(recorded: &[String], requested: &str) -> Result<String> {
    let matches: Vec<&String> = recorded
        .iter()
        .filter(|id| id.starts_with(requested))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 if requested.len() >= 16 => Ok(requested.to_string()),
        // TASK-402 (friction #1): the pasted value has the shape of an AIDA
        // lease id, not a Claude session UUID. The banner that advertised
        // `session <lease> started` check line is the trap — `--resume` wants the
        // Claude session UUID. Name the mismatch and point at the right id.
        // trace:TASK-402 | ai:claude
        0 if looks_like_lease_id(requested) => anyhow::bail!(
            "`{}` looks like an AIDA lease id, but --resume needs the Claude \
             session UUID (e.g. 019e45cf-acea-73c1-…) — run \
             `aida queue work <scope> --list-sessions` to see the resumable \
             session ids (or pass bare `--resume` to take the most recent)",
            requested
        ),
        0 => anyhow::bail!(
            "no recorded claude session matches `{}` — run \
             `aida queue work <scope> --list-sessions` to see recorded ids",
            requested
        ),
        n => anyhow::bail!(
            "{} recorded sessions match `{}` — use a longer prefix",
            n,
            requested
        ),
    }
}

/// TASK-112: decide fresh-vs-resume for `aida queue work`. Called before
/// the worktree is created so a bad `--resume` fails clean.
///   * `--session-id <uuid>` → cold launch with that caller-minted id
///     (STORY-132 — the TUI tracks the conversation deterministically)
///   * `--fresh`           → cold launch, new minted id
///   * `--resume <id>`     → resume that id (prefix-matched)
///   * bare `--resume`     → resume the most recent recorded session
///   * neither, prior exists, interactive → prompt
///   * neither, otherwise  → cold launch
///     trace:TASK-112, STORY-132 | ai:claude
pub(crate) fn resolve_queue_work_launch(
    scope: &str,
    resume: Option<&str>,
    fresh: bool,
    session_id_override: Option<&str>,
) -> Result<QueueWorkLaunch> {
    use std::io::IsTerminal;

    let mint_fresh = || QueueWorkLaunch::Fresh(uuid::Uuid::now_v7().to_string());

    // STORY-132: a caller-minted session id is a fresh cold launch with
    // that exact UUID — validated up front so a malformed id fails clean
    // before any worktree side effects. Clap already makes it mutually
    // exclusive with `--resume`.
    if let Some(sid) = session_id_override {
        uuid::Uuid::parse_str(sid)
            .with_context(|| format!("--session-id `{}` is not a valid UUID", sid))?;
        return Ok(QueueWorkLaunch::Fresh(sid.to_string()));
    }

    if fresh {
        return Ok(mint_fresh());
    }

    let prior = session::list_scope_sessions(scope).unwrap_or_default();

    match resume {
        // Explicit `--resume <id>`.
        Some(id) if !id.is_empty() => {
            let recorded: Vec<String> = prior.iter().map(|m| m.id.clone()).collect();
            Ok(QueueWorkLaunch::Resume(resolve_resume_id(&recorded, id)?))
        }
        // Bare `--resume` — most recent recorded session for the scope.
        Some(_) => {
            let m = prior.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "no recorded claude session for scope `{}` to resume — \
                     drop --resume (or pass --fresh) to cold-launch",
                    scope
                )
            })?;
            Ok(QueueWorkLaunch::Resume(m.id.clone()))
        }
        // Default: prompt when prior sessions exist and we're
        // interactive; cold-launch otherwise (non-interactive /
        // autonomous runs default to fresh — no blocking prompt).
        None => {
            if prior.is_empty() || !std::io::stdin().is_terminal() {
                return Ok(mint_fresh());
            }
            let fresh_label = "○  start a fresh session".to_string();
            let mut labels: Vec<String> = vec![fresh_label.clone()];
            for m in prior.iter().take(8) {
                labels.push(session::format_session_line(m));
            }
            let pick = inquire::Select::new(
                &format!(
                    "{} prior session(s) for {} — resume one?",
                    prior.len(),
                    scope
                ),
                labels.clone(),
            )
            .with_help_message("Use arrow keys to move, Enter to choose")
            .prompt()
            .context("session-resume picker cancelled")?;
            if pick == fresh_label {
                Ok(mint_fresh())
            } else {
                let idx = labels
                    .iter()
                    .position(|l| l == &pick)
                    .map(|p| p.saturating_sub(1))
                    .unwrap_or(0);
                Ok(QueueWorkLaunch::Resume(prior[idx].id.clone()))
            }
        }
    }
}

/// TASK-112: `aida queue work <scope> --list-sessions` — print recorded
/// conversations for the scope, newest first.
// trace:TASK-112
pub(crate) fn print_scope_sessions(scope: &str) -> Result<()> {
    let sessions = session::list_scope_sessions(scope)?;
    if sessions.is_empty() {
        println!(
            "{}",
            format!("(no recorded sessions for scope `{}`)", scope).dimmed()
        );
        println!(
            "  {}",
            "a session is recorded the first time `aida queue work` launches an agent for this scope"
                .dimmed()
        );
        return Ok(());
    }
    println!(
        "{} session(s) for {} (most recent first):",
        sessions.len(),
        scope.cyan()
    );
    for m in &sessions {
        println!("  {}", session::format_session_line(m));
    }
    println!();
    // TASK-402 (friction #1 + #5): make the resume hint paste-ready. Show the
    // *full* Claude session UUID (the id `--resume` actually needs — not the
    // AIDA lease id the kickoff banner shows, and not the 8-char prefix in the
    // table above) and, when the session's recorded worktree differs from the
    // current cwd, prepend the `cd <worktree>` step so a headless session
    // launched in a sibling worktree is resumable in one paste.
    // trace:TASK-402 | ai:claude
    let most_recent = &sessions[0];
    let resume_cmd = paste_ready_resume_command(scope, most_recent);
    println!("  {}", "resume the most recent:".dimmed());
    println!("    {}", resume_cmd.cyan());
    println!();
    println!(
        "  {}",
        format!(
            "(or `aida queue work {} --resume <full-session-uuid>` for a specific one above)",
            scope
        )
        .dimmed()
    );
    Ok(())
}

/// TASK-402: assemble a single paste-ready resume command for a recorded
/// session. Uses the FULL session UUID (`--resume` rejects the AIDA lease id
/// and a truncated prefix can be ambiguous) and, when the session's recorded
/// worktree is known and is not the current cwd, prepends `cd <worktree> && `
/// so a headless session launched in a sibling worktree (Claude's `--resume`
/// is project-slug scoped — invisible from the main repo's cwd) resumes in one
/// paste. Pure given the cwd argument resolved by the caller.
/// trace:TASK-402 | ai:claude
pub(crate) fn paste_ready_resume_command(scope: &str, m: &session::SessionMeta) -> String {
    let base = format!("aida queue work {} --resume {}", scope, m.id);
    let current = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    resume_command_with_cwd(&base, m.last_cwd.as_deref(), current.as_deref())
}

/// TASK-402: pure helper — prepend a `cd <worktree> && ` step when the
/// session's recorded worktree is known and differs from the current cwd.
/// Keeps the cwd-resolution side effect out so it's unit-testable.
/// trace:TASK-402 | ai:claude
pub(crate) fn resume_command_with_cwd(
    base: &str,
    worktree: Option<&str>,
    current: Option<&str>,
) -> String {
    match worktree {
        // The recorded cwd is session metadata and the line is meant to be
        // pasted into a shell: quote it. trace:BUG-1624 | ai:claude
        Some(wt) if !wt.is_empty() && Some(wt) != current => {
            format!("cd {} && {}", shell_quote(wt), base)
        }
        _ => base.to_string(),
    }
}
