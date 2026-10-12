//! worktree GC, salvage, stranded-primary detection, and worktree/pool commands (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Is this worktree one of the AIDA/Agent-tool managed isolation worktrees that
/// accumulate? Matched on EITHER the conventional path segment
/// (`.claude/worktrees/agent-<id>`) OR the conventional branch name
/// (`worktree-agent-<id>`) — both are stamped by the same launcher, and matching
/// either keeps detached or oddly-pathed cases in scope. Pure → unit-testable.
/// trace:TASK-878 | ai:claude
pub(crate) fn is_agent_managed_worktree(path: &std::path::Path, branch: Option<&str>) -> bool {
    let path_str = path.to_string_lossy().replace('\\', "/");
    let path_match = path_str.contains("/.claude/worktrees/agent-")
        || path_str.contains(".claude/worktrees/agent-");
    let branch_match = branch
        .map(|b| b.starts_with("worktree-agent-"))
        .unwrap_or(false);
    path_match || branch_match
}

/// BUG-614: lossless prune of dead worktree bookkeeping. `git worktree prune`
/// only removes administrative entries for worktrees whose directories no
/// longer exist — it never deletes a live worktree, so this is always safe.
/// Best-effort and quiet: a spawn/exec failure is swallowed (the caller treats
/// it as a non-event). trace:BUG-614 | ai:claude
pub(crate) fn lossless_prune_worktrees(project_root: &std::path::Path) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "prune"])
        .output();
}

/// BUG-614: the GC eligibility verdict for one agent-managed worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreeGcVerdict {
    /// All four safety predicates hold → safe for the operator GC to remove.
    Eligible,
    /// At least one predicate fails → PRESERVE and report the reason.
    Preserve(String),
}

/// BUG-614: pure inputs to the conservative GC predicate — every probe result
/// the safety model needs, gathered once per worktree by the scanner. Keeping
/// the predicate pure (no git/lease/process probes) makes the four safety
/// gates exhaustively unit-testable. trace:BUG-614 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct WorktreeGcFacts {
    /// True when this worktree is one of the `.claude/worktrees/agent-*` /
    /// `worktree-agent-*` isolation worktrees (the only ones the GC touches).
    pub(crate) is_agent_managed: bool,
    /// True when the branch is merged into the default branch OR is gone
    /// (detached / branch deleted). A worktree whose branch carries unmerged
    /// work is never GC'd.
    pub(crate) merged_or_gone: bool,
    /// True when the worktree has uncommitted/untracked changes. Dirty is
    /// unambiguously "has work" — never removed.
    pub(crate) dirty: bool,
    /// True when `git worktree list --porcelain` reports a `locked` line for
    /// this worktree. A locked worktree is operator-protected — never removed.
    pub(crate) locked: bool,
    /// True when a live process or an active session-lease references this
    /// worktree (a session is in flight there). Never removed.
    pub(crate) has_active_lease: bool,
}

/// BUG-614: the conservative GC predicate. Removal requires ALL of:
/// agent-managed AND merged-or-gone AND clean AND unlocked AND no active lease.
/// Any failing gate PRESERVES the worktree with a human-readable reason. The
/// order is deliberate: cheapest / most-protective signals first so the reason
/// names the strongest objection. Default-to-preserve: an unrecognized state
/// is kept, never removed. trace:BUG-614 | ai:claude
pub(crate) fn classify_worktree_gc(facts: &WorktreeGcFacts) -> WorktreeGcVerdict {
    if !facts.is_agent_managed {
        return WorktreeGcVerdict::Preserve("not an agent-managed worktree".to_string());
    }
    // Dirty first — uncommitted work is the costliest thing to lose.
    if facts.dirty {
        return WorktreeGcVerdict::Preserve(
            "uncommitted changes present — never auto-removed".to_string(),
        );
    }
    if facts.locked {
        return WorktreeGcVerdict::Preserve("worktree is locked — operator-protected".to_string());
    }
    if facts.has_active_lease {
        return WorktreeGcVerdict::Preserve(
            "an active session/process references it — in flight".to_string(),
        );
    }
    if !facts.merged_or_gone {
        return WorktreeGcVerdict::Preserve(
            "branch is not merged into the default branch — unmerged work".to_string(),
        );
    }
    WorktreeGcVerdict::Eligible
}

/// BUG-614: the set of worktree paths (canonicalized) that a live process or an
/// active session-lease references. Two sources, unioned:
///   1. session leases whose `worktree_path` resolves to a live state
///      ([`lease_state_for`] == Live) — a session is genuinely in flight there;
///   2. live `claude` processes whose cwd sits inside a worktree (covers
///      Agent-tool worktrees that carry no AIDA lease).
/// A path appearing in either set is "active" and must never be GC'd.
/// trace:BUG-614 | ai:claude
pub(crate) fn active_worktree_paths(project_root: &std::path::Path) -> HashSet<std::path::PathBuf> {
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let mut out: HashSet<std::path::PathBuf> = HashSet::new();
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    // (1) live session leases.
    for lease in list_leases(project_root) {
        if lease.worktree_path.as_os_str().is_empty() {
            continue;
        }
        if matches!(lease_state_for(&lease, &live, now), LeaseState::Live) {
            out.insert(canon(&lease.worktree_path));
        }
    }
    // (2) live claude processes whose cwd is a (non-stale) real directory.
    for s in &live {
        if s.stale_cwd {
            continue;
        }
        out.insert(canon(&s.cwd));
    }
    out
}

/// BUG-614: does any active worktree path equal `wt` or sit beneath it? A live
/// process one level down still pins the worktree. trace:BUG-614 | ai:claude
pub(crate) fn worktree_is_active(
    wt: &std::path::Path,
    active: &HashSet<std::path::PathBuf>,
) -> bool {
    let wt_canon = wt.canonicalize().unwrap_or_else(|_| wt.to_path_buf());
    active
        .iter()
        .any(|a| *a == wt_canon || a.starts_with(&wt_canon))
}

/// Is this worktree locked? Parses `git worktree list --porcelain -z`
/// and reports whether the record for `wt` carries a `locked` field (git emits
/// `locked` with an optional reason after it). `-z` terminates each attribute
/// with NUL and each entry with an extra NUL, correctly handling worktree paths
/// containing newlines. Read-only; false on any git failure so a probe error
/// never makes us treat a worktree as removable.
// trace:BUG-614 trace:TASK-1543 | ai:antigravity
pub(crate) fn worktree_is_locked(project_root: &std::path::Path, wt: &std::path::Path) -> bool {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "list", "--porcelain", "-z"])
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(out) = out else { return false };
    if !out.status.success() {
        return false;
    }
    let wt_canon = wt.canonicalize().unwrap_or_else(|_| wt.to_path_buf());
    let text = String::from_utf8_lossy(&out.stdout);
    parse_worktree_lock_states_z(&text)
        .into_iter()
        .any(|(rec, locked)| {
            locked && {
                let rec_canon = rec.canonicalize().unwrap_or(rec);
                rec_canon == wt_canon
            }
        })
}

/// Pure parser for `git worktree list --porcelain -z`: one `(path, locked)` pair
/// per registered worktree, in git's own order. `-z` terminates every attribute
/// with NUL and every record with an extra NUL, so a worktree path containing
/// newlines stays a single field — which is exactly the case a line-oriented
/// parser gets wrong. Split out from [`worktree_is_locked`] so the NUL framing
/// is testable on every platform, including ones whose filesystem cannot hold a
/// newline in a path at all (Windows).
// trace:TASK-1543 | ai:claude
pub(crate) fn parse_worktree_lock_states_z(text: &str) -> Vec<(std::path::PathBuf, bool)> {
    let mut out: Vec<(std::path::PathBuf, bool)> = Vec::new();
    // Index of the record the fields currently belong to; cleared by the empty
    // field that terminates each record.
    let mut cur: Option<usize> = None;
    for field in text.split('\0') {
        if let Some(p) = field.strip_prefix("worktree ") {
            out.push((std::path::PathBuf::from(p), false));
            cur = Some(out.len() - 1);
        } else if field.is_empty() {
            cur = None;
        } else if field == "locked" || field.starts_with("locked ") {
            if let Some(i) = cur {
                out[i].1 = true;
            }
        }
    }
    out
}

/// BUG-614: `aida session gc` — the explicit operator GC of stale agent
/// worktrees. Scans every `.claude/worktrees/agent-*` worktree, gathers the
/// four safety facts, and (unless `--dry-run`) runs `git worktree remove` (NOT
/// `--force`) on the ones the pure predicate calls Eligible. Anything preserved
/// is reported with its reason; a worktree git itself refuses to remove is
/// SKIPPED (never force-removed) and reported. Always runs the lossless prune
/// first. Never invoked on the hot `aida status` read path — only on this
/// explicit operator command. trace:BUG-614 | ai:claude
pub(crate) fn session_gc(dry_run: bool, yes: bool) -> Result<()> {
    let project_root = find_project_root()?;

    // Lossless first — clears dead bookkeeping cheaply regardless of the GC.
    lossless_prune_worktrees(&project_root);

    let default_ref = resolve_default_branch_ref(&project_root);
    let project_canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let active = active_worktree_paths(&project_root);

    let git_count = |args: &[&str]| -> Option<u32> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
            })
    };

    let mut eligible: Vec<(std::path::PathBuf, Option<String>)> = Vec::new();
    let mut preserved: Vec<(std::path::PathBuf, String)> = Vec::new();

    for wt in list_worktrees(&project_root) {
        let wt_canon = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
        // Never touch the main worktree or the store worktree.
        if wt_canon == project_canon || wt.branch.as_deref() == Some("aida-store") {
            continue;
        }
        let is_agent_managed = is_agent_managed_worktree(&wt.path, wt.branch.as_deref());
        if !is_agent_managed {
            continue;
        }

        // merged_or_gone: a detached/branchless agent worktree counts as "gone"
        // (no branch ⇒ nothing unmerged to lose); a branch is merged when it has
        // zero commits beyond the default ref. Without a resolvable default ref
        // we cannot prove merged-ness — treat as NOT merged (preserve).
        let merged_or_gone = match (wt.branch.as_deref(), default_ref.as_deref()) {
            (None, _) => true,
            (Some(branch), Some(default_ref)) => {
                git_count(&["rev-list", "--count", &format!("{default_ref}..{branch}")])
                    .map(|n| n == 0)
                    .unwrap_or(false)
            }
            (Some(_), None) => false,
        };

        let facts = WorktreeGcFacts {
            is_agent_managed,
            merged_or_gone,
            dirty: !worktree_dirty_entries(&wt.path).is_empty(),
            locked: worktree_is_locked(&project_root, &wt.path),
            has_active_lease: worktree_is_active(&wt.path, &active),
        };

        match classify_worktree_gc(&facts) {
            WorktreeGcVerdict::Eligible => eligible.push((wt.path.clone(), wt.branch.clone())),
            WorktreeGcVerdict::Preserve(reason) => preserved.push((wt.path.clone(), reason)),
        }
    }

    if eligible.is_empty() && preserved.is_empty() {
        println!("No agent-managed worktrees found — nothing to GC.");
        return Ok(());
    }

    if !preserved.is_empty() {
        println!(
            "Preserved {} worktree(s) (kept by design):",
            preserved.len()
        );
        for (path, reason) in &preserved {
            println!("  {} {} — {}", "keep".yellow(), path.display(), reason);
        }
    }

    if eligible.is_empty() {
        println!("No worktree is GC-eligible (merged + clean + unlocked + no active lease).");
        return Ok(());
    }

    println!(
        "{} agent worktree(s) are GC-eligible (merged + clean + unlocked + no active lease):",
        eligible.len()
    );
    for (path, branch) in &eligible {
        match branch {
            Some(b) => println!("  {} {} (branch `{}`)", "remove".cyan(), path.display(), b),
            None => println!("  {} {} (detached)", "remove".cyan(), path.display()),
        }
    }

    if dry_run {
        println!("\n--dry-run: no worktrees removed. Re-run without --dry-run to GC them.");
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        eprint!(
            "\nRemove the {} eligible worktree(s)? [y/N] ",
            eligible.len()
        );
        std::io::stderr().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted — nothing removed.");
            return Ok(());
        }
    }

    let mut removed = 0usize;
    let mut skipped = 0usize;
    for (path, _branch) in &eligible {
        // A live process inside the tree (cwd, open file, running program)
        // keeps it: gc never removes a worktree that is in use, and has no
        // override. trace:BUG-1947 | ai:claude
        if let Err(in_use) = aida_core::worktree_holders::check_worktree_removal(path) {
            skipped += 1;
            println!(
                "  {} {} — kept: {}",
                "skip".yellow(),
                path.display(),
                in_use.summary()
            );
            continue;
        }
        // `git worktree remove` (NOT --force): git re-checks the worktree is
        // clean and unlocked and refuses otherwise — a second guard on top of
        // our predicate. On refusal we SKIP (never force) and report.
        // trace:BUG-614 | ai:claude
        let res = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args(["worktree", "remove"])
            .arg(path)
            .output();
        match res {
            Ok(o) if o.status.success() => {
                removed += 1;
                println!(
                    "  {} removed {}",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    path.display()
                );
            }
            Ok(o) => {
                skipped += 1;
                let err = String::from_utf8_lossy(&o.stderr);
                println!(
                    "  {} git refused to remove {} — kept (not forced): {}",
                    "skip".yellow(),
                    path.display(),
                    err.trim()
                );
            }
            Err(e) => {
                skipped += 1;
                println!(
                    "  {} could not spawn git for {} — kept: {}",
                    "skip".yellow(),
                    path.display(),
                    e
                );
            }
        }
    }

    // Lossless tidy of any bookkeeping the removals left behind.
    lossless_prune_worktrees(&project_root);

    println!(
        "\nGC complete: {} removed, {} skipped, {} preserved.",
        removed,
        skipped,
        preserved.len()
    );
    Ok(())
}

pub(crate) fn criterion_trace_suffix(suffix: &str) -> bool {
    if suffix.is_empty()
        || !suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return false;
    }
    let upper = suffix.to_ascii_uppercase();
    let Some(rest) = upper.strip_prefix("AC") else {
        return upper
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
            && upper.chars().skip(1).all(|c| c.is_ascii_digit());
    };
    if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    matches!(rest.len(), 5 | 6) && rest.chars().all(|c| c.is_ascii_hexdigit())
}

/// Resolve a `--since` value to a UTC cutoff: first parse it with the shared
/// time-bound grammar (a relative duration, an ISO date at local midnight, a
/// zone-less ISO datetime, or RFC3339); failing that, try it as a git
/// ref/tag and take that commit's committer date. The grammar goes first so
/// a duration such as `500d` is never read as an abbreviated commit ID. An
/// error names `--since` when the value resolves to neither, when it matches
/// the grammar but cannot be resolved (a DST gap/overlap, an out-of-range
/// duration), or when it starts with `-` and so could reach git as an option.
/// trace:TASK-673 | ai:claude
// trace:TASK-1509 | ai:claude
// trace:BUG-1622 | ai:claude
pub(crate) fn resolve_completed_since_cutoff(
    project_root: &std::path::Path,
    since: &str,
) -> Result<chrono::DateTime<chrono::Utc>> {
    resolve_completed_since_cutoff_at(project_root, since, chrono::Utc::now(), &chrono::Local)
}

/// [`resolve_completed_since_cutoff`] against an explicit `now` and timezone.
// trace:TASK-1509 | ai:claude
// trace:BUG-1622 | ai:claude
pub(crate) fn resolve_completed_since_cutoff_at<Tz: chrono::TimeZone>(
    project_root: &std::path::Path,
    since: &str,
    now: chrono::DateTime<chrono::Utc>,
    tz: &Tz,
) -> Result<chrono::DateTime<chrono::Utc>> {
    use std::process::Command as PCmd;
    let since = since.trim();
    // trace:BUG-1622 | ai:claude
    if since.is_empty() {
        anyhow::bail!("invalid --since value: it is empty");
    }
    match queue_cmd::parse_since_arg_at(since, now, tz) {
        Ok(t) => return Ok(t),
        Err(e) if queue_cmd::is_definitive_time_bound_error(&e) => {
            anyhow::bail!("invalid --since value: {e}")
        }
        Err(_) => {}
    }
    // Two guards before the value reaches git: refuse a leading dash, and
    // put `--end-of-options` ahead of the revision so git never parses it
    // as an option (e.g. `--output=<path>`). trace:BUG-1622 | ai:claude
    git_arg_guard::reject_option_like("--since", since)?;
    let resolved = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "log",
            "-1",
            "--format=%cI",
            git_arg_guard::END_OF_OPTIONS,
            since,
            "--",
        ])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            chrono::DateTime::parse_from_rfc3339(&s).ok()
        })
        .map(|dt| dt.with_timezone(&chrono::Utc));
    resolved.ok_or_else(|| {
        anyhow::anyhow!(
            "invalid --since value `{since}`: not a relative duration (e.g. `7d`, `2w`, \
             `24 hours ago`), an ISO date (`YYYY-MM-DD`, local midnight), a zone-less ISO \
             datetime (local time), RFC3339, or a known git ref/tag"
        )
    })
}

/// One-line bubblewrap (`bwrap`) OS-sandbox availability status, shared by
/// `aida doctor` and `aida init`. Reports AVAILABILITY only — it does not
/// enable `os_wrap` (the `[contained] os_wrap` knob is a separate concern).
/// trace:TASK-865 | ai:claude
pub(crate) fn bwrap_status_line() -> String {
    match crate::session::bwrap_availability() {
        crate::session::BwrapAvailability::Ok => {
            "bwrap: OK (userns confinement available)".to_string()
        }
        crate::session::BwrapAvailability::NotInstalled => "bwrap: not installed".to_string(),
        crate::session::BwrapAvailability::UsernsBlocked { hint } => {
            format!("bwrap: installed but userns blocked — {hint}")
        }
    }
}

pub(crate) fn salvage_worktree_patch(
    project_root: &std::path::Path,
    spec_id: &str,
    agent: Option<&str>,
    worktree: &std::path::Path,
) -> Result<Option<std::path::PathBuf>> {
    if !worktree.exists() || worktree_dirty_entries(worktree).is_empty() {
        return Ok(None);
    }
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H%M%SZ");
    let agent = agent.unwrap_or("unknown");
    let filename = format!(
        "{}-{}-attempt-{}.patch",
        sanitize_salvage_component(spec_id),
        sanitize_salvage_component(agent),
        stamp
    );
    let dir = project_root.join(".aida").join("salvage");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(filename);
    let mut body = String::new();
    body.push_str(&format!(
        "# AIDA salvage patch\n# worktree: {}\n\n",
        worktree.display()
    ));
    body.push_str(&git_output_lossy(worktree, &["diff", "--binary", "HEAD"]));
    body.push_str(&git_output_lossy(
        worktree,
        &["diff", "--binary", "--cached"],
    ));
    let untracked = git_output_lossy(worktree, &["ls-files", "--others", "--exclude-standard"]);
    if !untracked.trim().is_empty() {
        // BUG-696: index the untracked files as comments AND capture their
        // CONTENT below — previously only the names were recorded, so an
        // orphan-worktree's untracked files (new, never `git add`ed) were lost
        // when `aida doctor --heal` tore the worktree down after salvage.
        body.push_str("\n# Untracked files captured below as new-file diffs:\n");
        for line in untracked.lines() {
            body.push_str("#   ");
            body.push_str(line);
            body.push('\n');
        }
        // `git diff --no-index --binary /dev/null <file>` emits an appliable
        // "new file" hunk (binary-safe) with no index mutation. It exits
        // non-zero because the two sides differ — `git_output_lossy` keeps
        // stdout and ignores the status, so on any platform where the idiom
        // is unsupported the salvage simply degrades to the name-only index
        // above rather than failing the heal. trace:BUG-696 | ai:claude
        for line in untracked.lines() {
            let f = line.trim();
            if f.is_empty() {
                continue;
            }
            body.push_str(&git_output_lossy(
                worktree,
                // `--` so an untracked file named like an option stays a
                // path. trace:BUG-1622 | ai:claude
                &["diff", "--no-index", "--binary", "--", "/dev/null", f],
            ));
        }
    }
    if body.trim().is_empty() {
        return Ok(None);
    }
    std::fs::write(&path, body)?;
    Ok(Some(path))
}

pub(crate) fn git_output_lossy(worktree: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output();
    match out {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(_) => String::new(),
    }
}

pub(crate) fn humanize_duration_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub(crate) fn sanitize_salvage_component(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').to_string()
}

/// STORY-70: a STORY or BUG description should carry an acceptance
/// section that STORY-67's review-prompt generator can lift verbatim.
/// Returns true when the requirement's type is in the lint scope and
/// its description doesn't carry one of the recognized headings.
// trace:STORY-70 | ai:claude
pub(crate) fn requirement_missing_acceptance(req: &aida_core::Requirement) -> bool {
    matches!(
        req.req_type,
        aida_core::RequirementType::Story | aida_core::RequirementType::Bug
    ) && extract_acceptance_section(&req.description).is_none()
}

/// TASK-475: how many commits the local `aida-store` branch is behind
/// `origin/aida-store`, using already-known refs (no network fetch). `None`
/// when the origin ref is unknown (never fetched), git is unavailable, or the
/// range doesn't resolve; `Some(0)` when up-to-date. trace:TASK-475 | ai:claude
pub(crate) fn orphan_store_behind_count(project_root: &std::path::Path) -> Option<u32> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-list",
            "--count",
            "refs/heads/aida-store..refs/remotes/origin/aida-store",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u32>()
        .ok()
}

#[cfg(test)]
#[path = "tests/task_475_store_behind_tests.rs"]
mod task_475_store_behind_tests;

/// Render the cwd-unreadable warning, given the result of a `getcwd()`
/// probe. Pure, so the unreadable-cwd state is regression-testable
/// without a process-global `chdir`: a worktree that `aida session end`
/// removed makes `std::env::current_dir()` return `Err(ENOENT)`, and a
/// dropped network share / permission change yields other `ErrorKind`s —
/// all of which are the `Err` input here. `None` means the cwd is healthy
/// and nothing should be printed.
///
/// BUG-566: the message must surface the REAL failure — the underlying
/// `io::Error` (its `ErrorKind`) — and stay cause-NEUTRAL rather than
/// asserting `aida session end` removed a worktree. `current_dir()` can
/// Err for several reasons that the old single-cause message conflated:
/// a worktree removed by `aida session end`, an unmounted/unavailable
/// network share (ESTALE / EIO), a plainly-deleted directory (ENOENT),
/// or a permissions change (EACCES). On platforms where `current_dir()`
/// returns the offending path in the error we surface it too.
/// trace:BUG-108 trace:BUG-566 | ai:claude
pub(crate) fn cwd_removed_warning(cwd: &std::io::Result<std::path::PathBuf>) -> Option<String> {
    // trace:BUG-566 | ai:claude
    let err = match cwd {
        Ok(_) => return None,
        Err(e) => e,
    };
    // Human-readable rendering of the OS error (kind + message), e.g.
    // "entity not found" / "stale file handle" / "permission denied".
    let reason = err.to_string();
    Some(format!(
        "{} cannot read the current directory: {reason}.\n  This usually \
         means one of: a worktree removed by `aida session end`, an \
         unmounted or unavailable network share, a directory that was \
         deleted, or a permissions change.\n  AIDA can't resolve a project \
         from a directory it can't read, so any queue / list output below \
         is an empty fallback, not the real state.\n  {} cd to a readable \
         directory (e.g. your project's main repo) and retry.",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
        "→".cyan(),
    ))
}

/// BUG-108: warn — once, up front — when the shell's working directory
/// was removed out from under it, typically a worktree that `aida
/// session end` deleted in another terminal. Without this, every
/// project-root walk silently falls back to empty state and `aida queue
/// list` prints "Your queue is empty" indistinguishably from a genuinely
/// empty queue. Returns true when the warning fired.
/// trace:BUG-108 | ai:claude
pub(crate) fn warn_if_cwd_removed() -> bool {
    match cwd_removed_warning(&std::env::current_dir()) {
        Some(msg) => {
            eprintln!("{msg}");
            true
        }
        None => false,
    }
}

/// Resolve the MAIN worktree path given a starting checkout (which may
/// itself be a linked worktree). When the caller is inside a linked
/// worktree (created via `git worktree add`), `find_project_root()`
/// returns the LINKED worktree's path because the climb stops at the
/// first `.git` (file or dir). Session-start path / branch derivation
/// needs the canonical project root regardless of cwd, otherwise new
/// sessions stack as `~/ai/aida-pr-9-epic-20` instead of landing as
/// siblings under `~/ai/aida`. trace:BUG-75 | ai:claude
///
/// Uses `git worktree list --porcelain`; the first record is always the
/// main worktree. Falls back to `start` when git can't be invoked.
pub(crate) fn main_worktree_root_from(start: &std::path::Path) -> std::path::PathBuf {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(start)
        .args(["worktree", "list", "--porcelain"])
        .output();
    if let Ok(o) = out {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    return std::path::PathBuf::from(p);
                }
            }
        }
    }
    start.to_path_buf()
}

/// Convenience wrapper that walks up from cwd. Same fallback semantics.
pub(crate) fn find_main_worktree_root() -> Result<std::path::PathBuf> {
    let start = find_project_root()?;
    Ok(main_worktree_root_from(&start))
}

/// Resolve the project's default branch ref (e.g. `origin/main` or
/// `origin/master`). Used by session_start to fork new branches from
/// origin's mainline regardless of cwd's HEAD. trace:BUG-76 | ai:claude
///
/// Resolution order:
/// 1. `git symbolic-ref refs/remotes/origin/HEAD` if it resolves
/// 2. The first of `origin/main`, `origin/master`, `main`, `master` that
///    `git rev-parse --verify` succeeds against
///    Returns None if no reasonable default is detectable (e.g. no remotes,
///    no main/master locally).
pub(crate) fn detect_default_branch_ref(project_root: &std::path::Path) -> Option<String> {
    let try_cmd = |args: &[&str]| -> Option<String> {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()?;
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
        None
    };

    if let Some(s) = try_cmd(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        return Some(s);
    }
    for c in &["origin/main", "origin/master", "main", "master"] {
        if try_cmd(&["rev-parse", "--verify", c]).is_some() {
            return Some((*c).to_string());
        }
    }
    None
}

/// STORY-335: read-only forecast of whether `branch` rebases cleanly onto
/// `base_ref`, via `git merge-tree --write-tree` (no worktree mutation, no
/// merge/rebase performed). git exits 0 = clean, 1 = conflict (paths listed),
/// anything else (old git, missing branch, error) → Unknown — never guessed.
/// trace:STORY-335 | ai:claude
pub(crate) fn forecast_rebase_onto(
    project_root: &std::path::Path,
    base_ref: &str,
    branch: &str,
) -> integrate::RebaseForecast {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            git_arg_guard::END_OF_OPTIONS, // trace:BUG-1622 | ai:claude
            base_ref,
            branch,
        ])
        .output();
    match out {
        Ok(o) => match o.status.code() {
            Some(0) => integrate::RebaseForecast::Clean,
            Some(1) => integrate::RebaseForecast::Conflict(
                integrate::parse_merge_tree_conflict_files(&String::from_utf8_lossy(&o.stdout)),
            ),
            _ => integrate::RebaseForecast::Unknown(
                String::from_utf8_lossy(&o.stderr).trim().to_string(),
            ),
        },
        Err(e) => integrate::RebaseForecast::Unknown(e.to_string()),
    }
}

/// Return the branch name currently checked out at `path` (its HEAD).
/// `None` if detached or git fails. trace:BUG-76 | ai:claude
pub(crate) fn current_branch_at(path: &std::path::Path) -> Option<String> {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Pure predicate for the worktree-tangle spawn gate: would launching a fan-out
/// agent in `launch_cwd` mutate the PRIMARY checkout? True iff the resolved
/// worktree root is the SAME directory as the primary checkout root. Both paths
/// are canonicalized first (so `./x` vs `/abs/x` and symlinked forms compare
/// equal); a path that can't be canonicalized (doesn't exist yet) falls back to
/// its raw form, which still compares correctly for the equal/unequal cases.
// trace:TASK-965 | ai:claude
pub(crate) fn is_worktree_tangle(
    launch_cwd: &std::path::Path,
    primary_root: &std::path::Path,
) -> bool {
    fn norm(p: &std::path::Path) -> std::path::PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    }
    norm(launch_cwd) == norm(primary_root)
}

/// Spawn-time worktree-tangle gate. A spec-scoped fan-out launch MUST land in its
/// OWN dedicated worktree — never the primary checkout. A crewmate that runs in
/// the primary tree can `git checkout -b` there and strand the repo on a feature
/// branch, aborting the next `git merge --ff-only origin/main`. This is the
/// programmatic gate that replaces the memory-RULE-only defense. The no-spec
/// launch (which deliberately runs in the primary, no worktree resolved) is
/// exempt — only a resolved worktree is asserted against the primary root.
// trace:TASK-965 | ai:claude
pub(crate) fn assert_no_worktree_tangle(
    plan: &AgentLaunchPlan,
    primary_root: &std::path::Path,
) -> Result<()> {
    if plan.current_spec.is_some() && is_worktree_tangle(&plan.launch_cwd, primary_root) {
        anyhow::bail!(
            "refusing to launch: the resolved worktree ({}) is the PRIMARY checkout. \
             A fan-out agent must run in its OWN isolated worktree — running it in the \
             primary tree lets it switch branches there and strand the repo off the \
             default branch (the next `git merge --ff-only` would abort). \
             Run `aida session leases` to inspect, and relaunch so a dedicated worktree \
             is created for this spec.",
            plan.launch_cwd.display()
        );
    }
    Ok(())
}

/// Pure predicate for the stranded-primary alarm. The PRIMARY checkout is the
/// non-worktree root of the repo; when it sits on a NON-default branch while
/// agents hold in-flight leases, the next `git merge --ff-only origin/main` in
/// the primary will abort — the "primary stranded on a feature branch" footgun.
/// Conservative by construction: an undetectable branch or default branch, or a
/// zero lease count, never alarms (so a clean default-branch primary is silent).
// trace:TASK-965 | ai:claude
pub(crate) fn primary_stranded_on_feature_branch(
    primary_branch: Option<&str>,
    default_branch: Option<&str>,
    in_flight_lease_count: usize,
) -> bool {
    let (Some(branch), Some(default)) = (primary_branch, default_branch) else {
        return false;
    };
    in_flight_lease_count > 0 && !branch.is_empty() && !branch.eq_ignore_ascii_case(default)
}

/// Local-only default-branch NAME probe (no `gh`, no network) — safe for the
/// fast `aida status` path. Reuses [`detect_default_branch_ref`] (git
/// symbolic-ref / rev-parse only) and strips a leading `origin/`.
// trace:TASK-965 | ai:claude
pub(crate) fn local_default_branch_name(project_root: &std::path::Path) -> Option<String> {
    let r = detect_default_branch_ref(project_root)?;
    Some(r.strip_prefix("origin/").unwrap_or(&r).to_string())
}

/// Resolved facts for the stranded-primary alarm. Assembled by
/// [`detect_stranded_primary`] (git + lease reads, all local), rendered by
/// [`print_stranded_primary_banner`].
// trace:TASK-965 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct StrandedPrimary {
    pub(crate) primary_root: std::path::PathBuf,
    pub(crate) branch: String,
    pub(crate) default_branch: String,
    pub(crate) lease_count: usize,
}

/// TASK-977: count leases that are LIVE (their backing process is alive) from a
/// SINGLE already-probed `live_sessions` snapshot. The stranded-primary alarm
/// gates on THIS count, not the raw lease-file count, so a dead-holder lease (a
/// crashed agent / exited session whose stale record still sits under
/// `.aida/sessions/`) never false-alarms, and a genuinely live one is never
/// missed. Reuses [`lease_state_for`] — the exact classifier `aida ps` /
/// `aida session leases` render under the ● live glyph (worktree/claude/age for
/// session leases, `creator_pid` for review/claim advisory leases) — so the
/// alarm can't drift from the process-liveness truth those views show. Mirrors
/// [`live_lease_worktrees`]' bounded-probe contract: takes the shared snapshot
/// by reference, so it performs zero live-session `/proc` walks itself.
// trace:TASK-977 | ai:claude
pub(crate) fn live_lease_count(
    now: chrono::DateTime<chrono::Utc>,
    leases: &[SessionLease],
    live_sessions: &[process_probe::LiveSession],
) -> usize {
    leases
        .iter()
        .filter(|l| lease_state_for(l, live_sessions, now) == LeaseState::Live)
        .count()
}

/// Resolve the PRIMARY checkout from `cwd`, then return `Some` iff it is stranded
/// on a feature branch with LIVE in-flight leases. Network-free:
/// `main_worktree_root_from` + `current_branch_at` + `local_default_branch_name`
/// are local git probes, `list_leases` is a `.aida/sessions/` file scan, and the
/// TASK-977 liveness gate uses one memoized `probe_live_claude_sessions` walk —
/// no `gh`, no full store load. Only leases whose backing process is alive count
/// toward the alarm, so a dead-holder lease can't false-alarm the primary.
// trace:TASK-965 trace:TASK-977 | ai:claude
pub(crate) fn detect_stranded_primary(cwd: &std::path::Path) -> Option<StrandedPrimary> {
    let primary_root = main_worktree_root_from(cwd);
    let branch = current_branch_at(&primary_root)?;
    let default_branch = local_default_branch_name(&primary_root)?;
    // TASK-977: gate on the process-liveness probe, not the raw lease-file count.
    // ONE memoized live-session walk drives liveness for every lease.
    let now = chrono::Utc::now();
    let live_sessions = process_probe::probe_live_claude_sessions();
    let lease_count = live_lease_count(now, &list_leases(&primary_root), &live_sessions);
    if primary_stranded_on_feature_branch(Some(&branch), Some(&default_branch), lease_count) {
        Some(StrandedPrimary {
            primary_root,
            branch,
            default_branch,
            lease_count,
        })
    } else {
        None
    }
}

/// Loud, top-of-output banner when the primary checkout is stranded. Shared by
/// `aida status` (fast path) and `aida ps`.
// trace:TASK-965 | ai:claude
pub(crate) fn print_stranded_primary_banner(s: &StrandedPrimary) {
    let warn = crate::glyph(crate::glyphs::Glyph::Warning);
    println!(
        "{}",
        format!("{warn} PRIMARY CHECKOUT STRANDED ON A FEATURE BRANCH")
            .red()
            .bold()
    );
    println!(
        "  primary {} is on {} (not {}) while {} lease{} in flight.",
        s.primary_root.display(),
        s.branch.yellow().bold(),
        s.default_branch.cyan(),
        s.lease_count,
        if s.lease_count == 1 { " is" } else { "s are" },
    );
    println!(
        "{}",
        format!(
            "  the next `git merge --ff-only {}` here will ABORT — switch the primary back: \
             `git -C {} switch {}`",
            s.default_branch,
            s.primary_root.display(),
            s.default_branch,
        )
        .dimmed()
    );
    println!();
}

#[cfg(test)]
#[path = "tests/task965_worktree_tangle_tests.rs"]
mod task965_worktree_tangle_tests;

// TASK-977: the stranded-primary alarm must gate on process-liveness, not the
// raw lease-file count. These tests lock the fix at the `live_lease_count` layer
// (the pure fold `detect_stranded_primary` feeds into
// `primary_stranded_on_feature_branch`): a dead-holder lease (crashed agent /
// exited session, worktree gone or claim-holder pid dead) is NOT counted, so it
// can't false-alarm; a genuinely live lease IS counted, so it isn't missed.
#[cfg(test)]
#[path = "tests/task977_stranded_liveness_tests.rs"]
mod task977_stranded_liveness_tests;

/// Link the parent checkout's runtime AIDA state into a new worktree
/// (BUG-52). `.aida-store/` is gitignored so a whole-directory symlink works.
/// `.aida/` is partially tracked, so only its gitignored runtime entries are
/// linked. `cache.db.lock-info` is deliberately NOT linked (BUG-1644).
// trace:BUG-52 trace:BUG-1644 | ai:claude
#[cfg(unix)]
pub(crate) fn link_worktree_runtime_state(
    project_root: &std::path::Path,
    worktree_path: &std::path::Path,
) -> Result<()> {
    let store_src = project_root.join(".aida-store");
    let store_dst = worktree_path.join(".aida-store");
    if store_src.exists() && !store_dst.exists() {
        std::os::unix::fs::symlink(&store_src, &store_dst).with_context(|| {
            format!(
                "symlink {} -> {} failed",
                store_dst.display(),
                store_src.display()
            )
        })?;
    }

    let parent_aida = project_root.join(".aida");
    let worktree_aida = worktree_path.join(".aida");
    if parent_aida.exists() {
        std::fs::create_dir_all(&worktree_aida)?;
        for runtime in &[
            "sessions",
            "agents",
            "roles",
            "cache.db",
            "cache.db-shm",
            "cache.db-wal",
            "pgdata",
        ] {
            let src = parent_aida.join(runtime);
            let dst = worktree_aida.join(runtime);
            if src.exists() && !dst.exists() {
                std::os::unix::fs::symlink(&src, &dst).with_context(|| {
                    format!("symlink {} -> {} failed", dst.display(), src.display())
                })?;
            }
        }
        // BUG-1644: `cache.db.lock-info` is deliberately NOT linked. Its
        // path derives from the shared (symlink-resolved) cache location
        // (`aida_core::cache_lock_info_path`), so a reader here and a
        // writer in the main checkout already meet at one sidecar, and a
        // not-yet-created parent cache needs no dangling link. A reused
        // worktree may still hold an older binary's per-worktree sidecar;
        // retire it when its owner is dead. trace:BUG-1644 | ai:claude
        retire_stray_worktree_lock_info(&worktree_aida);
    }
    Ok(())
}

/// The runtime symlinks session start places inside a worktree's `.aida/`,
/// which [`unlink_worktree_aida_runtime`] strips before removal.
// trace:BUG-52 trace:BUG-1986 | ai:claude
pub(crate) const WORKTREE_AIDA_RUNTIME_LINKS: &[&str] = &[
    "sessions",
    "roles",
    "cache.db",
    "cache.db-shm",
    "cache.db-wal",
    "pgdata",
];

/// Strip the runtime symlinks `link_worktree_runtime_state` created inside a
/// worktree's `.aida/` (BUG-52), so `git worktree remove` doesn't count them
/// as untracked files. `.aida/` itself holds tracked content and stays.
// trace:BUG-52 trace:BUG-1644 | ai:claude
pub(crate) fn unlink_worktree_aida_runtime(aida_dir: &std::path::Path) {
    // BUG-1644: retire an older binary's per-worktree lock-info sidecar
    // (dead owner only) while `cache.db` is still a symlink, i.e. while
    // it is still distinguishable from the shared sidecar.
    // trace:BUG-1644 | ai:claude
    retire_stray_worktree_lock_info(aida_dir);
    for runtime in WORKTREE_AIDA_RUNTIME_LINKS {
        let p = aida_dir.join(runtime);
        if p.is_symlink() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// Remove a per-worktree `cache.db.lock-info` that a pre-BUG-1644 binary left
/// beside the worktree's SYMLINKED `cache.db`, but only when its recorded owner
/// is provably dead (the TASK-1484 compare-and-delete). A live owner's file is
/// left for `aida doctor` to report. No-op when `cache.db` is not a symlink:
/// the sidecar there IS the shared one.
// trace:BUG-1644 | ai:claude
pub(crate) fn retire_stray_worktree_lock_info(worktree_aida: &std::path::Path) {
    if let Some(stray) = aida_core::stray_cache_lock_info_path(&worktree_aida.join("cache.db")) {
        let _ = aida_core::reclaim_dead_lock_info(&stray);
    }
}

/// BUG-528: resolve which role queue `aida add --queue` routes the freshly-filed
/// spec to. Mirrors `aida queue add --for`'s `--for any` semantic, but the
/// *default* (no `--for`) is the `implementer` queue — the overwhelmingly common
/// target for filed work — rather than the filer's own session role. Before this,
/// `enqueue_groomed` routed by `AIDA_SESSION_ROLE`, so an advisor filing
/// implementation work with `--queue` silently landed it in the advisor queue,
/// where `burndown run` (which drains the implementer queue) would miss it.
///   * `Some("any")`  → `None` (unrouted, explicit opt-out).
///   * `Some(role)`   → `Some(canonical_role_name(role))`.
///   * `None`         → `Some("implementer")` (canonicalized).
/// trace:BUG-528 | ai:claude
pub(crate) fn add_queue_route_role(r#for: Option<&str>) -> Option<String> {
    match r#for {
        Some("any") => None,
        Some(role) => Some(canonical_role_name(role)),
        None => Some(canonical_role_name("implementer")),
    }
}

// BUG-631: is a `--for <role>` queue route a REQUEST-for-review/triage route
// (open to any role) rather than a DISPATCH-for-execution route?
//
// The advisor-authority gate (TASK-647 / ADR-3) exists to stop a non-advisor
// committing the team to BUILD work. Routing a draft to the advisor / human /
// reviewer is the opposite act: a REQUEST for review or triage. It does not
// dispatch execution and does not bypass approval — the advisor/reviewer still
// decides. So those three targets are exempt from the gate; `implementer` and
// every other (execution or unknown/custom) role stay gated. The exempt set is
// the closed list of review/triage seats: widen it deliberately, never by
// default, so an unrecognized role leans toward the safer (gated) behavior.
// trace:BUG-631 | ai:claude
pub(crate) fn for_route_is_request_only(for_role: &str) -> bool {
    // `canonical_role_name` lowercases only `human`/`dialog`; other roles pass
    // through with their original casing, so compare case-insensitively to
    // accept `--for Reviewer` / `--for ADVISOR` the same as the lowercase forms.
    let canonical = canonical_role_name(for_role).to_ascii_lowercase();
    matches!(canonical.as_str(), "advisor" | "human" | "reviewer")
}

// BUG-631: does this queue-add `--for` target require dispatch authority?
//
// TRUE (gated) for dispatch-for-execution routes — `implementer`, any
// unknown/custom role, and the unrouted cases (`--for any` or no `--for` at
// all), both of which imply execution intent. FALSE (exempt) only for the
// request/review routes classified by `for_route_is_request_only`
// (advisor / human / reviewer). This scopes the TASK-647 gate to the act it was
// meant to guard — committing the team to build — and lets `aida queue add
// --for advisor` (a request for review) succeed from any role. trace:BUG-631
pub(crate) fn for_target_requires_dispatch_authority(for_role: Option<&str>) -> bool {
    match for_role {
        Some("any") => true,
        Some(role) => !for_route_is_request_only(role),
        None => true,
    }
}

// BUG-498: An operator doing advisor work (groom / approve / queue) by
// prefixing individual commands with `AIDA_SESSION_ROLE=advisor` is acting
// as the advisor, but their persistent shell seat is unset — so the
// statusline (which renders `$AIDA_SESSION_ROLE`, defaulting to `implementer`)
// shows `implementer`, and the seat-vs-work mismatch is invisible. This is the
// pure predicate behind the one-time hint suggesting they actually seat the
// role via `aida role enter advisor`.
//
// Discriminator: `aida role enter advisor` exports BOTH `AIDA_SESSION_ROLE`
// AND `AIDA_SESSION_PROJECT` (see `emit_role_enter_eval`), and persists them in
// the shell — so a *seated* advisor's every command carries both. A one-off
// `AIDA_SESSION_ROLE=advisor aida …` prefix sets only `AIDA_SESSION_ROLE` for
// that single process and leaves `AIDA_SESSION_PROJECT` unset. So: role resolves
// to advisor via the env var, but the seat (`AIDA_SESSION_PROJECT`) was never
// established → the operator is advisor-via-prefix, not advisor-seated.
// trace:BUG-498 | ai:claude
pub(crate) fn advisor_seat_hint_warranted(role: &str, session_project_set: bool) -> bool {
    role == "advisor" && !session_project_set
}

/// BUG-498: print a one-time stderr hint when an advisor-gated command is run
/// advisor-style via an `AIDA_SESSION_ROLE=advisor` prefix while the persistent
/// seat was never established (no `aida role enter advisor`). Never blocks the
/// command, never changes the role. Gated on a per-clone marker file under
/// `.aida/` so it fires at most once (the `.aida/*` deny-by-default gitignore
/// convention means the marker needs no allow-list entry). trace:BUG-498
pub(crate) fn maybe_hint_advisor_seat() {
    let role = effective_role();
    let session_project_set = std::env::var("AIDA_SESSION_PROJECT")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if !advisor_seat_hint_warranted(&role, session_project_set) {
        return;
    }
    let Ok(project_root) = find_project_root() else {
        return;
    };
    let marker = project_root.join(".aida/.advisor-seat-hint-shown");
    if marker.exists() {
        return;
    }
    // Best-effort: ensure `.aida/` exists, then drop the marker. Failure to
    // write is non-fatal — at worst the hint shows again next time.
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&marker, "shown\n");
    eprintln!(
        "{} You're operating as {} — run {} to seat it (your statusline still shows the default role).",
        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
        "advisor".yellow(),
        "aida role enter advisor".cyan()
    );
}

/// `aida status <spec>` — the per-spec liveness view. Shows the spec's
/// lifecycle status and a LIVENESS section answering "is a live session
/// actually working this, or is the In-Progress flag orphaned?". Reuses the
/// session-lease store ([`list_leases`]), the lease-state classifier
/// ([`lease_state_for`], which already folds in pid/worktree liveness), and the
/// elapsed-from-started helper ([`humanize_duration_secs`]).
// trace:STORY-694 | ai:claude
/// The outcome of [`ensure_epic_worktree`] / [`ensure_spec_worktree`]: where
/// the worktree lives, the branch it's on, the focus label written into it, and
/// whether THIS call created it (vs. found it already registered). Lets `add`
/// print "created" vs "already exists", and `enter` re-affirm the focus on an
/// existing tree.
// trace:STORY-716 trace:STORY-742 | ai:claude
pub(crate) struct WorktreeOutcome {
    pub(crate) path: std::path::PathBuf,
    pub(crate) branch: String,
    pub(crate) focus: String,
    pub(crate) created: bool,
    pub(crate) lease_id: Option<String>,
    pub(crate) has_session_env: bool,
}

// trace:TASK-1156 | ai:codex
pub(crate) struct ScopedEnvVar {
    pub(crate) key: &'static str,
    pub(crate) previous: Option<std::ffi::OsString>,
}

impl ScopedEnvVar {
    pub(crate) fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

/// Resolve the path + branch for an epic worktree (honoring `--path`/`--branch`
/// overrides), create the git worktree off the default branch (origin/main) if
/// it isn't already registered, and write the `aida focus <epic>` marker INSIDE
/// it. Idempotent: an already-registered worktree is left as-is and its focus
/// re-affirmed. Prints nothing — the caller routes human/eval output.
// trace:STORY-716 | ai:claude
pub(crate) fn ensure_epic_worktree(
    epic: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
) -> Result<WorktreeOutcome> {
    let main_root = find_main_worktree_root()?;
    let home =
        crate::home_dir().context("cannot resolve home directory for the default worktree path")?;
    ensure_epic_worktree_core(&main_root, &home, epic, path_override, branch_override)
}

/// The testable core of [`ensure_epic_worktree`] — takes the main worktree root
/// and the home dir explicitly so a test can drive it against a throwaway git
/// repo + temp home without depending on cwd discovery.
// trace:STORY-716 | ai:claude
pub(crate) fn ensure_epic_worktree_core(
    main_root: &std::path::Path,
    home: &std::path::Path,
    epic: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
) -> Result<WorktreeOutcome> {
    let focus_label = crate::worktree::normalize_epic_label(epic);

    let path: std::path::PathBuf = match path_override {
        Some(p) => std::path::PathBuf::from(p),
        None => crate::worktree::default_worktree_path(home, epic),
    };
    let branch = branch_override
        .map(|b| b.to_string())
        .unwrap_or_else(|| crate::worktree::default_branch(epic));

    // Idempotency: if git already tracks a worktree at this path, re-affirm the
    // focus and report it rather than erroring (mirrors a re-run of `add`).
    let porcelain = std::process::Command::new("git")
        .arg("-C")
        .arg(main_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();

    if crate::worktree::is_registered(&porcelain, &path) {
        if aida_core::git_ops::is_git_repo(&path) {
            aida_core::git_ops::ensure_aida_runtime_excluded(&path)
                .with_context(|| format!("exclude AIDA runtime files in {}", path.display()))?;
        }
        crate::focus::write_focus_marker(&path, &focus_label)?;
        return Ok(WorktreeOutcome {
            path,
            branch,
            focus: focus_label,
            created: false,
            lease_id: None,
            has_session_env: false,
        });
    }

    // Fork the new branch from origin's mainline regardless of the current
    // worktree's HEAD (BUG-76's detect helper), falling back to origin/main.
    let base_ref = detect_default_branch_ref(main_root).unwrap_or_else(|| "origin/main".into());

    // If the branch already exists, attach it (drop `-b`); else create it off
    // the base ref. `git worktree add` refuses to recreate an existing branch.
    let branch_exists = std::process::Command::new("git")
        .arg("-C")
        .arg(main_root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{}", branch))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let path_str = path
        .to_str()
        .context("worktree path is not valid UTF-8")?
        .to_string();

    // Ensure the parent dir exists (e.g. `~/ai` on a fresh machine) — older git
    // won't create missing intermediate directories for the worktree path.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // `--` ends options: the path and branch/base that follow are
    // positional even when dash-led. trace:BUG-1622 | ai:claude
    let mut args: Vec<String> = vec!["worktree".into(), "add".into()];
    if branch_exists {
        args.push("--".into());
        args.push(path_str.clone());
        args.push(branch.clone());
    } else {
        args.push("-b".into());
        args.push(branch.clone());
        args.push("--".into());
        args.push(path_str.clone());
        args.push(base_ref.clone());
    }

    let res = std::process::Command::new("git")
        .arg("-C")
        .arg(main_root)
        .args(&args)
        .output()
        .context("failed to invoke `git worktree add`")?;
    if !res.status.success() {
        anyhow::bail!(
            "`git worktree add` failed: {}",
            String::from_utf8_lossy(&res.stderr).trim()
        );
    }
    aida_core::git_ops::ensure_aida_runtime_excluded(&path)
        .with_context(|| format!("exclude AIDA runtime files in {}", path.display()))?;
    aida_core::git_ops::warn_worktree_container_gitdir(main_root, &path);
    aida_core::git_ops::init_submodules_or_warn(&path, worktree_config_init_submodules(main_root))
        .with_context(|| format!("prepare submodules in worktree {}", path.display()))?;

    // Auto-scope the new tree to the epic (STORY-706 focus). Uses the low-level
    // marker writer, not the validating `aida focus` path — the fresh worktree
    // has no cache attached yet, and validation already happened (or doesn't
    // apply) at the operator's keyboard.
    crate::focus::write_focus_marker(&path, &focus_label)?;

    Ok(WorktreeOutcome {
        path,
        branch,
        focus: focus_label,
        created: true,
        lease_id: None,
        has_session_env: false,
    })
}

/// Whether a `aida worktree add|enter <arg>` argument names an epic (the
/// STORY-716 legacy path — a scoping-only worktree, no lease) or a single
/// non-epic spec (create the worktree AND take the implementer lease so the
/// spec flips InProgress and shows live). Unresolvable args and epics both take
/// the epic path; only a resolved non-epic spec takes the lease path.
// trace:STORY-742 | ai:claude
pub(crate) enum WorktreeTarget {
    /// An epic (or an arg that didn't resolve to a stored spec): the STORY-716
    /// epic-scoped worktree, unchanged.
    Epic,
    /// A single non-epic spec: `display` is its canonical id, `focus` the label
    /// the new worktree is scoped to (the spec itself).
    Spec { display: String, focus: String },
}

/// Pure classification half of [`classify_worktree_arg`]: given the requirement
/// the arg resolved to (or `None`), decide epic-vs-spec. A non-epic spec routes
/// to the lease path; an epic or an unresolved arg keeps the legacy epic path.
// trace:STORY-742 | ai:claude
pub(crate) fn classify_worktree_target(
    req: Option<&aida_core::Requirement>,
    arg: &str,
) -> WorktreeTarget {
    match req {
        Some(r) if r.req_type != RequirementType::Epic => {
            let display = r
                .agreed_id
                .as_deref()
                .or(r.spec_id.as_deref())
                .unwrap_or(arg)
                .to_string();
            WorktreeTarget::Spec {
                focus: display.clone(),
                display,
            }
        }
        _ => WorktreeTarget::Epic,
    }
}

/// Resolve a `aida worktree add|enter` argument against the store to decide
/// whether it names an epic or a single spec. Best-effort: any store-load
/// failure falls back to the legacy epic path so the command never regresses on
/// a fresh clone / offline store.
// trace:STORY-742 | ai:claude
pub(crate) fn classify_worktree_arg(arg: &str) -> WorktreeTarget {
    let Ok(main_root) = find_main_worktree_root() else {
        return WorktreeTarget::Epic;
    };
    let Ok(store) = Storage::new(main_root.join(".aida-store")).load() else {
        return WorktreeTarget::Epic;
    };
    let req = store.requirements.iter().find(|r| spec_matches(r, arg));
    classify_worktree_target(req, arg)
}

/// Testable core of [`ensure_spec_worktree`]: decide create-vs-re-enter for a
/// single-spec worktree. Inputs: the resolved home dir + spec, whether a lease
/// already covers the spec (`existing` = its worktree path + branch), and
/// whether the default path is already a registered git worktree. The `mint`
/// closure performs the real worktree+lease setup (production wires it to
/// `session_start` — the same primitive `aida queue work --no-launch` rides)
/// and returns the created worktree path + branch. Idempotent: an existing
/// lease OR a registered worktree short-circuits to re-affirming the focus
/// marker without minting.
// trace:STORY-742 | ai:claude
pub(crate) fn ensure_spec_worktree_core(
    home: &std::path::Path,
    spec_display: &str,
    focus_label: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
    existing: Option<(std::path::PathBuf, String, String, bool)>,
    registered: bool,
    mint: impl FnOnce(&std::path::Path, &str) -> Result<(std::path::PathBuf, String)>,
) -> Result<WorktreeOutcome> {
    let path = match path_override {
        Some(p) => std::path::PathBuf::from(p),
        None => crate::worktree::default_worktree_path(home, spec_display),
    };
    let branch = branch_override
        .map(|b| b.to_string())
        .unwrap_or_else(|| crate::worktree::default_branch(spec_display));

    // Idempotent re-entry: a lease already covers this spec — re-affirm focus at
    // that worktree and report it, don't mint a second one.
    if let Some((worktree_path, lease_branch, lease_id, has_session_env)) = existing {
        if aida_core::git_ops::is_git_repo(&worktree_path) {
            aida_core::git_ops::ensure_aida_runtime_excluded(&worktree_path).with_context(
                || format!("exclude AIDA runtime files in {}", worktree_path.display()),
            )?;
        }
        crate::focus::write_focus_marker(&worktree_path, focus_label)?;
        return Ok(WorktreeOutcome {
            path: worktree_path,
            branch: lease_branch,
            focus: focus_label.to_string(),
            created: false,
            lease_id: Some(lease_id),
            has_session_env,
        });
    }

    // A worktree is registered at the default path but carries no lease — treat
    // it as already-there (re-affirm focus) rather than recreating it.
    if registered {
        if aida_core::git_ops::is_git_repo(&path) {
            aida_core::git_ops::ensure_aida_runtime_excluded(&path)
                .with_context(|| format!("exclude AIDA runtime files in {}", path.display()))?;
        }
        crate::focus::write_focus_marker(&path, focus_label)?;
        return Ok(WorktreeOutcome {
            path,
            branch,
            focus: focus_label.to_string(),
            created: false,
            lease_id: None,
            has_session_env: false,
        });
    }

    // Fresh: mint the worktree + lease, then scope focus to the spec.
    let (worktree_path, minted_branch) = mint(&path, &branch)?;
    crate::focus::write_focus_marker(&worktree_path, focus_label)?;
    Ok(WorktreeOutcome {
        path: worktree_path,
        branch: minted_branch,
        focus: focus_label.to_string(),
        created: true,
        lease_id: None,
        has_session_env: false,
    })
}

/// `<spec>` variant of [`ensure_epic_worktree`]: create-if-missing a worktree
/// for a single non-epic spec and take the implementer lease, reusing the same
/// `session_start` worktree+lease primitive that `aida queue work --no-launch`
/// rides — NO agent is launched. Idempotent: an existing lease/worktree for the
/// spec is re-entered, not recreated. The worktree forks off origin/main and is
/// auto-scoped (focus) to the spec.
// trace:STORY-742 | ai:claude
pub(crate) fn ensure_spec_worktree(
    spec_display: &str,
    focus_label: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
    worktree_context: &str,
) -> Result<WorktreeOutcome> {
    let main_root = find_main_worktree_root()?;
    let home =
        crate::home_dir().context("cannot resolve home directory for the default worktree path")?;

    // Existing lease for this spec → re-enter it (freshest wins).
    let existing = list_leases(&main_root)
        .into_iter()
        .filter(|l| l.scope.eq_ignore_ascii_case(spec_display))
        .max_by_key(|l| l.started_at)
        .map(|l| {
            (
                l.worktree_path.clone(),
                l.branch.clone(),
                l.id.clone(),
                l.cargo_target_dir.is_some()
                    || l.worktree_path
                        .join(".aida")
                        .join("session-env.sh")
                        .is_file(),
            )
        });

    // Is the default path already a registered (lease-less) git worktree?
    let default_path = match path_override {
        Some(p) => std::path::PathBuf::from(p),
        None => crate::worktree::default_worktree_path(&home, spec_display),
    };
    let porcelain = std::process::Command::new("git")
        .arg("-C")
        .arg(&main_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let registered = crate::worktree::is_registered(&porcelain, &default_path);

    ensure_spec_worktree_core(
        &home,
        spec_display,
        focus_label,
        path_override,
        branch_override,
        existing,
        registered,
        |path, branch| {
            // Reuse the queue-work `--no-launch` setup primitive: session_start
            // mints the worktree + lease + Approved→InProgress bump off
            // origin/main. use_pool=Some(false) keeps the deterministic
            // per-spec path (no warm-pool tree); launch=false = no agent.
            let _context = ScopedEnvVar::set("AIDA_WORKTREE_CONTEXT", worktree_context);
            let _suppress_next = ScopedEnvVar::set("AIDA_SUPPRESS_SESSION_NEXT", "1");
            session_start(
                spec_display,
                Some(branch),
                /* base */ None,
                /* reuse_branch */ false,
                path.to_str(),
                /* forge_override */ None,
                /* branch_style */ "auto",
                /* launch */ false,
                /* launch_title */ None,
                /* launch_set_title */ false,
                /* launch_name */ None,
                /* permission_mode */ None,
                /* launch_contained */ false,
                /* role */ Some("implementer".to_string()),
                /* force_claim */ false,
                /* use_pool */ Some(false),
            )?;
            let lease = list_leases(&main_root)
                .into_iter()
                .filter(|l| l.scope.eq_ignore_ascii_case(spec_display))
                .max_by_key(|l| l.started_at)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "worktree + lease setup completed but no lease for `{}` is visible \
                         — try `aida session leases`",
                        spec_display
                    )
                })?;
            Ok((lease.worktree_path.clone(), lease.branch.clone()))
        },
    )
    .and_then(|mut out| {
        if out.lease_id.is_some() && out.has_session_env {
            return Ok(out);
        }
        if let Some(lease) = list_leases(&main_root)
            .into_iter()
            .filter(|l| l.scope.eq_ignore_ascii_case(spec_display))
            .max_by_key(|l| l.started_at)
        {
            out.lease_id = Some(lease.id.clone());
            out.has_session_env = lease.cargo_target_dir.is_some()
                || lease
                    .worktree_path
                    .join(".aida")
                    .join("session-env.sh")
                    .is_file();
        }
        Ok(out)
    })
    .map(|out| {
        // BUG-778: both verbs that land here (`worktree enter` / `worktree
        // add`) hand the worktree to a HUMAN and launch no agent. Stamp that
        // provenance on the lease so `aida ps` reads the gap before the
        // operator's launch as "awaiting agent", not "process dead — resume
        // with `aida queue work <spec>`" (which would dispatch a competing
        // session onto a spec someone is working by hand). Best-effort: a
        // failed stamp costs a cosmetic misread, never the worktree the
        // operator asked for. trace:BUG-778 | ai:claude
        if let Some(id) = out.lease_id.as_deref() {
            let _ = mark_lease_manual_enter(&main_root, id);
        }
        // BUG-1944: taking the worktree starts work on every outcome (fresh,
        // re-entered lease, or lease-less registered worktree), so an Approved
        // spec moves to In Progress here too, not only inside session_start.
        // No driver is recorded: AIDA has no driver field (derived, STORY-1653).
        // trace:BUG-1944 | ai:claude
        crate::live_work::bump_spec_in_progress(
            &main_root,
            spec_display,
            aida_core::conflict::LEASE_TAKE_AUTHOR,
        );
        out
    })
}

/// Dispatch `aida worktree <subcommand>` (STORY-716, EPIC-55 workspace layer).
// trace:STORY-716 | ai:claude
pub(crate) fn handle_worktree_command(cmd: &WorktreeCommand) -> Result<()> {
    match cmd {
        WorktreeCommand::Add {
            target,
            path,
            branch,
        } => handle_worktree_add(target, path.as_deref(), branch.as_deref()),
        WorktreeCommand::Enter {
            target,
            path,
            branch,
        } => handle_worktree_enter(target, path.as_deref(), branch.as_deref()),
        WorktreeCommand::Exit => handle_worktree_exit(),
        WorktreeCommand::List { json } => handle_worktree_list(*json),
        WorktreeCommand::Gc { yes, force, json } => {
            doctor_cmd::run_merged_agent_worktree_gc(*yes, *force, *json)
        }
        WorktreeCommand::Dismiss { path, category } => {
            worktree_dismiss::handle_dismiss(path.as_deref(), category.as_deref())
        }
        // STORY-714 warm-pool surface.
        WorktreeCommand::Reclaim {
            apply,
            min_age_mins,
            target_pct,
            include_live,
            include_open_prs,
            json,
        } => worktree_reclaim::run(&worktree_reclaim::ReclaimOptions {
            apply: *apply,
            min_age_mins: *min_age_mins,
            target_pct: *target_pct,
            include_live: *include_live,
            include_open_prs: *include_open_prs,
            json: *json,
        }),
        WorktreeCommand::Pool(pool) => handle_worktree_pool_command(pool),
    }
}

/// `[worktree_pool] max_trees` from the repo-level config (clamped ≥1), or None
/// to fall back to the pool's default cap.
pub(crate) fn worktree_pool_config_max_trees(project_root: &std::path::Path) -> Option<usize> {
    let body = std::fs::read_to_string(project_root.join(".aida").join("config.toml")).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    value
        .get("worktree_pool")?
        .get("max_trees")?
        .as_integer()
        .map(|n| n.max(1) as usize)
}

/// Read `[worktree_pool] worktree_parent` from `.aida/config.toml`: the opt-in
/// single parent directory that AIDA-created worktrees are nested under, so one
/// editor/agent folder-trust grant on that directory covers every worktree AIDA
/// mints (folder trust inherits from a parent — BUG-1700). Unset (the default)
/// keeps the historical sibling layout, so nothing moves under a live fleet. A
/// blank value is treated as unset. `~` is expanded; a relative path is left
/// relative and resolved against the project root by the pool.
// trace:BUG-1700 | ai:claude
pub(crate) fn worktree_pool_config_worktree_parent(
    project_root: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let body = std::fs::read_to_string(project_root.join(".aida").join("config.toml")).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let raw = value
        .get("worktree_pool")?
        .get("worktree_parent")?
        .as_str()?
        .trim();
    if raw.is_empty() {
        return None;
    }
    Some(expand_worktree_parent_tilde(raw))
}

/// Expand a leading `~` / `~/` in a configured worktree parent against `$HOME`.
// trace:BUG-1700 | ai:claude
pub(crate) fn expand_worktree_parent_tilde(raw: &str) -> std::path::PathBuf {
    // `home_dir()`, not a direct HOME read: it is the crate's single home
    // resolver and carries the cfg(test) redirect to a temp home, which the
    // `no_direct_home_resolution_in_crate` guard enforces (TASK-1513).
    expand_tilde_against(raw, home_dir())
}

/// Pure core of [`expand_worktree_parent_tilde`], with `$HOME` injected so it is
/// testable without mutating process environment. A path is left EXACTLY as
/// written when home is unknown, so the caller sees the literal configured value
/// rather than a silently wrong one.
// trace:BUG-1700 | ai:claude
pub(crate) fn expand_tilde_against(
    raw: &str,
    home: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    match (raw, home) {
        ("~", Some(h)) => h,
        (r, Some(h)) if r.starts_with("~/") => h.join(&r[2..]),
        _ => std::path::PathBuf::from(raw),
    }
}

/// Read `[worktree_pool] lease_ttl_secs` (seconds) from `.aida/config.toml`,
/// falling back to the core default when unset/invalid. Drives the
/// reservation-leak backstop: a lease older than this with no live owner is
/// reclaimable (and shows as `expired` in `pool status`).
// trace:TASK-1008 | ai:claude
pub(crate) fn worktree_pool_config_lease_ttl_secs(project_root: &std::path::Path) -> i64 {
    std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        .ok()
        .and_then(|body| toml::from_str::<toml::Value>(&body).ok())
        .and_then(|value| {
            value
                .get("worktree_pool")?
                .get("lease_ttl_secs")?
                .as_integer()
        })
        .unwrap_or(aida_core::worktree_pool::DEFAULT_LEASE_TTL_SECS)
}

/// Read `[worktree] init_submodules` from `.aida/config.toml`. Missing or
/// invalid values default to true so AIDA-created worktrees are build-ready for
/// repos with vendored git submodules.
// trace:BUG-899 | ai:codex
pub(crate) fn worktree_config_init_submodules(project_root: &std::path::Path) -> bool {
    std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        .ok()
        .and_then(|body| toml::from_str::<toml::Value>(&body).ok())
        .and_then(|value| value.get("worktree")?.get("init_submodules")?.as_bool())
        .unwrap_or(true)
}

/// Hook commands for `key` (`post_create` / `pre_destroy`), sourced ONLY from
/// the machine-global `~/.aida/config.toml`. Repo-level config is deliberately
/// ignored — cloning a repo must never run arbitrary shell on your machine
/// (treehouse's stance).
///
/// For `post_create`, the opt-in `[worktree_pool] prewarm_build = true` knob
/// (also global-only, since it triggers a build) appends the canonical
/// backgrounded `cargo build` pre-warm so a freshly-created pool tree starts
/// warm on first use (TASK-1010).
// trace:STORY-714 trace:TASK-1010 | ai:claude
pub(crate) fn worktree_pool_global_hooks(key: &str) -> Vec<String> {
    let Some(home) = crate::home_dir() else {
        return Vec::new();
    };
    let Ok(body) = std::fs::read_to_string(home.join(".aida").join("config.toml")) else {
        return Vec::new();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return Vec::new();
    };
    worktree_pool_hooks_from_config(&value, key)
}

/// Pure parse of the `[worktree_pool]` hook list for `key`, plus the TASK-1010
/// `prewarm_build` injection. Split out so the sourcing rule (global-config
/// only, in `worktree_pool_global_hooks`) stays where it belongs and this
/// decision — which commands run for a phase — is unit-testable.
// trace:TASK-1010 | ai:claude
pub(crate) fn worktree_pool_hooks_from_config(value: &toml::Value, key: &str) -> Vec<String> {
    let pool = value.get("worktree_pool");
    let mut hooks: Vec<String> = pool
        .and_then(|w| w.get(key))
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    // Opt-in cargo pre-warm — turn cold-create into warm-on-first-use. The
    // command is backgrounded (see PREWARM_BUILD_COMMAND) so it never delays the
    // handout; appended after any explicit hooks so a user's own post_create
    // still runs first.
    if key == "post_create"
        && pool
            .and_then(|w| w.get("prewarm_build"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        hooks.push(aida_core::worktree_hooks::PREWARM_BUILD_COMMAND.to_string());
    }
    hooks
}

#[cfg(test)]
#[path = "tests/task_1010_prewarm_tests.rs"]
mod task_1010_prewarm_tests;

/// Trust policy for spec-authored acceptance commands, sourced ONLY from the
/// machine-global `~/.aida/config.toml` `[review]` table — the same sourcing
/// rule as [`worktree_pool_global_hooks`]. Repo config (branch-local AND the
/// trusted default-branch copy), the store, env vars and CLI flags are
/// deliberately not trust sources: an unattended-drain PR could otherwise
/// merge the opt-in through the very review it switches on. Every failure
/// (no home, no file, unreadable, malformed, wrong types) resolves to the
/// denied default and is reported once on stderr.
// trace:STORY-1476 | ai:claude
pub(crate) fn acceptance_command_policy_global() -> graded_review::AcceptanceCommandPolicy {
    match acceptance_command_policy_global_quiet() {
        Ok(policy) => policy,
        Err(reason) => {
            eprintln!(
                "  {} graded review: spec-authored acceptance commands will not run ({reason}); \
                 criteria that need them go to the reviewer seat as manual checks",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            graded_review::AcceptanceCommandPolicy::default()
        }
    }
}

/// [`acceptance_command_policy_global`] without the stderr notice: `Err` names
/// why the policy is denied. Used by `aida config show` to render the value.
// trace:STORY-1476 | ai:claude
pub(crate) fn acceptance_command_policy_global_quiet(
) -> std::result::Result<graded_review::AcceptanceCommandPolicy, String> {
    let Some(home) = crate::home_dir() else {
        return Err("home directory could not be resolved".to_string());
    };
    let path = home.join(".aida").join("config.toml");
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("no [review] opt-in in ~/.aida/config.toml".to_string());
        }
        Err(e) => return Err(format!("could not read ~/.aida/config.toml: {e}")),
    };
    let value = toml::from_str::<toml::Value>(&body)
        .map_err(|e| format!("~/.aida/config.toml did not parse: {e}"))?;
    acceptance_command_policy_from_toml(&value)
}

/// Pure parse of `[review] run_acceptance_commands` +
/// `acceptance_command_allow`. Fail-closed: anything other than a well-typed,
/// enabled, non-empty allowlist of clean entries is `Err` (denied), never a
/// partially-honoured policy — a bad element does not get skipped. Refused
/// characters are checked on the entry as written, before trimming, so a
/// malformed entry cannot be normalised into an accepted one.
// trace:STORY-1476 | ai:claude
pub(crate) fn acceptance_command_policy_from_toml(
    value: &toml::Value,
) -> std::result::Result<graded_review::AcceptanceCommandPolicy, String> {
    let Some(review) = value.get("review") else {
        return Err("no [review] table in ~/.aida/config.toml".to_string());
    };
    if !review.is_table() {
        return Err("[review] is not a table".to_string());
    }
    let enabled = match review.get("run_acceptance_commands") {
        None => return Err("[review] run_acceptance_commands is not set".to_string()),
        Some(v) => v.as_bool().ok_or_else(|| {
            "[review] run_acceptance_commands must be a boolean (true/false)".to_string()
        })?,
    };
    // Type-check the allowlist even when disabled, so a wrong type anywhere
    // in the pair is reported rather than silently tolerated.
    let allow: Vec<String> = match review.get("acceptance_command_allow") {
        None => Vec::new(),
        Some(v) => {
            let arr = v
                .as_array()
                .ok_or_else(|| "[review] acceptance_command_allow must be an array".to_string())?;
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let entry = item.as_str().ok_or_else(|| {
                    "[review] acceptance_command_allow entries must all be strings".to_string()
                })?;
                // Fail closed on the RAW entry, before any trimming: `*\r`
                // would otherwise normalise to `*` and be accepted as full
                // trust even though CR is refused.
                // trace:STORY-1476 trace:TASK-1545 | ai:claude
                if graded_review::contains_refused_chars(entry) {
                    return Err(format!(
                        "[review] acceptance_command_allow entry `{}` contains a shell \
                         metacharacter or control character",
                        entry.escape_debug()
                    ));
                }
                let trimmed = entry.trim();
                if trimmed.is_empty() {
                    return Err("[review] acceptance_command_allow has an empty entry".to_string());
                }
                // `*` is either the whole entry (full trust) or the last word
                // (any trailing arguments); anywhere else it is a typo.
                let words: Vec<&str> = trimmed.split_whitespace().collect();
                if words
                    .iter()
                    .enumerate()
                    .any(|(i, w)| *w == "*" && i + 1 != words.len())
                {
                    return Err(format!(
                        "[review] acceptance_command_allow entry `{trimmed}`: `*` may only be the \
                         last word"
                    ));
                }
                out.push(trimmed.to_string());
            }
            out
        }
    };
    if !enabled {
        return Err("[review] run_acceptance_commands = false".to_string());
    }
    if allow.is_empty() {
        return Err(
            "[review] run_acceptance_commands = true but acceptance_command_allow is missing or \
             empty"
                .to_string(),
        );
    }
    Ok(graded_review::AcceptanceCommandPolicy {
        enabled: true,
        allow,
        repo_optin_ignored: false,
    })
}

/// Display-only detection of a repo-level `[review]` opt-in — the branch-local
/// `.aida/config.toml` or its trusted default-branch copy naming
/// `run_acceptance_commands` / `acceptance_command_allow`. The result feeds a
/// one-line notice in the verdict summary and NEVER the policy itself.
// trace:STORY-1476 | ai:claude
pub(crate) fn repo_review_optin_present(project_root: &std::path::Path) -> bool {
    let names_optin = |body: &str| {
        toml::from_str::<toml::Value>(body)
            .ok()
            .and_then(|v| v.get("review").cloned())
            .is_some_and(|review| {
                review.get("run_acceptance_commands").is_some()
                    || review.get("acceptance_command_allow").is_some()
            })
    };
    let local = std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        .ok()
        .is_some_and(|body| names_optin(&body));
    local
        || crate::trusted_config::read_trusted_config_toml_local(project_root)
            .is_some_and(|body| names_optin(&body))
}

#[cfg(test)]
#[path = "tests/story_1476_acceptance_trust_tests.rs"]
mod story_1476_acceptance_trust_tests;

#[cfg(test)]
#[path = "tests/task_1558_agents_mcp_type_tests.rs"]
mod task_1558_agents_mcp_type_tests;

#[cfg(test)]
#[path = "tests/bug_1700_worktree_parent_tests.rs"]
mod bug_1700_worktree_parent_tests;

#[cfg(test)]
#[path = "tests/task_1561_pickup_worktree_parent_tests.rs"]
mod task_1561_pickup_worktree_parent_tests;

#[cfg(test)]
#[path = "tests/bug_1701_drain_routing_tests.rs"]
mod bug_1701_drain_routing_tests;

#[cfg(test)]
#[path = "tests/bug_1761_preclaim_collision_tests.rs"]
mod bug_1761_preclaim_collision_tests;

#[cfg(test)]
#[path = "tests/bug_1763_merged_branch_preclaim_tests.rs"]
mod bug_1763_merged_branch_preclaim_tests;

pub(crate) fn handle_worktree_pool_command(cmd: &WorktreePoolCommand) -> Result<()> {
    let project_root = find_project_root()?;
    match cmd {
        WorktreePoolCommand::Status { json } => worktree_pool_status(&project_root, *json),
        WorktreePoolCommand::Acquire { lease_holder, json } => {
            let opts = aida_core::worktree_pool::AcquireOptions {
                lease_holder: lease_holder.clone(),
                max_trees: worktree_pool_config_max_trees(&project_root),
                lease_ttl_secs: Some(worktree_pool_config_lease_ttl_secs(&project_root)),
                post_create_hooks: worktree_pool_global_hooks("post_create"),
                init_submodules: worktree_config_init_submodules(&project_root),
                parent_dir: worktree_pool_config_worktree_parent(&project_root),
            };
            let path = aida_core::worktree_pool::acquire(&project_root, &opts)?;
            if *json {
                println!("{}", serde_json::json!({ "path": path }));
            } else {
                println!("{}", path.display());
            }
            Ok(())
        }
        WorktreePoolCommand::Return {
            path,
            include_in_use,
        } => {
            let target = match path {
                Some(p) => std::path::PathBuf::from(p),
                None => std::env::current_dir()?,
            };
            // BUG-1986: a return runs a forced checkout and a hard reset,
            // which rewrites every file under a running build. Apply the
            // strict check: a live holder, or a scan that cannot be
            // verified, refuses unless --include-in-use. trace:BUG-1986 | ai:claude
            if !*include_in_use {
                if let Err(in_use) = aida_core::worktree_holders::check_worktree_removal(&target) {
                    anyhow::bail!(
                        "{}\n\nThe worktree was NOT returned; it is unchanged.",
                        in_use.refusal("--include-in-use")
                    );
                }
            }
            aida_core::worktree_pool::return_to_pool(&project_root, &target)?;
            println!(
                "{} returned {} to the warm-pool (reset to a clean base, marked idle — directory + build cache kept)",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                target.display()
            );
            Ok(())
        }
        WorktreePoolCommand::Adopt {
            paths,
            all,
            no_dry_run,
            include_unlanded,
            json,
        } => worktree_pool_adopt(
            &project_root,
            paths,
            *all,
            !*no_dry_run,
            *include_unlanded,
            *json,
        ),
        WorktreePoolCommand::Destroy {
            paths,
            all,
            no_dry_run,
            include_unlanded,
            include_in_use,
            include_leased,
            json,
        } => worktree_pool_destroy(
            &project_root,
            paths,
            *all,
            !*no_dry_run,
            *include_unlanded,
            *include_in_use,
            *include_leased,
            *json,
        ),
    }
}

pub(crate) fn worktree_pool_status(project_root: &std::path::Path, json: bool) -> Result<()> {
    let cwd = std::env::current_dir().ok();
    let lease_ttl = worktree_pool_config_lease_ttl_secs(project_root);
    let rows = aida_core::worktree_pool::list(project_root, cwd.as_deref(), lease_ttl)?;
    // Warm-pool HIT-RATE telemetry (reuse vs create) — proves the warm-cache
    // payoff empirically. Counts live in the pool state (counts only, no
    // paths/content). trace:TASK-1012 | ai:claude
    let pool_state = aida_core::worktree_pool::read_state(project_root).unwrap_or_default();

    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.entry.name,
                    "path": r.entry.path,
                    "state": r.state.label(),
                    "leased": r.entry.leased,
                    "lease_holder": r.entry.lease_holder,
                    "leased_at": r.entry.leased_at,
                    "owner_pid": r.entry.owner_pid,
                    "head": r.head,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "pool": arr,
                "telemetry": {
                    "reuse_count": pool_state.reuse_count,
                    "create_count": pool_state.create_count,
                    "total_acquires": pool_state.total_acquires(),
                    "hit_rate": pool_state.hit_rate(),
                },
            }))?
        );
        return Ok(());
    }

    if rows.is_empty() {
        println!("warm-pool is empty — `aida worktree pool acquire` creates the first tree");
        print_worktree_pool_hit_rate(&pool_state);
        return Ok(());
    }

    use crate::glyphs::Glyph;
    use aida_core::worktree_pool::PoolState;
    println!("{}", "WORKTREE WARM-POOL".bold());
    for r in &rows {
        // Registry-managed glyphs route through `crate::glyph` so they get the
        // ASCII fallback on terminals that can't draw them (glyph-lint). The
        // filled-circle/diamond/ellipsis markers below have no registry variant
        // and stay raw. trace:STORY-714 trace:TASK-835
        let dot = match r.state {
            PoolState::Available => "●".green(),
            PoolState::InUse => crate::glyph(Glyph::InFlight).yellow(),
            PoolState::Leased => "◆".cyan(),
            PoolState::Expired => crate::glyph(Glyph::Warning).yellow(),
            PoolState::Dirty => crate::glyph(Glyph::Cross).red(),
            PoolState::Destroying => "…".dimmed(),
            PoolState::Here => crate::glyph(Glyph::FlowActive).bold(),
        };
        let head = r
            .head
            .as_deref()
            .map(|h| h[..h.len().min(9)].to_string())
            .unwrap_or_else(|| "-".to_string());
        let holder = r
            .entry
            .lease_holder
            .as_deref()
            .map(|h| format!("  lease:{h}"))
            .unwrap_or_default();
        println!(
            "  {} {:<14} {:<11} {}  {}{}",
            dot,
            r.entry.name,
            r.state.label(),
            head.dimmed(),
            r.entry.path.display(),
            holder.cyan()
        );
    }
    print_worktree_pool_hit_rate(&pool_state);
    Ok(())
}

/// Render the warm-pool HIT-RATE line — reuse (warm-cache hit) vs create (cold
/// mint), and the ratio that proves the warm-cache payoff empirically. Silent
/// until the first acquire is observed (nothing to report yet). Counts only, no
/// paths/content.
// trace:TASK-1012 | ai:claude
pub(crate) fn print_worktree_pool_hit_rate(pool: &aida_core::worktree_pool::Pool) {
    match pool.hit_rate() {
        Some(rate) => {
            let pct = format!("{:.0}%", rate * 100.0);
            println!(
                "  hit-rate {}  ({} reuse / {} total, {} fresh mint)",
                pct.bold(),
                pool.reuse_count,
                pool.total_acquires(),
                pool.create_count,
            );
        }
        None => println!("{}", "  hit-rate n/a  (no acquires observed yet)".dimmed()),
    }
}

/// `aida worktree pool adopt` — register pre-pool sibling worktrees into the
/// warm-pool registry. Preview by default; the only side effect when applied is
/// new entries in the registry (no worktree is ever reset, checked out, or
/// removed). Exits non-zero only on a hard error — an all-skipped pass is a
/// legitimate "nothing to migrate" outcome.
// trace:TASK-1009 | ai:claude
pub(crate) fn worktree_pool_adopt(
    project_root: &std::path::Path,
    paths: &[String],
    all: bool,
    dry_run: bool,
    include_unlanded: bool,
    json: bool,
) -> Result<()> {
    use aida_core::worktree_pool_adopt::{self as wa, AdoptAction, AdoptOptions, AdoptSelector};

    let selector = if all {
        AdoptSelector::All
    } else if !paths.is_empty() {
        AdoptSelector::Paths(paths.iter().map(std::path::PathBuf::from).collect())
    } else {
        anyhow::bail!(
            "specify worktree path(s) to adopt, or pass --all to consider every worktree of this repository"
        );
    };

    let opts = AdoptOptions {
        dry_run,
        include_unlanded,
        max_trees: worktree_pool_config_max_trees(project_root),
        ..Default::default()
    };
    // A worktree a live session is working in is never adopted — the pool must
    // not start accounting for a tree somebody is inside.
    let live = wa::live_lease_worktrees(project_root);
    let report = wa::adopt(project_root, &selector, &opts, &live)?;

    if json {
        let arr: Vec<_> = report
            .targets
            .iter()
            .map(|t| {
                serde_json::json!({
                    "path": t.path,
                    "name": t.name,
                    "class": t.class.label(),
                    "action": match t.action {
                        AdoptAction::WouldAdopt => "would-adopt",
                        AdoptAction::Adopted => "adopted",
                        AdoptAction::Skipped => "skipped",
                    },
                    "reserved": t.reserved,
                    "reason": t.reason,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "dry_run": report.dry_run,
                "adopted": report.adopted_count(),
                "would_adopt": report.would_adopt_count(),
                "skipped": report.skipped_count(),
                "targets": arr,
            }))?
        );
        return Ok(());
    }

    if report.targets.is_empty() {
        println!("no candidate worktrees to adopt");
        return Ok(());
    }

    if report.dry_run {
        println!(
            "{} dry-run — registry unchanged (pass --no-dry-run to apply)",
            "→".dimmed()
        );
    }
    for t in &report.targets {
        let mark = match t.action {
            AdoptAction::WouldAdopt => "would adopt".yellow(),
            AdoptAction::Adopted => "adopted".green(),
            AdoptAction::Skipped => "skipped".dimmed(),
        };
        println!("  {}  {}  {}", mark, t.path.display(), t.reason.dimmed());
    }
    if !report.dry_run {
        println!(
            "{} {} worktree(s) adopted into the warm-pool",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            report.adopted_count()
        );
        if report.targets.iter().any(|t| t.reserved) {
            println!(
                "  reserved trees keep their unlanded work and are never handed out — release one with `aida worktree pool return <path>`"
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn worktree_pool_destroy(
    project_root: &std::path::Path,
    paths: &[String],
    all: bool,
    dry_run: bool,
    include_unlanded: bool,
    include_in_use: bool,
    include_leased: bool,
    json: bool,
) -> Result<()> {
    use aida_core::worktree_pool_destroy::{
        self as wd, DestroyAction, DestroyOptions, DestroySelector,
    };

    let selector = if all {
        DestroySelector::All
    } else if !paths.is_empty() {
        DestroySelector::Paths(paths.iter().map(std::path::PathBuf::from).collect())
    } else {
        anyhow::bail!("specify worktree path(s) to destroy, or pass --all for a bulk sweep");
    };

    let opts = DestroyOptions {
        dry_run,
        include_unlanded,
        include_in_use,
        include_leased,
        pre_destroy_hooks: worktree_pool_global_hooks("pre_destroy"),
    };

    let mut salvage = |p: &std::path::Path| {
        let _ = salvage_worktree_patch(project_root, "worktree-pool", Some("pool"), p);
    };
    let report = wd::destroy(project_root, &selector, &opts, &mut salvage)?;

    if json {
        let arr: Vec<_> = report
            .targets
            .iter()
            .map(|t| {
                serde_json::json!({
                    "path": t.entry.path,
                    "name": t.entry.name,
                    "class": t.class.label(),
                    "action": match t.action {
                        DestroyAction::WouldRemove => "would-remove",
                        DestroyAction::Removed => "removed",
                        DestroyAction::Skipped => "skipped",
                    },
                    "reason": t.reason,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "dry_run": report.dry_run,
                "removed": report.removed_count(),
                "targets": arr,
            }))?
        );
        return Ok(());
    }

    if report.targets.is_empty() {
        println!("no matching pool worktrees");
        return Ok(());
    }

    if report.dry_run {
        println!(
            "{} dry-run — nothing removed (pass --no-dry-run to apply)",
            "→".dimmed()
        );
    }
    for t in &report.targets {
        let mark = match t.action {
            DestroyAction::WouldRemove => "would remove".yellow(),
            DestroyAction::Removed => "removed".green(),
            DestroyAction::Skipped => "skipped".dimmed(),
        };
        println!(
            "  {}  {}  {}",
            mark,
            t.entry.path.display(),
            t.reason.dimmed()
        );
    }
    if !report.dry_run {
        println!(
            "{} {} worktree(s) removed",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            report.removed_count()
        );
    }
    Ok(())
}

/// `aida worktree add <epic|spec>` — create (or re-affirm) the worktree and
/// print its path. Mirrors `git worktree add`: prints the path, does NOT cd.
/// An epic arg keeps the STORY-716 scoping-only behavior; a single non-epic
/// spec additionally takes the implementer lease (STORY-742).
// trace:STORY-716 trace:STORY-742 | ai:claude
pub(crate) fn handle_worktree_add(
    arg: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
) -> Result<()> {
    let out = match classify_worktree_arg(arg) {
        WorktreeTarget::Epic => ensure_epic_worktree(arg, path_override, branch_override)?,
        WorktreeTarget::Spec { display, focus } => {
            ensure_spec_worktree(&display, &focus, path_override, branch_override, "add")?
        }
    };
    let verb = if out.created {
        "Created"
    } else {
        "Already exists"
    };
    println!(
        "{} {} worktree for {} at {}",
        crate::glyph(crate::glyphs::Glyph::Check),
        verb,
        out.focus.cyan().bold(),
        out.path.display().to_string().cyan(),
    );
    println!("  branch: {}  ·  focus: {}", out.branch, out.focus);
    print_worktree_next("add", arg, &out, false);
    Ok(())
}

/// `aida worktree enter <epic|spec>` — create-if-missing, then emit `cd
/// '<path>'` on stdout for the `aida()` shell wrapper to auto-eval. Human-facing
/// status goes to STDERR so it never contaminates the eval'd stdout. An epic arg
/// keeps the STORY-716 scoping-only behavior; a single non-epic spec
/// additionally takes the implementer lease (STORY-742).
// trace:STORY-716 trace:STORY-742 | ai:claude
pub(crate) fn handle_worktree_enter(
    arg: &str,
    path_override: Option<&str>,
    branch_override: Option<&str>,
) -> Result<()> {
    let out = match classify_worktree_arg(arg) {
        WorktreeTarget::Epic => ensure_epic_worktree(arg, path_override, branch_override)?,
        WorktreeTarget::Spec { display, focus } => {
            ensure_spec_worktree(&display, &focus, path_override, branch_override, "enter")?
        }
    };
    let verb = if out.created {
        "Created and entering"
    } else {
        "Entering"
    };
    // Status to stderr — the eval-wrapper captures stdout only.
    eprintln!(
        "{} {} worktree for {} ({} · focus {})",
        crate::glyph(crate::glyphs::Glyph::Check),
        verb,
        out.focus,
        out.branch,
        out.focus,
    );
    // `worktree enter` is itself a pickup surface. Keep stdout pure shell code
    // and put the same capped, META-citing protocol block on stderr.
    // trace:STORY-1221 | ai:codex
    if let Ok(main_root) = find_main_worktree_root() {
        if let Ok(store) = Storage::new(main_root.join(".aida-store")).load() {
            if let Some(req) = store.requirements.iter().find(|r| spec_matches(r, arg)) {
                if let Some(block) = protocol_cmd::pickup_block_for_requirement(&store, req) {
                    eprintln!("\n{}\n", block);
                }
            }
        }
    }
    // BUG-654: warn (to STDERR, never stdout) when the installed shell wrapper
    // can't auto-eval this `cd`, so the silent no-op is explained instead of
    // leaving the operator wondering why `aida focus` shows nothing afterward.
    if !wrapper_can_eval_worktree_enter() {
        eprintln!(
            "  {} shell wrapper looks stale or missing — the `cd` below won't auto-apply. \
             Run `aida dev shell-init --install` to enable auto-cd, or \
             `eval \"$(aida worktree enter {})\"`.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            arg,
        );
    }
    let shell_payload_evaled = wrapper_can_eval_worktree_enter();
    print_worktree_next("enter", arg, &out, shell_payload_evaled);
    // BUG-780: record the lease file backing the marker so the wrapper's prompt
    // hook can tell a live session from one ended elsewhere.
    // trace:TASK-1160 trace:BUG-780 | ai:claude
    let lease_file = out.lease_id.as_deref().and_then(|id| {
        find_main_worktree_root()
            .ok()
            .map(|root| lease_path(&root, id))
    });
    // trace:TASK-1171 | ai:claude — everything printed while this guard is
    // alive is SHELL CODE, marked as such for the wrapper.
    {
        let _eval = crate::shell_eval::EvalBlock::open();
        print!(
            "{}",
            enter_shell_payload(&out.path, &out.focus, lease_file.as_deref())
        );
    }
    Ok(())
}

/// BUG-780: the worktree session a shell CARRIES — the `AIDA_SESSION_ID` export
/// and the `(wt:...) ` PS1 segment `enter` spliced in. Both live in the shell,
/// not in cwd, so a bare `cd` out (or a `session end` run from another shell)
/// leaves them behind. `worktree exit` clears whichever of the two is present.
// trace:BUG-780 | ai:claude
pub(crate) struct CarriedSession {
    pub(crate) session_id: Option<String>,
    pub(crate) marker: Option<String>,
}

/// Read `key` from the env, trimmed, treating blank as unset.
// trace:BUG-780 | ai:claude
pub(crate) fn carried_env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// BUG-780: is this shell carrying a worktree session? `None` means there is
/// genuinely nothing to step out of, wherever the shell stands.
// trace:BUG-780 | ai:claude
pub(crate) fn carried_worktree_session() -> Option<CarriedSession> {
    let session_id = carried_env_value("AIDA_SESSION_ID");
    let marker = carried_env_value("AIDA_WT_PS1_PREFIX");
    if session_id.is_none() && marker.is_none() {
        return None;
    }
    Some(CarriedSession { session_id, marker })
}

/// Does `lease_id` name the session the shell carries? Exact match, or the
/// short (>= 8 char) prefix the session verbs print — so an id copied off a
/// status line still resolves. A carried id that matches NO lease is dangling:
/// the session was ended from another shell and the marker is pointing at work
/// that no longer exists. Pure so the decision is unit-testable.
// trace:BUG-780 | ai:claude
pub(crate) fn lease_id_matches(lease_id: &str, carried: &str) -> bool {
    let carried = carried.trim().to_ascii_lowercase();
    if carried.is_empty() {
        return false;
    }
    let lease_id = lease_id.trim().to_ascii_lowercase();
    lease_id == carried || (carried.len() >= 8 && lease_id.starts_with(&carried))
}

/// The env names a worktree's generated `session-env.sh` exports — the extra
/// unsets `worktree exit` emits beyond the well-known base set, derived from
/// the same file `enter` sources so the two never drift.
// trace:TASK-1160 trace:BUG-780 | ai:claude
pub(crate) fn session_env_unset_names(worktree: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(worktree.join(".aida").join("session-env.sh"))
        .map(|body| {
            // Allowlisted names only: a branch-committed file must not make
            // `worktree exit` unset PATH. trace:BUG-1624 | ai:claude
            parse_session_env(&body)
                .into_iter()
                .map(|(n, _)| n)
                .filter(|n| SESSION_ENV_NAMES.contains(&n.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

/// Pure decision: is the shell STANDING in a scoped worktree? `toplevel` is the
/// checkout containing cwd (`None` when cwd is outside any git checkout), and a
/// toplevel equal to the main checkout — or an unresolvable main root — means
/// there is no worktree to step out of. This is only half the exit question:
/// BUG-780's other half is what the shell CARRIES, which is cwd-independent.
// trace:BUG-780 | ai:claude
pub(crate) fn scoped_worktree_for_exit(
    toplevel: Option<&std::path::Path>,
    main_root: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    let toplevel = toplevel?;
    let main_root = main_root?;
    (crate::worktree::canonical_or_self(toplevel) != crate::worktree::canonical_or_self(main_root))
        .then(|| toplevel.to_path_buf())
}

/// `aida worktree exit` — the symmetric step-out: emit `cd '<main root>'`,
/// unset the session env exports `enter` applied, and strip the `(wt:...)`
/// PS1 segment, all via the same auto-evaled stdout payload. The session
/// lease and the spec's status are untouched — the worktree stays live for
/// re-enter; `aida session end` is the verb that finishes the work.
///
/// BUG-780: the step-out is keyed on what the shell CARRIES, not only on cwd.
/// Standing outside a scoped worktree while still carrying its session env (a
/// bare `cd` out, or a session ended from another shell) used to be a no-op
/// that left the `(wt:)` marker stuck with no verb to clear it — now the
/// session env + marker are cleared wherever you stand, minus the `cd` (moving
/// the shell out of an unrelated directory would be the surprise). Only a shell
/// carrying nothing gets the friendly no-op.
// trace:TASK-1160 | ai:claude
// trace:BUG-780 | ai:claude
pub(crate) fn handle_worktree_exit() -> Result<()> {
    let carried = carried_worktree_session();
    // Outside an AIDA project entirely we can still clear what the shell
    // carries — the env is the shell's, not the project's.
    let main_root = match find_main_worktree_root() {
        Ok(root) => Some(root),
        Err(e) => {
            if carried.is_none() {
                return Err(e);
            }
            None
        }
    };

    // Where is the shell standing? The toplevel of the checkout containing
    // cwd, or None when cwd isn't inside any git checkout.
    let toplevel = std::env::current_dir()
        .ok()
        .and_then(|cwd| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(["rev-parse", "--show-toplevel"])
                .output()
                .ok()
        })
        .filter(|o| o.status.success())
        .map(|o| std::path::PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string()));
    let toplevel = scoped_worktree_for_exit(toplevel.as_deref(), main_root.as_deref());

    let Some(toplevel) = toplevel else {
        return worktree_exit_from_outside(main_root.as_deref(), carried);
    };

    // Session env exports to clear: always the well-known base set, plus
    // whatever else this worktree's session-env.sh exports.
    let extra_unsets = session_env_unset_names(&toplevel);
    let main_root = main_root.expect("a scoped worktree implies a resolvable main root");

    // The lease (if any) survives the step-out — say so, with the way back.
    let toplevel_canon = crate::worktree::canonical_or_self(&toplevel);
    let lease = list_leases(&main_root)
        .into_iter()
        .filter(|l| crate::worktree::canonical_or_self(&l.worktree_path) == toplevel_canon)
        .max_by_key(|l| l.started_at);

    // Human-facing status to STDERR — the eval-wrapper captures stdout only.
    eprintln!(
        "{} Stepping out of {} back to {}",
        crate::glyph(crate::glyphs::Glyph::Check),
        toplevel.display().to_string().cyan(),
        main_root.display().to_string().cyan(),
    );
    match &lease {
        Some(l) => {
            let short = &l.id[..l.id.len().min(8)];
            eprintln!(
                "  stepped out — session {} ({}) still holds the worktree; re-enter with \
                 `aida worktree enter {}`, finish with `aida session end {}`",
                short.cyan(),
                l.scope.cyan().bold(),
                l.scope,
                short,
            );
        }
        None => {
            if let Some(focus) = crate::focus::read_focus_marker(&toplevel) {
                eprintln!(
                    "  the worktree stays put — re-enter with `aida worktree enter {}`",
                    focus
                );
            }
        }
    }
    // Warn (STDERR) when the installed wrapper predates the exit verb — its
    // case statement won't auto-eval this payload, so the cd would silently
    // no-op (same staleness UX as enter).
    if !wrapper_can_eval_worktree_exit() {
        eprintln!(
            "  {} shell wrapper looks stale or missing — the `cd` below won't auto-apply. \
             Run `aida dev shell-init --install` to refresh it, or \
             `eval \"$(aida worktree exit)\"`.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        );
    }
    // trace:TASK-1171 | ai:claude
    {
        let _eval = crate::shell_eval::EvalBlock::open();
        print!(
            "{}",
            crate::worktree::exit_shell_payload(&main_root, &extra_unsets)
        );
    }
    Ok(())
}

/// BUG-780: `worktree exit` run from OUTSIDE a scoped worktree. Carrying
/// nothing is the old friendly no-op. Carrying a session (id and/or the `(wt:)`
/// PS1 marker) clears it right here — the shell's carrying is the thing exit
/// ends, and it outlives the cwd that created it. No `cd` is emitted: the shell
/// is already out of the worktree, and relocating it from wherever it stands
/// would be the surprise.
// trace:BUG-780 | ai:claude
pub(crate) fn worktree_exit_from_outside(
    main_root: Option<&std::path::Path>,
    carried: Option<CarriedSession>,
) -> Result<()> {
    let Some(carried) = carried else {
        let where_main = main_root
            .map(|m| format!(" — the main checkout is {}", m.display().to_string().cyan()))
            .unwrap_or_default();
        eprintln!(
            "{} Not inside a scoped worktree{}. Nothing to step out of.",
            crate::glyph(crate::glyphs::Glyph::Check),
            where_main,
        );
        return Ok(());
    };

    // Resolve the carried id against the live leases so the report can say
    // whether the session is still running or was ended elsewhere, and so the
    // unset list covers that worktree's own session-env exports.
    let leases = main_root.map(list_leases).unwrap_or_default();
    let lease = carried
        .session_id
        .as_deref()
        .and_then(|id| leases.iter().find(|l| lease_id_matches(&l.id, id)));
    let extra_unsets = lease
        .map(|l| session_env_unset_names(&l.worktree_path))
        .unwrap_or_default();

    let label = lease
        .map(|l| l.scope.clone())
        .or_else(|| {
            carried
                .marker
                .as_deref()
                .and_then(|m| m.trim().strip_prefix("(wt:"))
                .and_then(|m| m.strip_suffix(')'))
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "this shell's session".to_string());

    eprintln!(
        "{} Stepped out of {} — this shell carried it but was standing elsewhere.",
        crate::glyph(crate::glyphs::Glyph::Check),
        label.cyan().bold(),
    );
    match lease {
        Some(l) => {
            let short = &l.id[..l.id.len().min(8)];
            eprintln!(
                "  the session is still live — re-enter with `aida worktree enter {}`, \
                 finish with `aida session end {}`",
                l.scope, short,
            );
        }
        None => {
            eprintln!(
                "  the session it pointed at is gone (ended elsewhere) — the marker was stale."
            );
        }
    }
    if !wrapper_can_eval_worktree_exit() {
        eprintln!(
            "  {} shell wrapper looks stale or missing — the clearing below won't auto-apply. \
             Run `aida dev shell-init --install` to refresh it, or \
             `eval \"$(aida worktree exit)\"`.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        );
    } else if !wrapper_can_self_heal_wt_marker() {
        eprintln!(
            "  {} refresh the shell wrapper (`aida dev shell-init --install`) so the marker \
             also clears itself when a session ends elsewhere.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
        );
    }
    // trace:TASK-1171 | ai:claude
    {
        let _eval = crate::shell_eval::EvalBlock::open();
        print!("{}", crate::worktree::session_clear_payload(&extra_unsets));
    }
    Ok(())
}

// trace:TASK-1156 | ai:codex
pub(crate) fn print_worktree_next(
    command: &str,
    arg: &str,
    out: &WorktreeOutcome,
    shell_payload_evaled: bool,
) {
    eprintln!();
    eprintln!("Next:");
    match (command, out.lease_id.as_deref(), shell_payload_evaled) {
        ("enter", Some(_), true) => {
            eprintln!(
                "  {}",
                format!("claude    # then /aida-implement {}", out.focus).cyan()
            );
        }
        ("enter", Some(_), false) => {
            eprintln!(
                "  {}",
                format!("eval \"$(aida worktree enter {})\"    # cd in", arg).cyan()
            );
            eprintln!(
                "  {}",
                format!("claude    # then /aida-implement {}", out.focus).cyan()
            );
        }
        ("add", Some(_), _) => {
            eprintln!(
                "  {}",
                format!(
                    "aida worktree enter {}    # cd in, then /aida-implement {}",
                    arg, out.focus
                )
                .cyan()
            );
        }
        ("enter", None, true) => {
            eprintln!("  {}", "work in this shell; the cd already happened".cyan());
        }
        ("enter", None, false) => {
            eprintln!(
                "  {}",
                format!("eval \"$(aida worktree enter {})\"    # cd in", arg).cyan()
            );
        }
        _ => {
            eprintln!(
                "  {}",
                format!("aida worktree enter {}    # cd in", arg).cyan()
            );
        }
    }
    // Never suggest a raw `source` of the worktree's session-env file: a
    // branch can commit it. Run enter through the eval so the filtered
    // payload applies. trace:BUG-1624 | ai:claude
    if command == "enter" && out.has_session_env && !shell_payload_evaled {
        eprintln!(
            "  {}    {}",
            "cat .aida/session-env.sh".cyan(),
            "# review it; the eval'd enter applies only CARGO_TARGET_DIR, AIDA_AGENT_TYPE, AIDA_BIN"
                .dimmed()
        );
    }
    if let Some(id) = out.lease_id.as_deref() {
        eprintln!();
        eprintln!("When finished:");
        // trace:TASK-1160 | ai:claude
        if command == "enter" {
            eprintln!(
                "  {}",
                "aida worktree exit    # step back out (session stays live)".dimmed()
            );
        }
        eprintln!(
            "  {}",
            format!("aida session end {}", &id[..id.len().min(8)]).dimmed()
        );
    }
}

/// BUG-654: does the installed `aida()` shell wrapper auto-eval `worktree
/// enter`? The wrapper advertises which verb groups it auto-evals via the
/// comma-separated `AIDA_SHELL_WRAPPER` marker (exported from `SHELL_HELPERS`).
/// A wrapper installed before STORY-716 added `worktree enter` to its eval case
/// carries a marker WITHOUT the `worktree` capability; a shell with no wrapper
/// at all leaves the marker unset. Either way the `cd` line `worktree enter`
/// prints is never eval'd, so the cd silently no-ops. Returns true only when the
/// marker is present AND lists the `worktree` capability — the one wrapper shape
/// that will actually apply the cd.
// trace:BUG-654 | ai:claude
pub(crate) fn wrapper_can_eval_worktree_enter() -> bool {
    wrapper_marker_has_worktree_cap(std::env::var("AIDA_SHELL_WRAPPER").ok().as_deref())
}

/// Does the installed wrapper auto-eval `worktree exit`? A wrapper installed
/// before the exit verb existed advertises `worktree` (it evals `enter`) but
/// not `worktree-exit` — its case statement matches `"worktree enter"` only,
/// so the exit payload would print without being eval'd. Same staleness logic
/// as [`wrapper_can_eval_worktree_enter`], keyed on the newer capability.
// trace:TASK-1160 | ai:claude
pub(crate) fn wrapper_can_eval_worktree_exit() -> bool {
    wrapper_marker_has_cap(
        std::env::var("AIDA_SHELL_WRAPPER").ok().as_deref(),
        "worktree-exit",
    )
}

/// BUG-780: does the installed wrapper carry the prompt hook that drops the
/// `(wt:)` marker when the session it names has ended elsewhere? Only wrappers
/// advertising the `worktree-stale` capability self-heal; older ones need
/// `aida worktree exit` (or a hand-rolled `unset`) to clear a dangling marker.
// trace:BUG-780 | ai:claude
pub(crate) fn wrapper_can_self_heal_wt_marker() -> bool {
    wrapper_marker_has_cap(
        std::env::var("AIDA_SHELL_WRAPPER").ok().as_deref(),
        "worktree-stale",
    )
}

/// BUG-654: pure decision half of [`wrapper_can_eval_worktree_enter`] — given
/// the `AIDA_SHELL_WRAPPER` marker value (`None` when unset), is the `worktree`
/// capability advertised? Pure so the parsing is unit-testable without mutating
/// the process-global env var.
// trace:BUG-654 | ai:claude
pub(crate) fn wrapper_marker_has_worktree_cap(marker: Option<&str>) -> bool {
    wrapper_marker_has_cap(marker, "worktree")
}

/// Generalized capability probe over the comma-separated `AIDA_SHELL_WRAPPER`
/// marker: is `cap` advertised as an auto-evaled verb group? Whole-token,
/// case-insensitive compare — a substring of another token never counts.
// trace:BUG-654 trace:TASK-1160 trace:TASK-1171 | ai:claude
pub(crate) fn wrapper_marker_has_cap(marker: Option<&str>, cap: &str) -> bool {
    crate::shell_eval::marker_has_cap(marker, cap)
}

/// The single shell line `aida worktree enter` emits on stdout for the `aida()`
/// wrapper to auto-eval: `cd '<single-quote-escaped path>'`. Pure so the
/// emitted-shell contract is unit-testable without git.
// trace:STORY-716 | ai:claude
pub(crate) fn enter_cd_line(path: &std::path::Path) -> String {
    format!("cd '{}'", sh_single_quote(&path.display().to_string()))
}

/// TASK-1156: the wrapper-evaled worktree-enter payload carries the shell
/// mutations: cd into the worktree and source the generated session env exports
/// so CARGO_TARGET_DIR is live with no extra manual step. It also splices the
/// ambient `(wt:<focus>) ` segment into PS1 (recorded in `AIDA_WT_PS1_PREFIX`)
/// so the shell always shows where it is standing; `worktree exit` strips it.
// trace:TASK-1156 | ai:codex
// trace:TASK-1160 | ai:claude
pub(crate) fn enter_shell_payload(
    path: &std::path::Path,
    focus: &str,
    lease_file: Option<&std::path::Path>,
) -> String {
    let mut payload = format!("{}\n", enter_cd_line(path));
    let env_path = path.join(".aida").join("session-env.sh");
    if let Ok(body) = std::fs::read_to_string(env_path) {
        payload.push_str(&session_env_eval_lines(&body, &resolve_aida_exe()));
    }
    payload.push_str(&crate::worktree::ps1_wt_splice_block(focus, lease_file));
    payload
}

/// Re-render `.aida/session-env.sh` for the eval'd `worktree enter` payload.
///
/// The file lives in the worktree, so a branch can commit its own copy; its
/// body is never eval'd verbatim. Only the allowlisted names survive
/// ([`trusted_session_env`]), each value re-quoted. The PATH prepend comes
/// from `running_exe`, the binary doing the enter, never from the file.
// trace:BUG-1624 | ai:claude
pub(crate) fn session_env_eval_lines(body: &str, running_exe: &std::path::Path) -> String {
    let mut out = String::new();
    for (name, value) in trusted_session_env(body, running_exe) {
        out.push_str(&format!("export {name}={}\n", shell_single_quote(&value)));
        if name == "AIDA_BIN" {
            if let Some(dir) = running_exe.parent() {
                out.push_str(&format!(
                    "PATH={}:\"$PATH\"\n",
                    shell_single_quote(&dir.display().to_string())
                ));
            }
        }
    }
    out
}

/// `aida worktree list` — every registered git worktree annotated with its
/// `.aida/focus` (the epic it's scoped to, or `—` when unscoped).
// trace:STORY-716 | ai:claude
pub(crate) fn handle_worktree_list(json: bool) -> Result<()> {
    let main_root = find_main_worktree_root()?;
    let porcelain = std::process::Command::new("git")
        .arg("-C")
        .arg(&main_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .context("failed to invoke `git worktree list`")?;
    if !porcelain.status.success() {
        anyhow::bail!(
            "`git worktree list` failed: {}",
            String::from_utf8_lossy(&porcelain.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&porcelain.stdout);
    let paths = crate::worktree::parse_worktree_paths(&text);

    // Pair each worktree with its focus marker + current branch (re-parse the
    // porcelain for the branch line that follows each `worktree` record).
    let branches = parse_worktree_branches(&text);
    let rows: Vec<(std::path::PathBuf, Option<String>, Option<String>)> = paths
        .iter()
        .map(|p| {
            let focus = crate::focus::read_focus_marker(p);
            let branch = branches.get(p).cloned();
            (p.clone(), branch, focus)
        })
        .collect();

    if json {
        let arr: Vec<serde_json::Value> = rows
            .iter()
            .map(|(p, b, f)| {
                serde_json::json!({
                    "path": p.display().to_string(),
                    "branch": b,
                    "focus": f,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::json!(arr))?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No git worktrees found.");
        return Ok(());
    }
    println!("{}", "AIDA worktrees".bold());
    for (p, b, f) in &rows {
        let focus = f
            .as_deref()
            .map(|s| s.cyan().bold().to_string())
            .unwrap_or_else(|| "\u{2014}".dimmed().to_string());
        let branch = b.as_deref().unwrap_or("(detached)");
        println!(
            "  {}  [{}]  focus: {}",
            p.display().to_string().cyan(),
            branch,
            focus,
        );
    }
    Ok(())
}

/// Parse `git worktree list --porcelain` into a path -> branch-shortname map.
/// Each record is `worktree <path>` followed (when on a branch) by
/// `branch refs/heads/<name>`; a detached record has no branch line. Pure.
// trace:STORY-716 | ai:claude
pub(crate) fn parse_worktree_branches(
    porcelain: &str,
) -> std::collections::HashMap<std::path::PathBuf, String> {
    let mut map = std::collections::HashMap::new();
    let mut current: Option<std::path::PathBuf> = None;
    for line in porcelain.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current = Some(std::path::PathBuf::from(p.trim()));
        } else if let Some(b) = line.strip_prefix("branch ") {
            if let Some(p) = &current {
                let short = b.trim().strip_prefix("refs/heads/").unwrap_or(b.trim());
                map.insert(p.clone(), short.to_string());
            }
        }
    }
    map
}

/// The indented session-detail block shared by the live + stale renderings of
/// `aida status <spec>`.
// trace:STORY-694 | ai:claude
pub(crate) fn print_lease_detail(l: &SessionLease, elapsed_secs: u64) {
    println!("  {:<12} {}", "session:".dimmed(), l.id);
    println!(
        "  {:<12} {}",
        "role:".dimmed(),
        l.role.as_deref().unwrap_or("-")
    );
    println!(
        "  {:<12} {}",
        "worktree:".dimmed(),
        l.worktree_path.display()
    );
    println!(
        "  {:<12} {}",
        "started:".dimmed(),
        l.started_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
    );
    println!(
        "  {:<12} {}",
        "elapsed:".dimmed(),
        humanize_duration_secs(elapsed_secs)
    );
}
