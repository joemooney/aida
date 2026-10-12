//! `aida push`/`aida pull` handlers, mirror fan-out and stacked-branch rebase cascade (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// TASK-106: resolve the effective push scope. An explicit `--code-only`
/// or `--store-only` always wins. When the user passes neither,
/// `AIDA_PUSH_DEFAULT` can flip the default: `code`/`code-only` scopes
/// to the code leg, `store`/`store-only` to the orphan store; anything
/// else (including unset) keeps the historical both-legs default.
/// trace:TASK-106 | ai:claude
pub(crate) fn resolve_push_scope(
    code_only: bool,
    store_only: bool,
    env_value: Option<&str>,
) -> (bool, bool) {
    if code_only || store_only {
        return (code_only, store_only);
    }
    match env_value.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("code") | Some("code-only") => (true, false),
        Some("store") | Some("store-only") => (false, true),
        _ => (false, false),
    }
}

/// TASK-106: one leg of a `aida push` / `aida pull` pre-action summary.
/// `pending` is true when the leg actually has work to do (commits to
/// push, pending orphan changes) — used to decide whether to prompt.
pub(crate) struct LegStatus {
    pub(crate) label: &'static str,
    pub(crate) detail: String,
    pub(crate) pending: bool,
}

/// BUG-1626: total time budget for refreshing `origin/<branch>` before a
/// push — shared by the `git fetch` and its `ls-remote` fallback, so the
/// worst case is this value, not a multiple of it. A hung or offline remote
/// degrades the plan to "remote state unknown" instead of stalling.
// trace:BUG-1626 | ai:claude
pub(crate) const PUSH_REMOTE_REFRESH_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(20);

/// BUG-1626: how much the code leg knows about the remote branch it would
/// push to. Before this, the plan read only the locally cached
/// `origin/<branch>` ref, so a remote that had advanced since the last fetch
/// was reported as "up to date".
// trace:BUG-1626 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteBranchFreshness {
    /// `git fetch` just refreshed `origin/<branch>` — ahead/behind is authoritative.
    Fresh,
    /// origin reachable, but it has no such branch — first publish.
    Absent,
    /// origin could not be consulted (offline, timeout, auth, detached HEAD).
    Unknown(String),
}

/// BUG-1626: refresh `origin/<branch>` from the live remote with a bounded
/// fetch. On failure, `ls-remote --exit-code` distinguishes "the branch does
/// not exist on origin" (exit 2) from "origin unreachable". Both calls draw
/// on ONE `timeout` budget.
// trace:BUG-1626 | ai:claude
pub(crate) fn refresh_remote_branch_state(
    project_root: &std::path::Path,
    branch: &str,
    timeout: std::time::Duration,
) -> RemoteBranchFreshness {
    if branch == "HEAD" || branch.trim().is_empty() {
        return RemoteBranchFreshness::Unknown("detached HEAD".to_string());
    }
    if branch.starts_with('-') {
        return RemoteBranchFreshness::Unknown(format!("unsafe branch name `{branch}`"));
    }
    let deadline = std::time::Instant::now() + timeout;
    let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
    match run_git_with_timeout(
        project_root,
        &["fetch", "--quiet", "--no-tags", "origin", &refspec],
        timeout,
    ) {
        Ok(status) if status.success() => RemoteBranchFreshness::Fresh,
        Ok(_) => {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return RemoteBranchFreshness::Unknown(format!(
                    "`git fetch origin {branch}` failed and the refresh budget is spent"
                ));
            }
            let head_ref = format!("refs/heads/{branch}");
            match run_git_with_timeout(
                project_root,
                &["ls-remote", "--exit-code", "--heads", "origin", &head_ref],
                remaining,
            ) {
                Ok(s) if s.code() == Some(2) => RemoteBranchFreshness::Absent,
                _ => RemoteBranchFreshness::Unknown(format!("`git fetch origin {branch}` failed")),
            }
        }
        Err(e) => RemoteBranchFreshness::Unknown(e.to_string()),
    }
}

/// BUG-1626: the code leg's pre-push state, gathered ONCE per `aida push`
/// from freshly fetched remote state and reused by the plan, the TASK-494
/// no-op check, and the decision to skip a pointless push.
// trace:BUG-1626 | ai:claude
pub(crate) struct CodeLegPlan {
    pub(crate) branch: String,
    pub(crate) freshness: RemoteBranchFreshness,
    /// (ahead, behind) vs `origin/<branch>` AFTER the refresh; `None` when
    /// there is no tracking ref.
    pub(crate) ahead_behind: Option<(u32, u32)>,
    /// True when origin pushes to the same URL(s) it fetches from. When a
    /// `pushurl` / `pushInsteadOf` points elsewhere, fetch freshness says
    /// nothing about the push target, so the no-op skip is not taken.
    pub(crate) push_matches_fetch: bool,
}

/// BUG-1626: does `origin` push to exactly the URL it fetches from?
/// Unreadable config counts as "no" — the safe answer is to push.
/// BUG-1633: requires exactly ONE fetch URL. With several `remote.origin.url`
/// entries `git push` pushes to all of them, but the fetch only refreshed the
/// first — a lagging second URL would never be caught up.
// trace:BUG-1626 trace:BUG-1633 | ai:claude
pub(crate) fn origin_push_url_matches_fetch(project_root: &std::path::Path) -> bool {
    let urls = |push: bool| -> Option<Vec<String>> {
        let mut args = vec!["remote", "get-url", "--all"];
        if push {
            args.push("--push");
        }
        args.push("origin");
        let out = aida_core::git_ops::host_git_command(project_root)
            .args(&args)
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        let mut v: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        v.sort();
        Some(v)
    };
    match (urls(false), urls(true)) {
        (Some(fetch), Some(push)) => fetch.len() == 1 && fetch == push,
        _ => false,
    }
}

impl CodeLegPlan {
    /// Assumes `origin` exists (callers check `has_remote` first).
    pub(crate) fn gather(project_root: &std::path::Path, timeout: std::time::Duration) -> Self {
        let branch =
            aida_core::git_ops::current_branch(project_root).unwrap_or_else(|_| "HEAD".to_string());
        let freshness = refresh_remote_branch_state(project_root, &branch, timeout);
        let ahead_behind = ahead_behind_vs_ref(project_root, &branch, &format!("origin/{branch}"));
        let push_matches_fetch = origin_push_url_matches_fetch(project_root);
        Self {
            branch,
            freshness,
            ahead_behind,
            push_matches_fetch,
        }
    }

    /// True only when FRESH remote state proves there is nothing local to
    /// publish. A stale or unknown remote never counts as "nothing to push".
    /// A separate push URL is never judged by fetch state.
    pub(crate) fn nothing_to_push(&self) -> bool {
        self.push_matches_fetch
            && self.freshness == RemoteBranchFreshness::Fresh
            && matches!(self.ahead_behind, Some((0, _)))
    }

    pub(crate) fn status(&self) -> LegStatus {
        let b = &self.branch;
        let plural = |n: u32| if n == 1 { "" } else { "s" };
        let (detail, pending) = match (&self.freshness, self.ahead_behind) {
            (RemoteBranchFreshness::Fresh, Some((0, _))) if !self.push_matches_fetch => (
                format!(
                    "{b} → origin (no new commits for origin's fetch URL; origin has a separate push URL — will push)"
                ),
                true,
            ),
            (RemoteBranchFreshness::Unknown(why), _) => (
                format!(
                    "{b} → origin (remote state unknown — could not refresh origin/{b}: {why}; will attempt push)"
                ),
                true,
            ),
            (RemoteBranchFreshness::Absent, _) | (RemoteBranchFreshness::Fresh, None) => {
                (format!("{b} → origin (new branch — will publish)"), true)
            }
            (RemoteBranchFreshness::Fresh, Some((0, 0))) => {
                (format!("{b} → origin (up to date, nothing to push)"), false)
            }
            (RemoteBranchFreshness::Fresh, Some((0, behind))) => (
                format!(
                    "{b} → origin (behind origin by {behind} commit{}, nothing to push — run `aida pull`)",
                    plural(behind)
                ),
                false,
            ),
            (RemoteBranchFreshness::Fresh, Some((ahead, 0))) => (
                format!("{b} → origin ({ahead} commit{} ahead)", plural(ahead)),
                true,
            ),
            (RemoteBranchFreshness::Fresh, Some((ahead, behind))) => (
                format!(
                    "{b} → origin (diverged: {ahead} ahead, {behind} behind — push will be rejected; \
                     rebase first with `git pull --rebase origin {b}`)"
                ),
                true,
            ),
        };
        LegStatus {
            label: "code",
            detail,
            pending,
        }
    }
}

/// TASK-106: describe what the code leg of `aida push` will do.
/// BUG-1626: built from a freshly fetched [`CodeLegPlan`], never from the
/// cached tracking ref alone.
// trace:TASK-106 | ai:claude
pub(crate) fn push_code_leg_status(plan: Option<&CodeLegPlan>) -> LegStatus {
    match plan {
        Some(plan) => plan.status(),
        None => LegStatus {
            label: "code",
            detail: "no `origin` remote — will skip".to_string(),
            pending: false,
        },
    }
}

/// BUG-1626: the outcome of one `aida push` leg, retained so a rejected leg
/// is never laundered into a zero exit by the other leg succeeding.
// trace:BUG-1626 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PushLegOutcome {
    /// Out of scope (`--code-only` / `--store-only`).
    NotRequested,
    /// In scope but skipped (no `origin`, no orphan worktree).
    Skipped,
    Pushed,
    /// Fresh remote state proved there was nothing to publish.
    UpToDate,
    /// BUG-1633: fresh remote state shows origin is AHEAD of the local
    /// branch — nothing to push, but the branch is not "up to date".
    // trace:BUG-1633 | ai:claude
    NothingToPush,
    Failed(String),
}

/// BUG-1626: the "Push anyway? [y/N]" gate used by the merged-PR and
/// behind-main checks. Without a terminal it does not prompt (stdin would
/// read EOF and silently abort with exit 0); it fails nonzero and names the
/// flag that skips the check. A declined prompt is also nonzero, so
/// scripted and interactive runs agree: nothing pushed ⇒ nonzero exit.
// trace:BUG-1626 | ai:claude
pub(crate) fn confirm_push_anyway(
    interactive: bool,
    reason: &str,
    fix_hint: &str,
    read_answer: impl FnOnce() -> String,
) -> Result<()> {
    if !interactive {
        anyhow::bail!(
            "aida push stopped before pushing: {reason}. There is no terminal to confirm, \
             so nothing was pushed. {fix_hint} To push anyway, re-run with --no-rebase-check."
        );
    }
    eprintln!("  Push anyway? [y/N] (or pass --no-rebase-check to skip this check)");
    let answer = read_answer();
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        anyhow::bail!("aida push aborted: {reason}. Nothing was pushed. {fix_hint}")
    }
}

pub(crate) fn read_stdin_answer() -> String {
    use std::io::Write;
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    answer
}

/// BUG-1626: `None` when no requested leg failed; otherwise the error text
/// naming which leg failed, which (if any) succeeded, and how to recover.
// trace:BUG-1626 | ai:claude
pub(crate) fn push_failure_summary(
    code: &PushLegOutcome,
    store: &PushLegOutcome,
    branch: &str,
) -> Option<String> {
    let code_failed = matches!(code, PushLegOutcome::Failed(_));
    let store_failed = matches!(store, PushLegOutcome::Failed(_));
    if !code_failed && !store_failed {
        return None;
    }
    let describe = |label: &str, o: &PushLegOutcome| -> Option<String> {
        match o {
            PushLegOutcome::NotRequested => None,
            PushLegOutcome::Skipped => Some(format!("{label} leg skipped")),
            PushLegOutcome::Pushed => Some(format!("{label} leg pushed")),
            PushLegOutcome::UpToDate => Some(format!("{label} leg already up to date")),
            PushLegOutcome::NothingToPush => Some(format!(
                "{label} leg nothing to push (behind origin — run `aida pull`)"
            )),
            PushLegOutcome::Failed(why) => Some(format!("{label} leg FAILED ({why})")),
        }
    };
    let parts: Vec<String> = [describe("code", code), describe("store", store)]
        .into_iter()
        .flatten()
        .collect();
    let headline = if code == &PushLegOutcome::Pushed || store == &PushLegOutcome::Pushed {
        "partial success"
    } else {
        "failed"
    };
    let mut msg = format!("aida push {headline} — {}.", parts.join("; "));
    if code_failed {
        msg.push_str(&format!(
            "\n  Recover code: `aida pull --code-only` (or `git pull --rebase origin {branch}` \
             if the branch diverged), then `aida push --code-only`."
        ));
    }
    if store_failed {
        msg.push_str(
            "\n  Recover store: `aida pull --store-only` (or `aida db sync --pull`), \
             then `aida push --store-only`.",
        );
    }
    Some(msg)
}

/// TASK-106: describe what the orphan-store leg of `aida push` will do.
/// trace:TASK-106 | ai:claude
pub(crate) fn push_store_leg_status(store_path: &std::path::Path) -> LegStatus {
    use aida_core::git_ops;
    if !git_ops::is_git_repo(store_path) {
        return LegStatus {
            label: "store",
            detail: "no orphan worktree — will skip".to_string(),
            pending: false,
        };
    }
    if !git_ops::has_remote(store_path, "origin") {
        return LegStatus {
            label: "store",
            detail: "orphan store has no `origin` — will skip".to_string(),
            pending: false,
        };
    }
    let has_changes = git_ops::has_changes(store_path).unwrap_or(false);
    let ahead = orphan_branch_sync_state(store_path)
        .map(|(a, _)| a)
        .unwrap_or(0);
    if has_changes || ahead > 0 {
        let mut parts: Vec<String> = Vec::new();
        if ahead > 0 {
            parts.push(format!(
                "{} commit{} ahead",
                ahead,
                if ahead == 1 { "" } else { "s" }
            ));
        }
        if has_changes {
            parts.push("uncommitted changes to bundle".to_string());
        }
        LegStatus {
            label: "store",
            detail: format!("aida-store → origin ({})", parts.join(", ")),
            pending: true,
        }
    } else {
        LegStatus {
            label: "store",
            detail: "aida-store → origin (up to date, nothing to push)".to_string(),
            pending: false,
        }
    }
}

/// TASK-106: print the pre-push plan and, when BOTH legs have commits to
/// push (the only genuinely ambiguous case), prompt for confirmation.
/// A single-leg push is unsurprising and proceeds silently. The summary
/// itself is shown only on an interactive stdin so scripted/CI output
/// stays stable; the prompt is additionally gated on `!no_rebase_check`
/// (the established "skip interactive checks" signal). Returns `false`
/// when the user declines. trace:TASK-106 | ai:claude
pub(crate) fn confirm_push_plan(legs: &[LegStatus], no_rebase_check: bool) -> bool {
    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if !interactive {
        return true;
    }
    println!("{}", "Push plan:".bold());
    for leg in legs {
        let marker = if leg.pending {
            "→".cyan().to_string()
        } else {
            "·".dimmed().to_string()
        };
        println!("  {} {:<6} {}", marker, leg.label, leg.detail);
    }
    let pending = legs.iter().filter(|l| l.pending).count();
    if pending < 2 || no_rebase_check {
        return true;
    }
    use std::io::Write;
    eprint!("Push both legs? [Y/n] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no") {
        eprintln!("{}", "Push aborted.".dimmed());
        false
    } else {
        true
    }
}

/// TASK-108: count commits matching a git rev-list spec. 0 on failure.
/// trace:TASK-108 | ai:claude
pub(crate) fn commit_count(repo: &std::path::Path, spec: &[&str]) -> usize {
    let mut args = vec!["rev-list", "--count"];
    args.extend_from_slice(spec);
    aida_core::git_ops::host_git_command(repo)
        .args(&args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

/// TASK-108: list "shorthash subject" lines for a git rev-list spec,
/// capped at `limit`. Empty on any git failure. trace:TASK-108 | ai:claude
pub(crate) fn commit_subjects(repo: &std::path::Path, spec: &[&str], limit: usize) -> Vec<String> {
    let limit_arg = format!("-n{}", limit);
    let mut args = vec!["log", "--format=%h %s", limit_arg.as_str()];
    args.extend_from_slice(spec);
    aida_core::git_ops::host_git_command(repo)
        .args(&args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// TASK-108: one leg of a `--dry-run` report.
pub(crate) struct DryRunLeg {
    pub(crate) label: &'static str,
    pub(crate) summary: String,
    pub(crate) count: usize,
    pub(crate) subjects: Vec<String>,
}

/// TASK-108: render the dry-run plan — human-readable by default, JSON
/// with `--json`. trace:TASK-108 | ai:claude
pub(crate) fn emit_dry_run(verb: &str, legs: &[DryRunLeg], json: bool) {
    if json {
        let arr: Vec<serde_json::Value> = legs
            .iter()
            .map(|l| {
                serde_json::json!({
                    "leg": l.label,
                    "summary": l.summary,
                    "count": l.count,
                    "commits": l.subjects,
                })
            })
            .collect();
        let obj = serde_json::json!({ "verb": verb, "dry_run": true, "legs": arr });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).unwrap_or_else(|_| "{}".to_string())
        );
        return;
    }
    println!(
        "{} {}",
        format!("aida {} --dry-run", verb).bold(),
        "(no changes made)".dimmed()
    );
    for leg in legs {
        println!("  {} {}", leg.label.cyan().bold(), leg.summary);
        for s in &leg.subjects {
            println!("      {}", s.dimmed());
        }
        if leg.count > leg.subjects.len() {
            println!(
                "      {}",
                format!("… and {} more", leg.count - leg.subjects.len()).dimmed()
            );
        }
    }
}

pub(crate) fn run_git_with_timeout(
    cwd: &std::path::Path,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<std::process::ExitStatus> {
    // BUG-1626: output is discarded, so a credential prompt would be an
    // invisible hang until the timeout — fail fast instead. BUG-1929 F1: the
    // hardened builder, since `cwd` is the store for the push-plan fetch and
    // the auto-push.
    // trace:BUG-1626 trace:BUG-1929 | ai:claude
    let mut child = aida_core::git_ops::host_git_command(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("spawn git {}", args.join(" ")))?;
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "git {} timed out after {}s",
                args.join(" "),
                timeout.as_secs()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

pub(crate) fn auto_push_store_best_effort(store_path: &std::path::Path, reason: &str) {
    use aida_core::git_ops;

    if !git_ops::is_git_repo(store_path) || !git_ops::has_remote(store_path, "origin") {
        return;
    }
    // trace:TASK-1717 | ai:claude — commit pending changes and capture the
    // exact commit under the store write lock; the timed push (and its
    // pre-push hook) runs after release and names that commit.
    let captured = aida_core::db::with_store_write_lock(store_path, || {
        let branch =
            git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());
        // A failing `git status` defers the push (warned below); it is
        // never read as "nothing pending".
        if git_ops::has_changes_checked(store_path)? {
            git_ops::add_all(store_path, ".")
                .and_then(|_| git_ops::commit(store_path, "chore: sync pending changes"))
                .context("could not commit pending store changes")?;
        }
        Ok((branch, git_ops::head_sha(store_path)?))
    });
    let (branch, sha) = match captured {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "  {} stored locally; push deferred ({reason}: {e:#})",
                "Warning:".yellow().bold()
            );
            return;
        }
    };
    // The same refusal `git_ops::push_exact` makes: never publish a store
    // holding an embedded repository. trace:BUG-1929 | ai:claude
    if let Err(e) = git_ops::refuse_embedded_repos_for_push(store_path, &sha) {
        eprintln!(
            "  {} stored locally; push deferred ({reason}: {e:#})",
            "Warning:".yellow().bold()
        );
        return;
    }
    let refspec = format!("{sha}:refs/heads/{branch}");
    match run_git_with_timeout(
        store_path,
        &["push", "origin", &refspec],
        std::time::Duration::from_secs(5),
    ) {
        Ok(status) if status.success() => {
            eprintln!(
                "  {} auto-pushed aida-store ({reason})",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        }
        Ok(_) => {
            eprintln!(
                "  {} stored locally; push deferred ({reason}: origin rejected or unreachable)",
                "Warning:".yellow().bold()
            );
        }
        Err(e) => {
            eprintln!(
                "  {} stored locally; push deferred ({reason}: {e})",
                "Warning:".yellow().bold()
            );
        }
    }
}

/// Digest the local mailbox layer into the git-canonical orphan store and
/// commit it locally. Shared by the manual `mailbox sync` command and the
/// auto-triggers (session-end / drain-end). Returns the number of messages
/// newly digested (0 = nothing new). Append-only/id-keyed, so this is
/// idempotent — re-running digests nothing. trace:STORY-493 | ai:claude
// trace:TASK-1717 | ai:claude — the local layer is read first, without the
// store lock; the canonical re-read, writes, stage and commit then run as one
// store-writer transaction, so no other writer or rebase lands in between.
pub(crate) fn digest_mailbox_to_canonical(
    store_root: &std::path::Path,
    project_root: &std::path::Path,
) -> Result<usize> {
    let local = mailbox_store::read_local_messages(project_root)?;
    aida_core::db::with_store_write_lock(store_root, || {
        // A store whose status cannot be read is refused before any
        // canonical write.
        aida_core::git_ops::has_changes_checked(store_root)?;
        let n = mailbox_store::digest_snapshot_to_canonical(store_root, &local)?;
        if n == 0 {
            return Ok(0);
        }
        aida_core::git_ops::add(store_root, &["mailbox"])?;
        aida_core::git_ops::commit(store_root, &format!("mailbox: digest {n} message(s)"))?;
        Ok(n)
    })
}

/// STORY-643: project-wide opt-out for the auto mailbox sync wired into the
/// `aida pull` / `aida push` store legs. Defaults to on; disable with
/// `AIDA_MAILBOX_AUTOSYNC=0` (or `false` / `no` / `off`) or
/// `[mailbox] autosync = false` in `.aida/config.toml`. The env knob wins over
/// the config file (matching the rest of the AIDA_* surface). trace:STORY-643
pub(crate) fn mailbox_autosync_enabled(project_root: &std::path::Path) -> bool {
    // Env first — an explicit AIDA_MAILBOX_AUTOSYNC overrides the config file.
    if let Ok(v) = std::env::var("AIDA_MAILBOX_AUTOSYNC") {
        return !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        );
    }
    // Then the project config; absent / unparseable / unset → default on.
    if let Ok(body) = std::fs::read_to_string(project_root.join(".aida").join("config.toml")) {
        if let Ok(value) = body.parse::<toml::Value>() {
            if let Some(b) =
                config_lookup(Some(&value), "mailbox", "autosync").and_then(|v| v.as_bool())
            {
                return b;
            }
        }
    }
    true
}

/// STORY-643: best-effort PUBLISH leg for the auto mailbox sync. Digests the
/// local `.aida/mailbox/` layer into the canonical `<store>/mailbox/` WITHOUT
/// committing — the caller's store leg already commits pending orphan changes,
/// so the digested files fold into that single commit (no second commit/push).
/// Idempotent + id-keyed, so it is safe to call on every sync. Honors the
/// `mailbox_autosync_enabled` opt-out and never errors out the host command:
/// any failure is logged as a warning and swallowed. Returns the number of
/// messages newly staged (0 = nothing new / disabled / no local layer).
/// trace:STORY-643 | ai:claude
pub(crate) fn maybe_publish_mailbox_for_sync(store_path: &std::path::Path, reason: &str) -> usize {
    match mailbox_publish_snapshot(store_path, reason) {
        Some(local) => {
            let n = publish_mailbox_snapshot(store_path, &local, reason);
            report_mailbox_published(n, reason);
            n
        }
        None => 0,
    }
}

/// The local mailbox messages a sync publishes, read BEFORE the sync takes
/// the store write lock (local reads and sends never need it). `None` when
/// autosync is off or the local layer cannot be read (warned).
// trace:TASK-1717 | ai:claude
pub(crate) fn mailbox_publish_snapshot(
    store_path: &std::path::Path,
    reason: &str,
) -> Option<Vec<aida_core::mailbox::Message>> {
    let project_root = store_path.parent()?;
    if !mailbox_autosync_enabled(project_root) {
        return None;
    }
    match mailbox_store::read_local_messages(project_root) {
        Ok(local) => Some(local),
        Err(e) => {
            eprintln!(
                "  {} mailbox publish skipped ({reason}): {e}",
                "Warning:".yellow().bold()
            );
            None
        }
    }
}

/// Write a [`mailbox_publish_snapshot`] into the canonical store, inside the
/// caller's store-writer transaction (the sync's pending-changes commit picks
/// the files up). Returns how many messages were written; the caller reports
/// them with [`report_mailbox_published`] only once its commit succeeded.
/// Best-effort: a failure is warned and counts as nothing written.
// trace:TASK-1717 | ai:claude
pub(crate) fn publish_mailbox_snapshot(
    store_path: &std::path::Path,
    local: &[aida_core::mailbox::Message],
    reason: &str,
) -> usize {
    match mailbox_store::digest_snapshot_to_canonical(store_path, local) {
        Ok(n) => n,
        Err(e) => {
            eprintln!(
                "  {} mailbox publish skipped ({reason}): {e}",
                "Warning:".yellow().bold()
            );
            0
        }
    }
}

/// Report mailbox messages a sync committed to the canonical store.
// trace:TASK-1717 | ai:claude
pub(crate) fn report_mailbox_published(n: usize, reason: &str) {
    if n > 0 {
        eprintln!(
            "  {} published {n} mailbox message(s) to the canonical store ({reason})",
            crate::glyph(crate::glyphs::Glyph::Mailbox).dimmed()
        );
    }
}

/// Best-effort mailbox digest at a lifecycle boundary (session-end, drain-end).
/// A digest failure must NOT abort session cleanup or break the drain, so any
/// error is logged as a warning and swallowed. No-ops cleanly when there is no
/// local mailbox layer / nothing new to digest. trace:STORY-493 | ai:claude
pub(crate) fn maybe_digest_mailbox_best_effort(store_path: &std::path::Path, reason: &str) {
    let Some(project_root) = store_path.parent() else {
        return;
    };
    match digest_mailbox_to_canonical(store_path, project_root) {
        Ok(0) => {}
        Ok(n) => eprintln!(
            "  {} digested {n} mailbox message(s) to the canonical store ({reason})",
            crate::glyph(crate::glyphs::Glyph::Mailbox).dimmed()
        ),
        Err(e) => eprintln!(
            "  {} mailbox digest skipped ({reason}): {e}",
            "Warning:".yellow().bold()
        ),
    }
}

#[cfg(test)]
#[path = "tests/mailbox_digest_autotrigger_tests.rs"]
mod mailbox_digest_autotrigger_tests;

pub(crate) fn maybe_auto_push_store(
    store_path: &std::path::Path,
    mode: StoreAutoPushMode,
    reason: &str,
) {
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    match read_store_sync_config(&project_root) {
        Ok(cfg) if cfg.auto_push == mode => auto_push_store_best_effort(store_path, reason),
        Ok(cfg) if cfg.auto_push == StoreAutoPushMode::Periodic => {
            warn_if_periodic_auto_push(&project_root);
        }
        Ok(_) => {}
        Err(e) => eprintln!(
            "  {} store auto-push config ignored: {e}",
            "Warning:".yellow().bold()
        ),
    }
}

pub(crate) fn store_sync_config_project_root(storage: &Storage) -> std::path::PathBuf {
    storage
        .path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

/// TASK-108: `aida push --dry-run` — report what each in-scope leg would
/// push without pushing anything.
/// BUG-1633: the report is built from a fresh, time-bounded fetch (the same
/// shared budget `aida push` uses), never from the cached tracking refs alone.
// trace:TASK-108 trace:BUG-1633 | ai:claude
pub(crate) fn handle_push_dry_run(
    store_path: &std::path::Path,
    code_only: bool,
    store_only: bool,
    json: bool,
) -> Result<()> {
    let legs = push_dry_run_legs(
        store_path,
        code_only,
        store_only,
        PUSH_REMOTE_REFRESH_TIMEOUT,
    );
    emit_dry_run("push", &legs, json);
    Ok(())
}

/// BUG-1633: the legs of `aida push --dry-run`. Both legs refresh their
/// `origin/<branch>` ref first, drawing on ONE `budget` so an offline or hung
/// remote costs at most `budget` in total; a leg whose refresh failed says its
/// counts come from the cached ref.
// trace:TASK-108 trace:BUG-1633 | ai:claude
pub(crate) fn push_dry_run_legs(
    store_path: &std::path::Path,
    code_only: bool,
    store_only: bool,
    budget: std::time::Duration,
) -> Vec<DryRunLeg> {
    use aida_core::git_ops;
    const LIMIT: usize = 10;
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let deadline = std::time::Instant::now() + budget;
    let remaining = || deadline.saturating_duration_since(std::time::Instant::now());

    let mut legs: Vec<DryRunLeg> = Vec::new();

    if !store_only {
        let leg = if !git_ops::has_remote(&project_root, "origin") {
            DryRunLeg {
                label: "code",
                summary: "no `origin` remote — nothing to push".to_string(),
                count: 0,
                subjects: Vec::new(),
            }
        } else {
            let branch =
                git_ops::current_branch(&project_root).unwrap_or_else(|_| "HEAD".to_string());
            let origin_ref = format!("origin/{}", branch);
            let freshness = refresh_remote_branch_state(&project_root, &branch, remaining());
            let tracking = if freshness == RemoteBranchFreshness::Absent {
                None
            } else {
                ahead_behind_vs_ref(&project_root, &branch, &origin_ref)
            };
            let stale_note = match &freshness {
                RemoteBranchFreshness::Unknown(why) => format!(
                    " (remote state unknown — could not refresh {origin_ref}: {why}; \
                     counts use the cached ref)"
                ),
                _ => String::new(),
            };
            match tracking {
                Some((ahead, behind)) => {
                    let range = format!("{}..HEAD", origin_ref);
                    let subjects = commit_subjects(&project_root, &[range.as_str()], LIMIT);
                    let mut summary = format!(
                        "{} → origin/{}: {} commit{} to push",
                        branch,
                        branch,
                        ahead,
                        if ahead == 1 { "" } else { "s" }
                    );
                    if behind > 0 && ahead > 0 {
                        summary += &format!(
                            " (DIVERGED — {} commit{} behind; rebase before pushing)",
                            behind,
                            if behind == 1 { "" } else { "s" }
                        );
                    } else if behind > 0 {
                        summary += &format!(
                            " (behind origin by {} commit{} — run `aida pull`)",
                            behind,
                            if behind == 1 { "" } else { "s" }
                        );
                    }
                    summary += &stale_note;
                    DryRunLeg {
                        label: "code",
                        summary,
                        count: ahead as usize,
                        subjects,
                    }
                }
                // No origin/<branch> — first push of a new branch.
                None => {
                    let spec = ["HEAD", "--not", "--remotes"];
                    let count = commit_count(&project_root, &spec);
                    let subjects = commit_subjects(&project_root, &spec, LIMIT);
                    DryRunLeg {
                        label: "code",
                        summary: format!(
                            "{} → origin (new branch — would publish {} commit{}){}",
                            branch,
                            count,
                            if count == 1 { "" } else { "s" },
                            stale_note
                        ),
                        count,
                        subjects,
                    }
                }
            }
        };
        legs.push(leg);
    }

    if !code_only {
        let leg = if !git_ops::is_git_repo(store_path) {
            DryRunLeg {
                label: "store",
                summary: "no orphan worktree — nothing to push".to_string(),
                count: 0,
                subjects: Vec::new(),
            }
        } else if !git_ops::has_remote(store_path, "origin") {
            DryRunLeg {
                label: "store",
                summary: "orphan store has no `origin` — nothing to push".to_string(),
                count: 0,
                subjects: Vec::new(),
            }
        } else {
            let store_branch =
                git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());
            let freshness = refresh_remote_branch_state(store_path, &store_branch, remaining());
            // BUG-1633: compare against the ref just refreshed, not a
            // hardcoded `origin/aida-store`. trace:BUG-1633 | ai:claude
            let store_origin_ref = format!("origin/{store_branch}");
            let (ahead, behind) = ahead_behind_vs_ref(store_path, &store_branch, &store_origin_ref)
                .map(|(a, b)| (a as usize, b as usize))
                .unwrap_or((0, 0));
            let store_range = format!("{store_origin_ref}..HEAD");
            let subjects = commit_subjects(store_path, &[store_range.as_str()], LIMIT);
            let mut summary = format!(
                "aida-store → origin: {} commit{} to push",
                ahead,
                if ahead == 1 { "" } else { "s" }
            );
            if behind > 0 {
                summary += &format!(
                    " (origin has {} store commit{} you do not — pull first)",
                    behind,
                    if behind == 1 { "" } else { "s" }
                );
            }
            if git_ops::has_changes(store_path).unwrap_or(false) {
                summary += " (+ uncommitted changes that would be bundled)";
            }
            if let RemoteBranchFreshness::Unknown(why) = &freshness {
                summary += &format!(
                    " (remote state unknown — could not refresh origin/{store_branch}: {why}; \
                     counts use the cached ref)"
                );
            }
            DryRunLeg {
                label: "store",
                summary,
                count: ahead,
                subjects,
            }
        };
        legs.push(leg);
    }

    legs
}

/// TASK-108: `aida pull --dry-run` — fetch both in-scope legs, then
/// report what each would pull without merging. trace:TASK-108 | ai:claude
pub(crate) fn handle_pull_dry_run(
    store_path: &std::path::Path,
    code_only: bool,
    store_only: bool,
    json: bool,
) -> Result<()> {
    use aida_core::git_ops;
    const LIMIT: usize = 10;
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Refresh origin refs first so the behind-counts reflect reality.
    let _ = handle_fetch_command(store_path, code_only, store_only, true);

    let mut legs: Vec<DryRunLeg> = Vec::new();

    if !store_only {
        let leg = if !git_ops::has_remote(&project_root, "origin") {
            DryRunLeg {
                label: "code",
                summary: "no `origin` remote — nothing to pull".to_string(),
                count: 0,
                subjects: Vec::new(),
            }
        } else {
            let branch =
                git_ops::current_branch(&project_root).unwrap_or_else(|_| "HEAD".to_string());
            let origin_ref = format!("origin/{}", branch);
            match ahead_behind_vs_ref(&project_root, &branch, &origin_ref) {
                Some((ahead, behind)) => {
                    let range = format!("HEAD..{}", origin_ref);
                    let subjects = commit_subjects(&project_root, &[range.as_str()], LIMIT);
                    let how = if behind == 0 {
                        "up to date".to_string()
                    } else if ahead == 0 {
                        format!(
                            "{} commit{} to pull (fast-forward)",
                            behind,
                            if behind == 1 { "" } else { "s" }
                        )
                    } else {
                        format!(
                            "{} commit{} to pull ({} local commit{} → rebase/merge needed)",
                            behind,
                            if behind == 1 { "" } else { "s" },
                            ahead,
                            if ahead == 1 { "" } else { "s" }
                        )
                    };
                    DryRunLeg {
                        label: "code",
                        summary: format!("{} ← origin/{}: {}", branch, branch, how),
                        count: behind as usize,
                        subjects,
                    }
                }
                None => DryRunLeg {
                    label: "code",
                    summary: format!("no origin/{} ref — nothing to pull", branch),
                    count: 0,
                    subjects: Vec::new(),
                },
            }
        };
        legs.push(leg);
    }

    if !code_only {
        let leg = if !git_ops::is_git_repo(store_path) {
            DryRunLeg {
                label: "store",
                summary: "no orphan worktree — nothing to pull".to_string(),
                count: 0,
                subjects: Vec::new(),
            }
        } else if !git_ops::has_remote(store_path, "origin") {
            DryRunLeg {
                label: "store",
                summary: "orphan store has no `origin` — nothing to pull".to_string(),
                count: 0,
                subjects: Vec::new(),
            }
        } else {
            let (ahead, behind) = orphan_branch_sync_state(store_path).unwrap_or((0, 0));
            let subjects = commit_subjects(store_path, &["HEAD..origin/aida-store"], LIMIT);
            let how = if behind == 0 {
                "up to date".to_string()
            } else if ahead == 0 {
                format!(
                    "{} commit{} to pull (fast-forward)",
                    behind,
                    if behind == 1 { "" } else { "s" }
                )
            } else {
                format!(
                    "{} commit{} to pull ({} local commit{} → rebase)",
                    behind,
                    if behind == 1 { "" } else { "s" },
                    ahead,
                    if ahead == 1 { "" } else { "s" }
                )
            };
            DryRunLeg {
                label: "store",
                summary: format!("aida-store ← origin: {}", how),
                count: behind,
                subjects,
            }
        };
        legs.push(leg);
    }

    emit_dry_run("pull", &legs, json);
    Ok(())
}

/// TASK-863: count uncommitted/unstaged changes in the code working tree via a
/// single cheap `git status --porcelain`. Returns `Some(n)` when there are `n`
/// changed entries (n > 0), `None` for a clean tree (so the caller stays
/// silent) or when status can't be read (e.g. not a git repo) — we never
/// fabricate a count or block on a status failure. Each porcelain line is one
/// changed path (staged and/or unstaged), so the line count is the change
/// count. trace:TASK-863 | ai:claude
pub(crate) fn uncommitted_change_count(repo: &std::path::Path) -> Option<usize> {
    let out = aida_core::git_ops::host_git_command(repo)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let n = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    if n == 0 {
        None
    } else {
        Some(n)
    }
}

/// TASK-863: the uncommitted-changes notice is suppressible via the
/// `AIDA_PUSH_QUIET` env var (any non-empty, non-"0"/"false" value). The
/// `--no-notice` flag is the per-invocation equivalent. trace:TASK-863 | ai:claude
pub(crate) fn push_notice_suppressed_by_env() -> bool {
    match std::env::var("AIDA_PUSH_QUIET") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !v.is_empty() && v != "0" && v != "false" && v != "no"
        }
        Err(_) => false,
    }
}

/// Best-effort fan-out of a just-pushed `branch` to every configured mirror
/// remote (`[store.sync] mirror_remotes`). `repo` is the git dir to push from,
/// `project_root` is where `.aida/config.toml` lives. A non-ff / unreachable /
/// unconfigured mirror is still non-fatal, but it is recorded as remote drift
/// and notify'd when configured so the operator is not left with silent mirror
/// drift. Shared by `aida db sync --push` (store leg) and `aida push` (both
/// legs) so a clone can't silently leave one hub behind.
// trace:STORY-760 | ai:claude
/// Fan a store push out to the mirrors, pinned to `origin_sha`: the exact
/// commit origin just accepted, never the local branch (which another writer
/// may have advanced since the push).
// trace:TASK-1717 | ai:claude
pub(crate) fn fan_out_store_mirror_push(
    store: &std::path::Path,
    branch: &str,
    origin_sha: &str,
    project_root: &std::path::Path,
) {
    let refspec = format!("{origin_sha}:refs/heads/{branch}");
    fan_out_mirror_refspec(store, branch, &refspec, project_root);
}

pub(crate) fn fan_out_mirror_push(
    repo: &std::path::Path,
    branch: &str,
    project_root: &std::path::Path,
) {
    let cfg = read_store_sync_config(project_root).unwrap_or_default();
    if cfg.mirror_remotes.is_empty() {
        return;
    }
    // Code mirrors must follow origin even if the local branch moves after
    // the successful push. Store fan-out retains its union/reconcile model.
    // trace:BUG-1803 | ai:codex
    let refspec = if branch == "aida-store" {
        branch.to_string()
    } else {
        match remote_create::origin_code_refspec(repo, branch) {
            Ok(refspec) => refspec,
            Err(e) => {
                eprintln!("  mirror push skipped: {e}");
                return;
            }
        }
    };
    fan_out_mirror_refspec(repo, branch, &refspec, project_root);
}

fn fan_out_mirror_refspec(
    repo: &std::path::Path,
    branch: &str,
    refspec: &str,
    project_root: &std::path::Path,
) {
    let cfg = read_store_sync_config(project_root).unwrap_or_default();
    let warn = crate::glyph(crate::glyphs::Glyph::Warning);
    for mirror in &cfg.mirror_remotes {
        if mirror == "origin" {
            continue;
        }
        if !aida_core::git_ops::has_remote(repo, mirror) {
            eprintln!("  {warn} mirror remote `{mirror}` not configured — skipping");
            // trace:TASK-1227 | ai:codex
            record_store_mirror_fanout_failure(
                project_root,
                repo,
                branch,
                mirror,
                "mirror remote is configured in [store.sync] but missing from this repo",
            );
            continue;
        }
        println!("Mirroring {branch} → {mirror}...");
        match aida_core::git_ops::push(repo, mirror, refspec) {
            Ok(true) => {
                println!("  Mirror push complete.");
                clear_store_mirror_fanout_failure(project_root, repo, branch, mirror);
            }
            Ok(false) => {
                let reason = "push rejected (diverged)";
                eprintln!(
                    "  {warn} mirror `{mirror}` rejected (diverged) — run `aida remote reconcile` to union-merge and re-sync every hub"
                );
                record_store_mirror_fanout_failure(project_root, repo, branch, mirror, reason);
            }
            Err(e) => {
                let reason = e.to_string();
                eprintln!("  {warn} mirror `{mirror}` push failed: {reason} — skipped");
                record_store_mirror_fanout_failure(project_root, repo, branch, mirror, &reason);
            }
        }
    }
}

pub(crate) fn handle_push_command(
    store_path: &std::path::Path,
    code_only: bool,
    store_only: bool,
    message: Option<&str>,
    no_rebase_check: bool,
    no_notice: bool,
) -> Result<()> {
    use aida_core::git_ops;

    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // TASK-863: gentle non-blocking notice when the code working tree has
    // uncommitted/unstaged changes that won't be part of this push. `git push`
    // pushes commits, not the working tree, so this is expected git behavior —
    // a hard warning (let alone a block) would be noise. We surface a single
    // informational line so the operator isn't surprised, then proceed
    // normally. Clean tree → silent. Suppress with --no-notice or
    // AIDA_PUSH_QUIET. Store-only pushes don't touch the code tree, so skip.
    // trace:TASK-863 | ai:claude
    if !store_only && !no_notice && !push_notice_suppressed_by_env() {
        if let Some(n) = uncommitted_change_count(&project_root) {
            eprintln!(
                "{} {} uncommitted change(s) not included in this push",
                "note:".dimmed(),
                n,
            );
        }
    }

    // STORY-537: with no `origin`, both push legs would silently skip. Offer a
    // guided origin bootstrap (TTY) so the user isn't left to manually create
    // the repo + `git remote add origin` + push. Declining (or no TTY) falls
    // through to the existing "no origin — skipping" path, which still works.
    // trace:STORY-537 | ai:claude
    if !store_only && !git_ops::has_remote(&project_root, "origin") {
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            if prompt_yes_no(
                "No `origin` remote — set one up now (create on a forge or attach existing)? [Y/n] ",
                true,
            )
            .unwrap_or(false)
            {
                remote_create::handle_remote_create(&project_root, None, false, None, true)?;
            }
        } else {
            // Non-interactive: print the manual recipe once so a scripted push
            // tells the operator how to bootstrap, then continue (legs skip).
            let repo_name = remote_create::default_repo_name(&project_root);
            let branch =
                git_ops::current_branch(&project_root).unwrap_or_else(|_| "main".to_string());
            println!("{}", remote_create::manual_recipe(&repo_name, &branch));
        }
    }

    // TASK-106: pre-push summary. Show what each in-scope leg will do
    // BEFORE touching origin; prompt only when both legs have commits
    // (the ambiguous case — a single-leg push is no surprise).
    // BUG-1626: the code-leg state is gathered ONCE from a bounded fetch, so
    // the plan, the no-op check and the push decision all agree and none of
    // them trusts a stale `origin/<branch>`. trace:BUG-1626 | ai:claude
    let code_plan: Option<CodeLegPlan> =
        if !store_only && git_ops::has_remote(&project_root, "origin") {
            Some(CodeLegPlan::gather(
                &project_root,
                PUSH_REMOTE_REFRESH_TIMEOUT,
            ))
        } else {
            None
        };
    // BUG-1626: per-leg outcomes — any failed leg makes the command exit
    // nonzero, in scripted and interactive runs alike. trace:BUG-1626
    let mut code_outcome = if store_only {
        PushLegOutcome::NotRequested
    } else {
        PushLegOutcome::Skipped
    };
    let mut store_outcome = if code_only {
        PushLegOutcome::NotRequested
    } else {
        PushLegOutcome::Skipped
    };
    let code_branch = code_plan
        .as_ref()
        .map(|p| p.branch.clone())
        .unwrap_or_else(|| "<branch>".to_string());

    {
        let mut legs: Vec<LegStatus> = Vec::new();
        if !store_only {
            legs.push(push_code_leg_status(code_plan.as_ref()));
        }
        if !code_only {
            legs.push(push_store_leg_status(store_path));
        }
        // BUG-1626: a declined plan pushes nothing, so it exits nonzero like
        // every other no-push outcome. trace:BUG-1626 | ai:claude
        if !confirm_push_plan(&legs, no_rebase_check) {
            anyhow::bail!("aida push aborted at the plan prompt. Nothing was pushed.");
        }
    }

    // ---- Code push (current branch on the project repo) ----
    if !store_only {
        if !git_ops::has_remote(&project_root, "origin") {
            println!(
                "  {} no `origin` remote — skipping code push",
                "Note:".dimmed()
            );
        } else if let Some(plan) = code_plan.as_ref() {
            let branch = plan.branch.clone();

            // TASK-494: when the code-leg has NOTHING to push (branch is
            // up-to-date with `origin/<branch>`), the merged-branch +
            // stale-base prompts below are pure noise — they warn about risks
            // that don't apply to a no-op push (the user saw them fire next to
            // "up to date, nothing to push"). They still fire when there ARE
            // unpushed commits (the real-risk case) and when the branch was
            // never pushed (`None` ≠ nothing-to-push → whole branch is unpushed).
            // trace:TASK-494 | ai:claude
            // BUG-1626: judged from the freshly fetched plan — a stale or
            // unreachable remote is never "nothing to push".
            let code_nothing_to_push = plan.nothing_to_push();

            // BUG-88: warn when pushing to a branch whose PR has
            // already merged. New commits land on `origin/<branch>`
            // but won't reach `main` without a fresh PR. Skipped on
            // --no-rebase-check (scripts/CI/automation), main itself,
            // and when gh isn't available (we can't confirm merged
            // state without it — silent rather than crying wolf).
            // trace:BUG-88 | ai:claude
            if !no_rebase_check
                && !code_nothing_to_push
                && branch != "main"
                && branch != "master"
                && matches!(
                    change_lookup_for_branch(&project_root, &branch),
                    crate::forge::ChangeLookup::NoChange
                )
            {
                if let PrLookup::Found(pr) =
                    detect_merged_pr_for_branch_via_forge(&project_root, &branch)
                {
                    eprintln!(
                        "{} branch {} was the head of {} which already merged.",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                        branch.cyan(),
                        format!("PR-{}", pr.number).cyan(),
                    );
                    eprintln!("  {}", pr.url.dimmed());
                    eprintln!(
                        "  {}",
                        "New commits will land on origin/<branch> but won't reach main without a fresh PR.".dimmed(),
                    );
                    eprintln!("  {}", "Fix paths:".dimmed());
                    eprintln!(
                        "    {} open a follow-up PR: {}",
                        "·".dimmed(),
                        format!("gh pr create --base main --head {} --title \"...\"", branch)
                            .cyan()
                    );
                    eprintln!(
                        "    {} cherry-pick onto a fresh branch off main: {}",
                        "·".dimmed(),
                        "git checkout -b <new-branch> origin/main && git cherry-pick <sha>".cyan()
                    );
                    // BUG-1626: never read stdin without a terminal, and
                    // never exit 0 when nothing was pushed.
                    // trace:BUG-1626 | ai:claude
                    confirm_push_anyway(
                        std::io::stdin().is_terminal(),
                        &format!(
                            "branch {branch} was the head of already-merged PR-{}",
                            pr.number
                        ),
                        "Open a follow-up PR or move the commits to a fresh branch.",
                        read_stdin_answer,
                    )?;
                }
            }

            // TASK-54: pre-flight "behind main" check. If the current
            // branch lags `main` by N commits, prompt before pushing
            // so the user gets a chance to rebase first (or push
            // anyway). Skipped: --no-rebase-check (scripts/CI),
            // pushing main itself, no upstream main locally, branch
            // is exactly up-to-date. trace:TASK-54 | ai:claude
            // TASK-494: also skipped when the code-leg has nothing to push.
            if !no_rebase_check && !code_nothing_to_push {
                if let Some((behind, sample)) = branch_behind_main(&project_root, &branch) {
                    eprintln!(
                        "{} {} is {} commit{} behind {}:",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                        branch.cyan(),
                        behind,
                        if behind == 1 { "" } else { "s" },
                        "main".cyan()
                    );
                    for s in sample.iter().take(5) {
                        eprintln!("    {}", s.dimmed());
                    }
                    if sample.len() > 5 {
                        eprintln!(
                            "    {}",
                            format!("… and {} more", sample.len() - 5).dimmed()
                        );
                    }
                    eprintln!(
                        "  {}",
                        "Rebase first to avoid stale-base review noise:".dimmed()
                    );
                    eprintln!("    {}", "git pull --rebase origin main".cyan());
                    // BUG-1626: same terminal gate as the merged-PR check.
                    // trace:BUG-1626 | ai:claude
                    confirm_push_anyway(
                        std::io::stdin().is_terminal(),
                        &format!(
                            "{branch} is {behind} commit{} behind main",
                            if behind == 1 { "" } else { "s" }
                        ),
                        "Rebase and re-run `aida push`.",
                        read_stdin_answer,
                    )?;
                }
            }

            println!("{} {} → origin", "Pushing code".cyan().bold(), branch);
            // BUG-1626: fresh remote state proves origin already has every
            // local commit — skip the pointless (and, when behind, doomed)
            // `git push`. trace:BUG-1626 | ai:claude
            let res = if code_nothing_to_push {
                None
            } else {
                Some(
                    aida_core::git_ops::host_git_command(&project_root)
                        .args(["push", "origin", &branch])
                        .status(),
                )
            };
            match res {
                None => {
                    // BUG-1633: behind origin is "nothing to push", not "up to
                    // date". trace:BUG-1633 | ai:claude
                    let behind_origin = matches!(plan.ahead_behind, Some((0, b)) if b > 0);
                    if behind_origin {
                        code_outcome = PushLegOutcome::NothingToPush;
                        println!(
                            "  {}",
                            "nothing to push (origin already has these commits)".green()
                        );
                    } else {
                        code_outcome = PushLegOutcome::UpToDate;
                        println!(
                            "  {}",
                            "code push complete (origin already has these commits)".green()
                        );
                    }
                    if let Some((0, behind)) = plan.ahead_behind.filter(|(_, b)| *b > 0) {
                        // Behind origin: the local branch is stale, so fanning
                        // it out would push an old tip at the mirrors.
                        eprintln!(
                            "  {} origin/{} has {} commit{} not in your branch — run `aida pull`.",
                            "Note:".dimmed(),
                            branch,
                            behind,
                            if behind == 1 { "" } else { "s" }
                        );
                    } else {
                        // BUG-1626: exactly level with origin — still fan out,
                        // exactly as the old no-op `git push` path did. This
                        // is how a lagging mirror catches up after the
                        // everyday merge → `aida pull` → `aida push` flow,
                        // and how recorded mirror drift gets cleared.
                        // trace:BUG-1626 | ai:claude
                        fan_out_mirror_push(&project_root, &branch, &project_root);
                        gitlab_mirror_link::sync_mirror_ci_link(&project_root, &branch);
                    }
                }
                Some(Ok(s)) if s.success() => {
                    code_outcome = PushLegOutcome::Pushed;
                    println!("  {}", "code push complete".green());
                    // STORY-760: fan the code branch out to every mirror hub so
                    // `aida push` can't leave one behind. Best-effort.
                    fan_out_mirror_push(&project_root, &branch, &project_root);
                    // TASK-1424: best-effort, bounded — if GitHub is the review
                    // surface and a GitLab mirror pipeline is known for this
                    // branch, link it onto the GitHub PR head as a non-blocking
                    // commit status. Never fails or delays this push.
                    gitlab_mirror_link::sync_mirror_ci_link(&project_root, &branch);
                }
                // BUG-1626: retain the rejection (and still attempt the
                // independent store leg) instead of warning and exiting 0.
                // trace:BUG-1626 | ai:claude
                Some(Ok(s)) => {
                    eprintln!(
                        "  {} git push exited with status {}",
                        "Warning:".yellow().bold(),
                        s
                    );
                    code_outcome = PushLegOutcome::Failed(format!(
                        "git push origin {branch} exited {s}; origin may have commits you do not have"
                    ));
                }
                Some(Err(e)) => {
                    eprintln!("  {} git push failed: {}", "Warning:".yellow().bold(), e);
                    code_outcome = PushLegOutcome::Failed(format!("could not run git push: {e}"));
                }
            }
        }
    }

    // ---- Store push (orphan branch via aida db sync) ----
    // BUG-44: only print the "Pushing store..." header when we'll actually
    // attempt a push. Mirroring the code-push leg above, which prints only
    // a Note line when there's no origin.
    // BUG-1626: no early return on a missing orphan worktree — that would
    // launder a failed code leg into a zero exit. trace:BUG-1626 | ai:claude
    if !code_only && !git_ops::is_git_repo(store_path) {
        println!(
            "  {} no orphan worktree — skipping store push",
            "Note:".dimmed()
        );
    } else if !code_only {
        let store_has_origin = git_ops::has_remote(store_path, "origin");
        if store_has_origin {
            println!("{} aida-store → origin", "Pushing store".cyan().bold());
        } else {
            println!(
                "  {} orphan store has no `origin` — skipping store push",
                "Note:".dimmed()
            );
        }
        // STORY-643: PUBLISH the local mailbox into the canonical store BEFORE
        // committing pending changes, so locally-sent messages fold into the
        // single store commit below and propagate with this push — no manual
        // `aida mailbox sync`. Best-effort + idempotent; folded into the same
        // commit (no second push). trace:STORY-643 | ai:claude
        // trace:TASK-1717 | ai:claude — local messages are read first; the
        // canonical digest, the pending-changes commit and the capture of the
        // commit to push are one store-writer transaction. The push names
        // that commit and runs after the lock is released.
        let mailbox = mailbox_publish_snapshot(store_path, "push");
        let window = aida_core::db::with_store_write_lock(store_path, || {
            // A store whose status cannot be read fails the leg before the
            // digest or anything else touches it.
            git_ops::has_changes_checked(store_path)
                .context("cannot read the store status; nothing was written or pushed")?;
            let published = mailbox
                .as_ref()
                .map(|local| publish_mailbox_snapshot(store_path, local, "push"))
                .unwrap_or(0);
            // Commit any pending orphan-branch changes regardless of origin —
            // the user's local edits should land in a commit either way so
            // subsequent operations have a clean tree. trace:BUG-44 | ai:claude
            // trace:TASK-1717 | ai:claude — a failed stage/commit (a refusing
            // hook) fails the leg with git's error: nothing is pushed and
            // the pending changes are kept, uncommitted.
            if git_ops::has_changes_checked(store_path)? {
                let msg = message.unwrap_or("chore: sync pending changes");
                let committed = git_ops::add(store_path, &["."])
                    .and_then(|_| git_ops::commit(store_path, msg))
                    .context("could not commit pending store changes; nothing was pushed")?;
                if committed {
                    println!("  Committed: {}", msg);
                }
            }
            report_mailbox_published(published, "push");
            if !store_has_origin {
                return Ok(None);
            }
            let branch =
                git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());
            Ok(Some((branch, git_ops::head_sha(store_path)?)))
        });
        match window {
            Ok(None) => {}
            Err(e) => {
                eprintln!("  {} store push failed: {e:#}", "Warning:".yellow().bold());
                store_outcome = PushLegOutcome::Failed(format!("store push failed: {e:#}"));
            }
            Ok(Some((branch, sha))) => {
                match git_ops::push_exact(store_path, "origin", &sha, &branch) {
                    Ok(true) => {
                        store_outcome = PushLegOutcome::Pushed;
                        println!("  {}", "store push complete".green());
                        // STORY-760: fan the store branch out to every mirror hub.
                        // trace:TASK-1717 | ai:claude — the commit origin accepted.
                        fan_out_store_mirror_push(store_path, &branch, &sha, &project_root);
                    }
                    // BUG-1626: a rejected store push is a failed leg, not a
                    // warning over a zero exit. trace:BUG-1626 | ai:claude
                    Ok(false) => {
                        eprintln!(
                            "  {} push rejected — pull/rebase first (`aida db sync --pull`)",
                            "Warning:".yellow().bold()
                        );
                        store_outcome = PushLegOutcome::Failed(format!(
                        "git push origin {branch} rejected; origin has store commits you do not have"
                    ));
                    }
                    Err(e) => {
                        eprintln!("  {} store push failed: {}", "Warning:".yellow().bold(), e);
                        store_outcome = PushLegOutcome::Failed(format!("store push failed: {e}"));
                    }
                }
            }
        }
    }

    // BUG-1626: any failed leg → nonzero exit with a partial-success summary
    // naming the failed leg and its recovery command. trace:BUG-1626 | ai:claude
    match push_failure_summary(&code_outcome, &store_outcome, &code_branch) {
        None => Ok(()),
        Some(summary) => Err(anyhow::anyhow!(summary)),
    }
}

/// TASK-106: print the pre-pull plan — which legs are in scope and the
/// action each will take. Shown only on an interactive stdin. Unlike
/// `aida push` there is no confirmation prompt: both legs are
/// non-destructive (`--ff-only` for code, rebase-pull for the
/// AIDA-managed orphan store), so there is no "surprise publish" to
/// guard against. trace:TASK-106 | ai:claude
pub(crate) fn print_pull_plan(
    project_root: &std::path::Path,
    store_path: &std::path::Path,
    code_only: bool,
    store_only: bool,
) {
    use aida_core::git_ops;
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return;
    }
    let mut legs: Vec<LegStatus> = Vec::new();
    if !store_only {
        let leg = if !git_ops::has_remote(project_root, "origin") {
            LegStatus {
                label: "code",
                detail: "no `origin` remote — will skip".to_string(),
                pending: false,
            }
        } else {
            let branch =
                git_ops::current_branch(project_root).unwrap_or_else(|_| "HEAD".to_string());
            LegStatus {
                label: "code",
                detail: format!("{} ← origin (git pull --ff-only)", branch),
                pending: true,
            }
        };
        legs.push(leg);
    }
    if !code_only {
        let leg = if !git_ops::is_git_repo(store_path) {
            LegStatus {
                label: "store",
                detail: "no orphan worktree — will skip".to_string(),
                pending: false,
            }
        } else if !git_ops::has_remote(store_path, "origin") {
            LegStatus {
                label: "store",
                detail: "orphan store has no `origin` — will skip".to_string(),
                pending: false,
            }
        } else {
            LegStatus {
                label: "store",
                detail: "aida-store ← origin (rebase pull)".to_string(),
                pending: true,
            }
        };
        legs.push(leg);
    }
    println!("{}", "Pull plan:".bold());
    for leg in &legs {
        let marker = if leg.pending {
            "←".cyan().to_string()
        } else {
            "·".dimmed().to_string()
        };
        println!("  {} {:<6} {}", marker, leg.label, leg.detail);
    }
}

// BUG-691: after a failed code-leg pull, restore any autostash git's built-in
// autostash mechanism left un-applied (the ff-update died — e.g. an index.lock
// race — after stashing the dirty tree and before re-applying it). Prints a
// note when it restores, and a clear warning naming the stash SHA + recovery
// step when it cannot. `pre_stash_top` is the stash-stack top captured before
// the pull, used to avoid touching a pre-existing/unrelated stash.
// trace:BUG-691 | ai:claude
pub(crate) fn report_autostash_restore(
    project_root: &std::path::Path,
    pre_stash_top: Option<&str>,
) {
    use aida_core::git_ops::AutostashRestore;
    let short = |sha: &str| sha.chars().take(9).collect::<String>();
    match aida_core::git_ops::restore_stranded_autostash(project_root, pre_stash_top) {
        AutostashRestore::Restored(sha) => {
            eprintln!(
                "  {} restored autostash {} to your working tree — the failed pull \
                 had left your uncommitted changes stashed.",
                "Note:".dimmed(),
                short(&sha),
            );
        }
        AutostashRestore::Stranded { sha, reason } => {
            eprintln!(
                "  {} the failed pull left an autostash ({}) that could not be \
                 restored automatically ({}). Your uncommitted work is safe in the \
                 stash — recover it with `git stash list` then \
                 `git stash pop` (once no other git process is running).",
                "Warning:".yellow().bold(),
                short(&sha),
                reason,
            );
        }
        AutostashRestore::Nothing => {}
    }
}

/// `aida pull` — symmetric counterpart of `aida push`. Pulls both the
/// current code branch (via `git pull --ff-only`) and the orphan store
/// (via `git_ops::pull_rebase`, matching `aida db sync --pull`). Each
/// leg skips cleanly when its remote isn't configured, so the command
/// is safe to run in any project state. trace:TASK-43 | ai:claude
// BUG-1500: `aida pull`'s store-leg failure message used to advise
// `git rebase --abort` unconditionally, on every store-leg error —
// including a transient network failure (e.g. a 502) where no rebase
// is in progress and the abort is the wrong action. Check the actual
// rebase state (a cheap filesystem stat via `git_ops::rebase_in_progress`)
// before recommending it, so a transient failure doesn't read as a
// broken/corrupted store.
// trace:BUG-1500 | ai:claude
// trace:TASK-1604 | ai:codex
pub(crate) fn store_pull_failure_hint(store_path: &std::path::Path, err_display: &str) -> String {
    if aida_core::git_ops::rebase_in_progress(store_path) {
        format!(
            "{}\n  The orphan store is mid-rebase. To recover:\n    \
                 cd {} && git rebase --abort\n  \
             Then re-run `aida pull` or `aida db sync --pull`.",
            err_display,
            store_path.display()
        )
    } else {
        format!(
            "{}\n  The store is not mid-rebase. Check the Git error above and resolve \
             any ref/configuration or concurrent-writer problem before re-running \
             `aida pull` or `aida db sync --pull`.",
            err_display
        )
    }
}

// trace:BUG-1626 | ai:claude
#[cfg(test)]
mod bug_1626_push_fresh_state_tests {
    use super::{
        confirm_push_anyway, handle_push_command, push_failure_summary, CodeLegPlan,
        PushLegOutcome, RemoteBranchFreshness,
    };
    use std::path::Path;
    use std::time::Duration;

    const T: Duration = Duration::from_secs(20);

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} in {} failed: {}",
            args,
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).expect("write");
        git(dir, &["add", file]);
        git(dir, &["commit", "-q", "-m", file]);
    }

    /// A bare remote `name.git` seeded with one commit on `branch`, plus a
    /// clone of it at `clone_dir`.
    fn remote_with_clone(
        root: &Path,
        name: &str,
        branch: &str,
        clone_dir: &Path,
    ) -> std::path::PathBuf {
        let bare = root.join(format!("{name}.git"));
        std::fs::create_dir_all(&bare).expect("mkdir bare");
        git(&bare, &["init", "-q", "--bare", "-b", branch]);
        std::fs::create_dir_all(clone_dir).expect("mkdir clone");
        git(clone_dir, &["init", "-q", "-b", branch]);
        git(
            clone_dir,
            &["remote", "add", "origin", bare.to_str().unwrap()],
        );
        commit(clone_dir, &format!("{name}-seed.txt"));
        git(clone_dir, &["push", "-q", "-u", "origin", branch]);
        bare
    }

    fn second_clone(bare: &Path, dir: &Path) {
        let out = std::process::Command::new("git")
            .args(["clone", "-q", bare.to_str().unwrap(), dir.to_str().unwrap()])
            .output()
            .expect("clone");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Project repo `proj` (code, branch main) with an orphan-store clone at
    /// `proj/.aida-store` (branch aida-store). Returns (code_bare, store_bare).
    fn project(root: &Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let proj = root.join("proj");
        let code_bare = remote_with_clone(root, "code", "main", &proj);
        std::fs::write(proj.join(".git/info/exclude"), ".aida-store/\n.aida/\n").unwrap();
        let store = proj.join(".aida-store");
        let store_bare = remote_with_clone(root, "store", "aida-store", &store);
        (proj, code_bare, store_bare)
    }

    fn advance_remote(root: &Path, bare: &Path, name: &str) {
        let other = root.join(format!("other-{name}"));
        second_clone(bare, &other);
        commit(&other, &format!("{name}-remote-advance.txt"));
        git(&other, &["push", "-q", "origin", "HEAD"]);
    }

    fn bare_head(bare: &Path, branch: &str) -> String {
        git(bare, &["rev-parse", &format!("refs/heads/{branch}")])
    }

    #[test]
    fn stale_tracking_ref_is_not_reported_up_to_date() {
        let tmp = tempfile::tempdir().unwrap();
        let (proj, code_bare, _) = project(tmp.path());
        advance_remote(tmp.path(), &code_bare, "code");
        // Local HEAD == stale local origin/main, remote has advanced.
        assert_eq!(
            git(&proj, &["rev-parse", "HEAD"]),
            git(&proj, &["rev-parse", "origin/main"])
        );

        let plan = CodeLegPlan::gather(&proj, T);
        assert_eq!(plan.freshness, RemoteBranchFreshness::Fresh);
        assert_eq!(plan.ahead_behind, Some((0, 1)));
        let status = plan.status();
        assert!(!status.detail.contains("up to date"), "{}", status.detail);
        assert!(
            status.detail.contains("behind origin by 1 commit"),
            "{}",
            status.detail
        );
    }

    #[test]
    fn unreachable_remote_reports_unknown_not_up_to_date() {
        let tmp = tempfile::tempdir().unwrap();
        let (proj, _, _) = project(tmp.path());
        let gone = tmp.path().join("gone.git");
        git(
            &proj,
            &["remote", "set-url", "origin", gone.to_str().unwrap()],
        );

        let plan = CodeLegPlan::gather(&proj, T);
        assert!(matches!(plan.freshness, RemoteBranchFreshness::Unknown(_)));
        assert!(!plan.nothing_to_push());
        let status = plan.status();
        assert!(status.detail.contains("unknown"), "{}", status.detail);
        assert!(!status.detail.contains("up to date"), "{}", status.detail);
    }

    #[test]
    fn missing_remote_branch_is_new_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let (proj, _, _) = project(tmp.path());
        git(&proj, &["checkout", "-q", "-b", "feature"]);
        let plan = CodeLegPlan::gather(&proj, T);
        assert_eq!(plan.freshness, RemoteBranchFreshness::Absent);
        assert!(plan.status().detail.contains("new branch"));
    }

    #[test]
    fn rejected_code_push_exits_nonzero_after_attempting_store() {
        let tmp = tempfile::tempdir().unwrap();
        let (proj, code_bare, store_bare) = project(tmp.path());
        let store = proj.join(".aida-store");
        advance_remote(tmp.path(), &code_bare, "code");
        commit(&proj, "local-only.txt"); // diverged → non-fast-forward
        commit(&store, "store-local.txt");
        let store_head = git(&store, &["rev-parse", "HEAD"]);

        let err = handle_push_command(&store, false, false, None, true, true)
            .expect_err("a rejected code leg must fail the command");
        let msg = err.to_string();
        assert!(msg.contains("partial success"), "{msg}");
        assert!(msg.contains("code leg FAILED"), "{msg}");
        assert!(msg.contains("store leg pushed"), "{msg}");
        assert!(msg.contains("aida pull --code-only"), "{msg}");
        // The independent store leg was still attempted and landed.
        assert_eq!(bare_head(&store_bare, "aida-store"), store_head);
    }

    #[test]
    fn rejected_store_push_exits_nonzero() {
        let tmp = tempfile::tempdir().unwrap();
        let (proj, code_bare, store_bare) = project(tmp.path());
        let store = proj.join(".aida-store");
        advance_remote(tmp.path(), &store_bare, "store");
        commit(&store, "store-local.txt"); // diverged store
        commit(&proj, "code-local.txt");
        let code_head = git(&proj, &["rev-parse", "HEAD"]);

        let err = handle_push_command(&store, false, false, None, true, true)
            .expect_err("a rejected store leg must fail the command");
        let msg = err.to_string();
        assert!(msg.contains("partial success"), "{msg}");
        assert!(msg.contains("store leg FAILED"), "{msg}");
        assert!(msg.contains("aida pull --store-only"), "{msg}");
        assert_eq!(bare_head(&code_bare, "main"), code_head);
    }

    #[test]
    fn both_legs_succeed_exit_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let (proj, code_bare, store_bare) = project(tmp.path());
        let store = proj.join(".aida-store");
        commit(&proj, "code-local.txt");
        commit(&store, "store-local.txt");
        let code_head = git(&proj, &["rev-parse", "HEAD"]);
        let store_head = git(&store, &["rev-parse", "HEAD"]);

        handle_push_command(&store, false, false, None, true, true).expect("both legs push");
        assert_eq!(bare_head(&code_bare, "main"), code_head);
        assert_eq!(bare_head(&store_bare, "aida-store"), store_head);
    }

    #[test]
    fn behind_code_leg_skips_push_and_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let (proj, code_bare, _) = project(tmp.path());
        advance_remote(tmp.path(), &code_bare, "code");
        let remote_head = bare_head(&code_bare, "main");
        let store = proj.join(".aida-store");
        handle_push_command(&store, true, false, None, true, true)
            .expect("nothing to publish is not a failure");
        assert_eq!(bare_head(&code_bare, "main"), remote_head);
    }

    #[test]
    fn missing_store_worktree_does_not_launder_code_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let (proj, code_bare, _) = project(tmp.path());
        advance_remote(tmp.path(), &code_bare, "code");
        commit(&proj, "local-only.txt");
        let not_a_store = proj.join("no-store-here");
        let err = handle_push_command(&not_a_store, false, false, None, true, true)
            .expect_err("code failure must survive a skipped store leg");
        assert!(err.to_string().contains("code leg FAILED"), "{err}");
    }

    #[test]
    fn up_to_date_push_still_catches_up_a_lagging_mirror() {
        let tmp = tempfile::tempdir().unwrap();
        let (proj, code_bare, _) = project(tmp.path());
        // Mirror hub sits at the seed commit; origin then advances via a
        // normal push, and the mirror is left behind (the fan-out failed or
        // the push happened elsewhere).
        let mirror = tmp.path().join("mirror.git");
        std::fs::create_dir_all(&mirror).unwrap();
        git(&mirror, &["init", "-q", "--bare", "-b", "main"]);
        git(
            &proj,
            &["remote", "add", "gitlab", mirror.to_str().unwrap()],
        );
        git(&proj, &["push", "-q", "gitlab", "main"]);
        commit(&proj, "merged-on-github.txt");
        git(&proj, &["push", "-q", "origin", "main"]);
        let head = git(&proj, &["rev-parse", "HEAD"]);
        assert_ne!(bare_head(&mirror, "main"), head, "mirror should lag");
        std::fs::create_dir_all(proj.join(".aida")).unwrap();
        std::fs::write(
            proj.join(".aida/config.toml"),
            "[store.sync]\nmirror_remotes = [\"gitlab\"]\n",
        )
        .unwrap();

        let plan = CodeLegPlan::gather(&proj, T);
        assert!(plan.nothing_to_push(), "origin is level: skip path");
        handle_push_command(&proj.join(".aida-store"), true, false, None, true, true)
            .expect("up-to-date push succeeds");
        assert_eq!(bare_head(&code_bare, "main"), head);
        assert_eq!(
            bare_head(&mirror, "main"),
            head,
            "mirror caught up on the skip path"
        );
    }

    #[test]
    fn separate_push_url_is_always_pushed() {
        let tmp = tempfile::tempdir().unwrap();
        let (proj, code_bare, _) = project(tmp.path());
        // Push target is a different repository that lags the fetch URL.
        let push_target = tmp.path().join("push-target.git");
        let out = std::process::Command::new("git")
            .args([
                "clone",
                "-q",
                "--bare",
                code_bare.to_str().unwrap(),
                push_target.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(out.status.success());
        commit(&proj, "only-on-fetch-url.txt");
        git(&proj, &["push", "-q", "origin", "main"]);
        git(
            &proj,
            &[
                "remote",
                "set-url",
                "--push",
                "origin",
                push_target.to_str().unwrap(),
            ],
        );
        let head = git(&proj, &["rev-parse", "HEAD"]);

        let plan = CodeLegPlan::gather(&proj, T);
        assert_eq!(plan.ahead_behind, Some((0, 0)));
        assert!(!plan.push_matches_fetch);
        assert!(
            !plan.nothing_to_push(),
            "fetch state must not skip a separate push URL"
        );
        assert!(
            !plan.status().detail.contains("up to date"),
            "{}",
            plan.status().detail
        );
        handle_push_command(&proj.join(".aida-store"), true, false, None, true, true)
            .expect("push to the push URL");
        assert_eq!(bare_head(&push_target, "main"), head);
    }

    #[test]
    fn up_to_date_code_leg_is_not_reported_as_pushed() {
        let msg = push_failure_summary(
            &PushLegOutcome::UpToDate,
            &PushLegOutcome::Failed("rejected".into()),
            "main",
        )
        .unwrap();
        assert!(msg.contains("code leg already up to date"), "{msg}");
        assert!(!msg.contains("code leg pushed"), "{msg}");
        assert!(!msg.contains("partial success"), "{msg}");
    }

    #[test]
    fn push_anyway_gate_never_prompts_without_a_terminal() {
        let err = confirm_push_anyway(false, "feature is 1 commit behind main", "Rebase.", || {
            panic!("must not read stdin without a terminal")
        })
        .expect_err("non-interactive must fail, not exit 0 unpushed");
        let msg = err.to_string();
        assert!(msg.contains("no terminal"), "{msg}");
        assert!(msg.contains("--no-rebase-check"), "{msg}");
        assert!(msg.contains("nothing was pushed"), "{msg}");

        assert!(confirm_push_anyway(true, "r", "h", || "n\n".to_string()).is_err());
        assert!(confirm_push_anyway(true, "r", "h", String::new).is_err());
        assert!(confirm_push_anyway(true, "r", "h", || "yes\n".to_string()).is_ok());
    }

    #[test]
    fn behind_main_check_without_terminal_exits_nonzero_unpushed() {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            // The gate would (correctly) prompt; covered by the unit test above.
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (proj, code_bare, _) = project(tmp.path());
        git(&proj, &["checkout", "-q", "-b", "feature"]);
        commit(&proj, "feature.txt");
        advance_remote(tmp.path(), &code_bare, "code");
        git(&proj, &["fetch", "-q", "origin"]);

        let err = handle_push_command(&proj.join(".aida-store"), true, false, None, false, true)
            .expect_err("a behind-main branch with no terminal must not exit 0");
        assert!(err.to_string().contains("behind main"), "{err}");
        let has_feature = std::process::Command::new("git")
            .arg("-C")
            .arg(&code_bare)
            .args(["rev-parse", "--verify", "--quiet", "refs/heads/feature"])
            .status()
            .unwrap()
            .success();
        assert!(!has_feature, "nothing may be pushed");
    }

    #[test]
    fn summary_is_none_when_nothing_failed() {
        assert_eq!(
            push_failure_summary(&PushLegOutcome::Pushed, &PushLegOutcome::Pushed, "main"),
            None
        );
        assert_eq!(
            push_failure_summary(
                &PushLegOutcome::Skipped,
                &PushLegOutcome::NotRequested,
                "main"
            ),
            None
        );
    }

    #[test]
    fn summary_names_single_failed_leg_without_partial_success() {
        let msg = push_failure_summary(
            &PushLegOutcome::NotRequested,
            &PushLegOutcome::Failed("rejected".into()),
            "main",
        )
        .unwrap();
        assert!(msg.starts_with("aida push failed"), "{msg}");
        assert!(!msg.contains("code leg"), "{msg}");
        assert!(msg.contains("aida push --store-only"), "{msg}");
    }
}

#[cfg(test)]
mod bug_1500_store_pull_hint_tests {
    use super::store_pull_failure_hint;

    // trace:BUG-1500 | ai:claude
    #[test]
    fn no_rebase_in_progress_does_not_suggest_abort() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // A plain non-repo directory: `rebase_in_progress` returns false
        // for it (no `.git` at all), the same as a repo that is simply
        // not mid-rebase — e.g. a transient network 502.
        let hint = store_pull_failure_hint(tmp.path(), "connection reset (502)");
        assert!(
            !hint.contains("rebase --abort"),
            "hint should not advise `git rebase --abort` when no rebase is in progress: {hint}"
        );
        assert!(hint.contains("The store is not mid-rebase"));
        assert!(hint.contains("connection reset (502)"));
    }

    // trace:TASK-1604 | ai:codex
    #[test]
    fn deterministic_git_errors_are_not_labelled_transient() {
        let tmp = tempfile::tempdir().unwrap();
        for error in [
            "fatal: Cannot rebase onto multiple branches",
            "fatal: cannot lock ref HEAD: is at abc but expected def",
            "store fetch failed: invalid refspec",
        ] {
            let hint = store_pull_failure_hint(tmp.path(), error);
            assert!(hint.contains(error), "{hint}");
            assert!(!hint.contains("transient"), "{hint}");
            assert!(!hint.contains("rebase --abort"), "{hint}");
            assert!(
                hint.contains("ref/configuration or concurrent-writer"),
                "{hint}"
            );
        }
    }

    // trace:BUG-1500 | ai:claude
    #[test]
    fn rebase_in_progress_still_suggests_abort() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo)
            .status()
            .expect("git init")
            .success());
        // Simulate a rebase actually in progress: create the marker
        // directory `rebase_in_progress` checks for.
        std::fs::create_dir(repo.join(".git").join("rebase-merge")).expect("mkdir rebase-merge");

        let hint = store_pull_failure_hint(repo, "some pull error");
        assert!(
            hint.contains("rebase --abort"),
            "hint should advise `git rebase --abort` when a rebase IS in progress: {hint}"
        );
    }

    // trace:BUG-1500 | ai:claude
    // Covers the second emission site: `aida db sync --pull`'s
    // `handle_git_backend_command` (aida-cli-lib/src/git_backend_cmd.rs)
    // wraps the same `store_pull_failure_hint` output as
    // `anyhow::bail!("Pull failed: {}", ...)`. This mirrors that exact
    // format string so a regression there (e.g. reverting to the old
    // unconditional "may be mid-rebase" text) is caught here too.
    #[test]
    fn db_sync_pull_bail_message_omits_abort_hint_without_rebase() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let hint = store_pull_failure_hint(tmp.path(), "connection reset (502)");
        let bail_message = format!("Pull failed: {}", hint);
        assert!(
            !bail_message.contains("rebase --abort"),
            "db sync --pull's bail message should not advise `git rebase --abort` \
             when no rebase is in progress: {bail_message}"
        );
    }

    // trace:BUG-1500 | ai:claude
    #[test]
    fn db_sync_pull_bail_message_includes_abort_hint_with_rebase() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo)
            .status()
            .expect("git init")
            .success());
        std::fs::create_dir(repo.join(".git").join("rebase-merge")).expect("mkdir rebase-merge");

        let hint = store_pull_failure_hint(repo, "some pull error");
        let bail_message = format!("Pull failed: {}", hint);
        assert!(
            bail_message.contains("rebase --abort"),
            "db sync --pull's bail message should advise `git rebase --abort` \
             when a rebase IS in progress: {bail_message}"
        );
    }
}

/// `aida pull`'s store leg between the mailbox publish and the code-derived
/// reconcile. The caller holds the store write lock. `Err` carries the
/// store-leg failure text for the exit summary (already warned).
// trace:TASK-1717 | ai:claude
fn pull_store_leg_locked(
    store_path: &std::path::Path,
    quiet: bool,
    no_gate: bool,
    mailbox_published: usize,
) -> std::result::Result<(), String> {
    use aida_core::git_ops;
    // Mirror `aida db sync --pull`: commit any pending orphan
    // changes first, then pull --rebase. Without the pre-commit
    // step, rebase refuses on a dirty tree and leaves the user
    // half-pulled.
    // trace:TASK-1717 | ai:claude — a failed stage/commit (a refusing hook)
    // ends the leg with git's own error before any pull; the pending changes
    // are kept, uncommitted.
    let pending = git_ops::has_changes_checked(store_path).map_err(|e| {
        let failure = format!("store leg could not read pending changes: {e:#}");
        eprintln!("  {} {failure}", "Warning:".yellow().bold());
        failure
    })?;
    if pending {
        if let Err(e) = git_ops::add(store_path, &["."])
            .and_then(|_| git_ops::commit(store_path, "chore: sync pending changes"))
        {
            let failure = format!("store leg could not commit pending changes: {e:#}");
            eprintln!("  {} {failure}", "Warning:".yellow().bold());
            return Err(failure);
        }
        println!("  Committed pending orphan changes before pull");
    }
    report_mailbox_published(mailbox_published, "pull");
    let branch = git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());
    println!("{}", pull_store_start_line());

    // TASK-73: snapshot the orphan-store HEAD SHA before pull so we
    // can summarize what landed once it completes. None when the
    // store is empty / not yet committed. trace:TASK-73 | ai:claude
    let pre_sha = git_ops::head_sha(store_path).ok();

    // STORY-641 / MU-204: auto-reconcile conflicting spec objects instead
    // of stopping for manual resolution. Two clones editing the SAME spec
    // (each appending a HistoryEntry + a scalar edit) are structurally
    // mergeable — history is unioned by id, scalars resolve LWW. The
    // oplog and per-user queue registries (same user on two machines —
    // BUG-725) union the same way; other conflicts (blocks, nodes,
    // counters) still fall back to the manual path.
    // trace:STORY-641 | ai:claude
    let outcome = match git_ops::pull_rebase_auto_merge(store_path, "origin", &branch) {
        Ok(outcome) => outcome,
        Err(e) => {
            eprintln!(
                "  {} {}",
                "Warning:".yellow().bold(),
                store_pull_failure_hint(store_path, &e.to_string())
            );
            return Err(format!("store leg pull_rebase failed: {}", e));
        }
    };
    if let git_ops::StorePullOutcome::AutoMerged { notes } = &outcome {
        for note in notes {
            println!("  {} {}", "auto-merged".cyan().bold(), note);
        }
    }
    println!("  {}", "store pull complete".green());
    if let Err(e) = ensure_no_spec_id_collisions_canonical(store_path) {
        eprintln!("  {} {}", "Warning:".yellow().bold(), e);
        return Err(format!("store collision scan failed: {}", e));
    }
    // TASK-73 — summarize the delta unless --quiet.
    if !quiet {
        if let Some(pre) = pre_sha.as_deref() {
            print_pull_summary(store_path, pre);
        }
    }
    // TASK-78: post-pull merge-gate. The pull may have brought
    // in commits from collaborators with un-gated node-aware
    // ids; promoting them now means subsequent `aida list` /
    // queue surfaces show short ids without an extra ritual.
    // Skipped by `--no-gate` or `AIDA_AUTO_MERGE_GATE=false`.
    // Idempotent (no-op when there's nothing pending) and
    // cheap. trace:TASK-78 | ai:claude
    if !no_gate && auto_merge_gate_enabled() {
        match git_ops::merge_gate(store_path) {
            Ok(assignments) if assignments.is_empty() => {
                // Stay silent when nothing was promoted — pull
                // already printed its summary; an empty footer
                // would just be noise.
            }
            Ok(assignments) => {
                println!(
                    "  {} ({} promotion{})",
                    "auto-gate ran".cyan(),
                    assignments.len(),
                    if assignments.len() == 1 { "" } else { "s" }
                );
                for (node_id, agreed_id) in &assignments {
                    println!("    {} → {}", node_id, agreed_id.green().bold());
                }
            }
            Err(e) => {
                eprintln!(
                    "  {} merge-gate failed: {} \
                     (run `aida db merge-gate` manually to retry)",
                    "Warning:".yellow().bold(),
                    e
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn handle_pull_command(
    store_path: &std::path::Path,
    code_only: bool,
    store_only: bool,
    quiet: bool,
    no_gate: bool,
    auto: bool,
) -> Result<()> {
    use aida_core::git_ops;

    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // TASK-106: pre-pull summary (interactive only, no prompt).
    print_pull_plan(&project_root, store_path, code_only, store_only);

    // BUG-254: track per-leg failures so the function's exit reflects what
    // actually happened. Before this, a non-zero `git pull --ff-only` was
    // logged as a Warning and swallowed — `aida pull` returned 0 and the
    // orchestrator's phase 5 then announced `phase 5 complete` over a
    // stale tree, breaking the auto-bump scan and confusing phase 6.
    // trace:BUG-254 | ai:claude
    let mut code_failed: Option<String> = None;
    let mut store_failed: Option<String> = None;
    // BUG-1625: set by a successful code leg to the auto-bump scan base; the
    // store-touching reconcile runs only after the store leg has pulled (or
    // was legitimately skipped). trace:BUG-1625 | ai:claude
    let mut deferred_reconcile: Option<Option<String>> = None;

    // ---- Code pull (current branch on the project repo) ----
    if !store_only {
        if !git_ops::has_remote(&project_root, "origin") {
            println!(
                "  {}",
                pull_skip_warning("no `origin` remote — skipping code pull")
            );
        } else {
            let branch =
                git_ops::current_branch(&project_root).unwrap_or_else(|_| "HEAD".to_string());
            println!("{}", pull_code_start_line(&branch));
            // STORY-86: snapshot HEAD before pull so the auto-bump scan
            // can range over exactly what landed. None on first ever
            // commit / empty repo — the helper falls back to HEAD~50.
            // trace:STORY-86 | ai:claude
            let pre_code_sha = git_ops::head_sha(&project_root).ok();
            // BUG-691: snapshot the stash-stack top BEFORE the pull. If git's
            // built-in autostash (the recommended global `rebase.autoStash` /
            // `merge.autoStash`) leaves a stash un-restored after a failed
            // ff-update, we detect + restore it below by the top having moved.
            // trace:BUG-691 | ai:claude
            let pre_stash_top = git_ops::stash_top_sha(&project_root);
            let config_snapshots = snapshot_known_project_configs(&project_root);
            // BUG-691: refuse the pull UP FRONT if another git process holds the
            // index lock. Otherwise git's autostash can stash the dirty tree,
            // then die on the reset when the lock reappears — stranding the
            // user's work. A short wait absorbs a transient lock; a genuinely
            // stuck one surfaces in ~1s and we fail before anything is stashed.
            // trace:BUG-691 | ai:claude
            let index_clear = git_ops::wait_for_index_lock_clear(&project_root, 10, 100);
            if !index_clear {
                eprintln!(
                    "  {} another git process holds this repo's index lock \
                     (.git/index.lock) — refusing the pull before anything is \
                     stashed, so your uncommitted work is not stranded.\n  \
                     {} wait for the other git command to finish (or remove a \
                     stale .git/index.lock if none is running), then re-run \
                     `aida pull`.",
                    "Warning:".yellow().bold(),
                    "Note:".dimmed(),
                );
                code_failed = Some(format!(
                    "git index locked (.git/index.lock held) on branch {}",
                    branch
                ));
            }
            // --ff-only refuses if the local branch has diverged from
            // origin (user has unpushed commits AND origin has new
            // commits). Safer default than --rebase for a working
            // branch — the user gets a clear error and can decide
            // whether to rebase, merge, or stash. Matches the task's
            // acceptance shape ("equivalent to `git pull --ff-only`").
            // trace:TASK-43 | ai:claude
            //
            // Skipped entirely when the index lock is held (handled above) so
            // nothing is stashed and stranded. trace:BUG-691 | ai:claude
            if index_clear {
                // BUG-1780: don't use `git pull --ff-only` because it uses
                // FETCH_HEAD (which races with concurrent fetches) and respects
                // pull.rebase config (which can fail with "Cannot rebase onto
                // multiple branches"). Do fetch + merge instead.
                // trace:BUG-1780 | ai:codex
                let _ = aida_core::git_ops::host_git_command(&project_root)
                    .args(["fetch", "origin", &branch])
                    .status();
                let res = aida_core::git_ops::host_git_command(&project_root)
                    .args(["merge", "--ff-only", &format!("origin/{}", branch)])
                    .status();
                match res {
                    Ok(s) if s.success() => {
                        if let Err(e) =
                            validate_and_restore_project_configs_after_pull(&config_snapshots)
                        {
                            eprintln!("  {} {}", "Warning:".yellow().bold(), e);
                            anyhow::bail!("aida pull: code leg failed (post-pull AIDA config validation failed: {e})");
                        }
                        println!("  {}", "code pull complete".green());
                        // STORY-86: scan the just-pulled commits for
                        // refs to specs currently in Done and bump them.
                        // Best-effort: any failure prints a warning but
                        // doesn't fail the pull. trace:STORY-86 | ai:claude
                        //
                        // BUG-404: the narrow `pre_code_sha..HEAD` range assumes
                        // THIS pull is what advanced main. But `aida pr ship`
                        // step 3 (`gh pr merge --squash`) fast-forwards local
                        // main to the squash commit BEFORE step 4's pull runs —
                        // so the pull is a no-op ("Already up to date"),
                        // pre_code_sha == post_head, and the narrow range is
                        // empty. The merged spec then never bumps (confirmed by
                        // AIDA_DEBUG_AUTOBUMP instrumentation: 0 commits in the
                        // range, 0 flips). When the pull moved nothing, fall back
                        // to the wide scan so a main advanced outside this pull
                        // (gh merge, or a manual merge-then-pull) is still
                        // covered. The Done-guard inside the helper keeps the
                        // wide scan idempotent. trace:BUG-404 | ai:claude
                        let post_code_sha = git_ops::head_sha(&project_root).ok();
                        let pull_was_noop =
                            match (pre_code_sha.as_deref(), post_code_sha.as_deref()) {
                                (Some(a), Some(b)) => a == b,
                                _ => false,
                            };
                        // STORY-127 detectors (1)+(2): a no-op code pull. If the
                        // user holds a reviewer lease on an unmerged PR, this is
                        // the PR-27-style "ran catch-up before the merge happened"
                        // mistake — warn so they wait for the merge. Otherwise a
                        // quiet one-liner. Suppressed in --quiet (orchestrator /
                        // scripted) runs. trace:STORY-127 | ai:claude
                        if pull_was_noop && !quiet {
                            let reviewer_pr = active_reviewer_unmerged_pr(&project_root);
                            if let Some(msg) = pull_noop_warning(true, reviewer_pr.as_deref()) {
                                let tag = if reviewer_pr.is_some() {
                                    "Warning:".yellow().bold()
                                } else {
                                    "Note:".dimmed()
                                };
                                eprintln!("  {} {}", tag, msg);
                            }
                        }
                        let scan_pre: Option<&str> = if pull_was_noop {
                            None
                        } else {
                            pre_code_sha.as_deref()
                        };
                        // Opt-in instrumentation (default off) — permanent
                        // visibility into why an auto-bump did or didn't fire.
                        let dbg_autobump = std::env::var("AIDA_DEBUG_AUTOBUMP")
                            .map(|v| v != "0" && !v.is_empty())
                            .unwrap_or(false);
                        if dbg_autobump {
                            eprintln!(
                            "  [autobump-debug] enabled={} pre_code_sha={:?} post_head={:?} pull_was_noop={} scan_range={} store_path={}",
                            auto_bump_enabled(),
                            pre_code_sha,
                            post_code_sha,
                            pull_was_noop,
                            match scan_pre {
                                Some(p) => format!("{}..HEAD", p),
                                None => "(wide → HEAD~50)".to_string(),
                            },
                            store_path.display()
                        );
                        }
                        // BUG-1625: DEFER every store-reading/-writing step
                        // (auto-bump reconcile, closure steps, followup
                        // extraction, archive sweep) until the store leg has
                        // pulled — running them here acted on a stale local
                        // store and regressed remotely-Completed specs.
                        // trace:BUG-1625 | ai:claude
                        // BUG-1633: a prior pull whose store leg failed saved
                        // its scan start — resume from it so the commits that
                        // pull brought in are still scanned, however many.
                        // trace:BUG-1633 | ai:claude
                        deferred_reconcile = Some(
                            load_pending_reconcile_base(&project_root)
                                .or_else(|| scan_pre.map(str::to_string)),
                        );
                        // STORY-248: stacked-branch cascade. Walks
                        // `.aida/stacks.json`; for each entry whose parent
                        // branch is no longer reachable locally + on origin
                        // (= it was merged + auto-deleted), rebase it onto
                        // origin/main using the recorded fork-point SHA.
                        // Best-effort: a failure prints a warning but does
                        // NOT fail the pull (BUG-254 contract — pull's exit
                        // reflects code + store legs only).
                        // trace:STORY-248 | ai:claude
                        if let Err(e) = cascade_rebase_stacked_branches(&project_root, auto) {
                            eprintln!(
                                "  {} stacked-branch cascade failed: {} (some stacked branches \
                             may be unrebased; run `/aida-rebase` in each, or \
                             `aida stack show` to inspect)",
                                "Warning:".yellow().bold(),
                                e
                            );
                        }

                        // BUG-665: HEAD just advanced. If a dev-activated in-repo
                        // build is on PATH and is now behind HEAD, the user's `aida`
                        // (tui / integrate / anything) is silently running old code
                        // until they rebuild. Nudge once. Suppressed in --quiet
                        // (orchestrator / scripted) runs. trace:BUG-665 | ai:claude
                        if !quiet {
                            warn_if_pulled_binary_stale(&project_root);
                        }
                    }
                    Ok(s) => {
                        // BUG-254: surface the recovery hint and an explicit
                        // auto-bump-skipped note. The hint covers the two
                        // common causes — an untracked file that would be
                        // overwritten, and a diverged branch — because
                        // `git pull --ff-only` itself prints which case
                        // applies right above this line. trace:BUG-254
                        // BUG-1780: also include concurrent-fetch as the first
                        // suggestion because we might fail if FETCH_HEAD was modified.
                        eprintln!(
                            "  {} git merge --ff-only exited with status {} — \
                         a concurrent fetch may have interrupted it, your \
                         branch may have diverged from origin/{}, or \
                         the merge would overwrite an untracked file.\n  \
                         To recover:\n    \
                         - Concurrent fetch or untracked-file: re-run `aida pull` \
                         (remove/rename any listed untracked files first).\n    \
                         - Diverged branch: `git pull --rebase origin {}` \
                         (resolve conflicts), or `git pull --no-rebase`.\n  \
                         {} auto-bump skipped — code leg did not advance, \
                         so any Done→Completed bumps for the missed \
                         commits will fire on the next successful pull.",
                            "Warning:".yellow().bold(),
                            s,
                            branch,
                            branch,
                            "Note:".dimmed(),
                        );
                        code_failed = Some(format!(
                            "git merge --ff-only exited {} on branch {}",
                            s, branch
                        ));
                        // BUG-691: if git's autostash stashed the working tree and
                        // then the ff-update failed, restore it so the user's
                        // uncommitted work is not silently stranded in the stash.
                        // trace:BUG-691 | ai:claude
                        report_autostash_restore(&project_root, pre_stash_top.as_deref());
                        if let Err(e) =
                            validate_and_restore_project_configs_after_pull(&config_snapshots)
                        {
                            eprintln!("  {} {}", "Warning:".yellow().bold(), e);
                            code_failed =
                                Some(format!("post-pull AIDA config validation failed: {e}"));
                        }
                    }
                    Err(e) => {
                        // BUG-691: same recovery on the process-spawn failure path
                        // before bailing — never leave a stranded autostash behind.
                        // trace:BUG-691 | ai:claude
                        report_autostash_restore(&project_root, pre_stash_top.as_deref());
                        anyhow::bail!("git pull failed: {}", e);
                    }
                }
            }
        }
    }

    // ---- Store pull (orphan branch via pull_rebase) ----
    // `--code-only`: the operator opted out of the store leg, so the deferred
    // reconcile runs against the local store as before. trace:BUG-1625
    // BUG-1633: when that store has a remote it may be stale, so plan-followup
    // filing is deferred to the next full pull. trace:BUG-1633 | ai:claude
    if code_only {
        if let Some(scan_pre) = deferred_reconcile.take() {
            let store_may_be_stale =
                git_ops::is_git_repo(store_path) && git_ops::has_remote(store_path, "origin");
            run_deferred_code_reconcile(
                &project_root,
                store_path,
                scan_pre.as_deref(),
                quiet,
                !store_may_be_stale,
            );
        }
    }
    if !code_only {
        if !git_ops::is_git_repo(store_path) {
            println!(
                "  {}",
                pull_skip_warning("no orphan worktree — skipping store pull")
            );
            // BUG-1625: no store leg to wait for (legacy / not-yet-attached
            // store) — the local store IS the canonical one. trace:BUG-1625
            if let Some(scan_pre) = deferred_reconcile.take() {
                run_deferred_code_reconcile(
                    &project_root,
                    store_path,
                    scan_pre.as_deref(),
                    quiet,
                    true,
                );
            }
            // BUG-476: skipping the store pull must NOT launder a failed code
            // leg into a success. A code-only clone (or a not-yet-attached
            // store, common in CI) hits this early return with `code_failed`
            // already set; returning Ok(()) here would exit 0 over a stale,
            // non-advanced tree — exactly the BUG-254 contract this is meant
            // to honor. Bail with the same code-leg error the bottom check
            // produces. trace:BUG-476
            if let Some(c) = code_failed.as_deref() {
                anyhow::bail!("aida pull: code leg failed ({c})");
            }
            return Ok(());
        }
        if !git_ops::has_remote(store_path, "origin") {
            println!(
                "  {}",
                pull_skip_warning("orphan store has no `origin` — skipping store pull")
            );
            // BUG-1625: nothing remote to pull first. trace:BUG-1625
            if let Some(scan_pre) = deferred_reconcile.take() {
                run_deferred_code_reconcile(
                    &project_root,
                    store_path,
                    scan_pre.as_deref(),
                    quiet,
                    true,
                );
            }
            // BUG-476: same as above — a no-origin store does not redeem a
            // failed code leg. trace:BUG-476
            if let Some(c) = code_failed.as_deref() {
                anyhow::bail!("aida pull: code leg failed ({c})");
            }
            return Ok(());
        }
        // STORY-643: PUBLISH the local mailbox into the canonical store BEFORE
        // the pre-pull commit, so this clone's locally-sent messages are
        // committed onto the orphan branch (and propagate on the next push)
        // while the rebase below brings DOWN other clones' canonical messages.
        // Together with `read_inbox`/`mailbox inbox` merging canonical, this
        // makes messages flow both ways on a normal `aida pull` — no manual
        // digest. Best-effort + idempotent; folded into the pre-pull commit
        // (no separate commit). trace:STORY-643 | ai:claude
        // trace:TASK-1717 | ai:claude — local messages are read first; the
        // canonical digest, the pending-changes commit, the rebase pull with
        // its own cleanup, the collision scan, the summary and the nested
        // merge-gate are ONE store-writer transaction under the store write
        // lock. The code-derived reconcile and maintenance run after release.
        let mailbox = mailbox_publish_snapshot(store_path, "pull");
        let window = aida_core::db::with_store_write_lock(store_path, || {
            // A store whose status cannot be read ends the leg before the
            // digest or anything else touches it.
            if let Err(e) = git_ops::has_changes_checked(store_path) {
                let failure = format!("store leg could not read pending changes: {e:#}");
                eprintln!("  {} {failure}", "Warning:".yellow().bold());
                return Ok(Err(failure));
            }
            let published = mailbox
                .as_ref()
                .map(|local| publish_mailbox_snapshot(store_path, local, "pull"))
                .unwrap_or(0);
            Ok(pull_store_leg_locked(store_path, quiet, no_gate, published))
        });
        let pulled = matches!(window, Ok(Ok(())));
        match window {
            Ok(Ok(())) => {
                // BUG-1625: the store is now fresh — run the code-derived
                // reconcile (auto-bump, closure steps, followup extraction,
                // archive sweep) against it, never against the pre-pull
                // snapshot. trace:BUG-1625 | ai:claude
                if let Some(scan_pre) = deferred_reconcile.take() {
                    run_deferred_code_reconcile(
                        &project_root,
                        store_path,
                        scan_pre.as_deref(),
                        quiet,
                        true,
                    );
                }
            }
            Ok(Err(failure)) => store_failed = Some(failure),
            Err(e) => {
                eprintln!("  {} {e:#}", "Warning:".yellow().bold());
                store_failed = Some(format!("store leg could not start: {e:#}"));
            }
        }
        // TASK-1033: opportunistic store maintenance after a clean
        // store-leg pull — ensure the lowered gc.auto is set, then
        // `git gc --auto` (no-op unless the threshold is exceeded).
        // Best-effort; never affects pull's exit code. It runs after the
        // store write lock is released: `gc --auto` may detach.
        if pulled {
            aida_core::git_ops::opportunistic_store_gc(store_path);
        }
    }
    // BUG-1625: the store leg failed (or its post-pull scan did), so the local
    // store may be stale — leave code-derived reconciliation unapplied rather
    // than write status transitions against it. trace:BUG-1625 | ai:claude
    // BUG-1633: persist the scan start so the retry covers every commit this
    // pull brought in, not just the last 50. trace:BUG-1633 | ai:claude
    if let Some(scan_pre) = deferred_reconcile.take() {
        let resume = match scan_pre.as_deref() {
            Some(base) => {
                write_reconcile_state(&project_root, PENDING_RECONCILE_BASE_STATE, base);
                "the scan start is saved, so the retry covers every commit this pull brought in"
                    .to_string()
            }
            None => {
                "if more than 50 commits landed, also run `aida db reconcile-status`".to_string()
            }
        };
        eprintln!(
            "  {} Done→Completed auto-bump and plan-followup filing skipped — the \
             store did not sync, so they would act on stale data. Fix the store \
             pull above, then re-run `aida pull` to apply them ({resume}).",
            "Note:".dimmed(),
        );
    }

    // STORY-262: no-daemon scheduled advisor tasks. After the legs settle,
    // evaluate registered schedules (`.aida/schedules.toml`) and file a TASK
    // into the target role's queue for each that's due. Best-effort: a
    // failure prints a warning but does NOT change `aida pull`'s exit code
    // (that stays bound to the code+store legs per the BUG-254 contract).
    // trace:STORY-262 | ai:claude
    match schedule_cmd::fire_schedules(&project_root, store_path, None, quiet) {
        Ok(fired) if !fired.is_empty() && !quiet => {
            println!(
                "  {} {} scheduled task{} filed",
                "Schedules:".cyan().bold(),
                fired.len(),
                if fired.len() == 1 { "" } else { "s" },
            );
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!(
                "  {} schedule evaluation failed: {} (no scheduled tasks filed this pull)",
                "Warning:".yellow().bold(),
                e,
            );
        }
    }

    // BUG-1676: the mirror hubs follow ORIGIN, not this machine's pushes. The
    // default branch advances by forge-side merges that only a pull ever
    // sees, and the store is pushed to origin by targeted writes that never
    // fan out, so this is the one place both hubs are brought level on every
    // regular cadence (drain phase 5, `aida pr ship`, an operator catch-up).
    // Best-effort and silent on success: a hub failure is printed and never
    // changes the pull's exit code (the BUG-254 contract below stays bound to
    // the two legs).
    // trace:BUG-1676 | ai:claude
    if code_failed.is_none() && store_failed.is_none() {
        remote_create::mirror_sync_after_pull(&project_root);
    }

    // BUG-254: any leg failure → non-zero exit, so the orchestrator's
    // phase 5 reports failure instead of `phase 5 complete` over a
    // tree that did not advance. The detail names which legs failed so
    // the headless drain's JSONL log captures the cause.
    // trace:BUG-254 | ai:claude
    match (code_failed.as_deref(), store_failed.as_deref()) {
        (None, None) => Ok(()),
        (Some(c), None) => anyhow::bail!("aida pull: code leg failed ({c})"),
        (None, Some(s)) => anyhow::bail!("aida pull: store leg failed ({s})"),
        (Some(c), Some(s)) => {
            anyhow::bail!("aida pull: code leg failed ({c}); store leg failed ({s})")
        }
    }
}

/// BUG-1633: per-worktree git-path file holding the auto-bump scan start of a
/// pull whose store leg failed, so the retry scans exactly what that pull
/// brought in instead of falling back to the last 50 commits.
///
/// Both BUG-1633 state files live in the worktree's own git dir
/// (`.git/` for the main checkout, `.git/worktrees/<name>/` for a linked
/// worktree). That is enough because the auto-bump only acts on the default
/// branch, which one worktree at a time can have checked out — but
/// `git worktree remove` discards the state with the worktree. Recover a lost
/// scan start with `aida db reconcile-status --since <sha>`.
// trace:BUG-1633 | ai:claude
pub(crate) const PENDING_RECONCILE_BASE_STATE: &str = "aida-pending-reconcile-base";

/// BUG-1633: per-worktree git-path file listing specs a `--code-only` pull
/// flipped without filing their plan followups (one spec id per line). See
/// [`PENDING_RECONCILE_BASE_STATE`] for the per-worktree caveat.
// trace:BUG-1633 | ai:claude
pub(crate) const PENDING_FOLLOWUPS_STATE: &str = "aida-pending-followups";

/// BUG-1633: `git rev-parse --git-path <name>`, absolutized. Lives in the git
/// dir, so it never dirties the working tree.
// trace:BUG-1633 | ai:claude
pub(crate) fn reconcile_state_path(
    project_root: &std::path::Path,
    name: &str,
) -> Option<std::path::PathBuf> {
    let out = aida_core::git_ops::host_git_command(project_root)
        .args(["rev-parse", "--git-path", name])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    let p = std::path::PathBuf::from(raw);
    Some(if p.is_absolute() {
        p
    } else {
        project_root.join(p)
    })
}

// trace:BUG-1633 | ai:claude
pub(crate) fn read_reconcile_state_lines(
    project_root: &std::path::Path,
    name: &str,
) -> Vec<String> {
    reconcile_state_path(project_root, name)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|body| {
            body.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// BUG-1633: write via a temp file + rename, so a reader never sees a
/// half-written file and a crash never leaves a truncated one.
// trace:BUG-1633 | ai:claude
pub(crate) fn write_reconcile_state(project_root: &std::path::Path, name: &str, body: &str) {
    if let Some(p) = reconcile_state_path(project_root, name) {
        let tmp = p.with_file_name(format!("{name}.tmp.{}", std::process::id()));
        let res = std::fs::write(&tmp, format!("{body}\n")).and_then(|_| std::fs::rename(&tmp, &p));
        if let Err(e) = res {
            let _ = std::fs::remove_file(&tmp);
            eprintln!(
                "  {} could not save {}: {e}",
                "Warning:".yellow().bold(),
                p.display()
            );
        }
    }
}

/// BUG-1633: read-modify-write of the pending-followups list against a
/// FRESH read, so a concurrent `--code-only` append made while this process
/// was filing followups is kept. An empty result removes the file.
// trace:BUG-1633 | ai:claude
pub(crate) fn update_pending_followups(
    project_root: &std::path::Path,
    f: impl FnOnce(&mut Vec<String>),
) {
    let mut ids = read_reconcile_state_lines(project_root, PENDING_FOLLOWUPS_STATE);
    f(&mut ids);
    if ids.is_empty() {
        clear_reconcile_state(project_root, PENDING_FOLLOWUPS_STATE);
    } else {
        write_reconcile_state(project_root, PENDING_FOLLOWUPS_STATE, &ids.join("\n"));
    }
}

/// BUG-1633: file the plan followups a prior `--code-only` pull deferred.
/// Removes only the ids whose extraction succeeded; an id whose extraction
/// errored (e.g. a transient store read failure) stays pending for the next
/// pull — safe, because extraction is idempotent via the followups marker.
/// Returns the ids still pending.
// trace:BUG-1633 | ai:claude
pub(crate) fn drain_pending_followups(
    project_root: &std::path::Path,
    storage: &Storage,
) -> Vec<String> {
    let pending = read_reconcile_state_lines(project_root, PENDING_FOLLOWUPS_STATE);
    if pending.is_empty() {
        return Vec::new();
    }
    let mut filed: Vec<String> = Vec::new();
    for spec_id in &pending {
        match extract_plan_followups(storage, project_root, spec_id, spec_id, false) {
            Ok(()) => filed.push(spec_id.clone()),
            Err(e) => eprintln!(
                "  {} deferred plan-followup filing for {spec_id} failed: {e} \
                 (kept pending; the next `aida pull` retries it)",
                "Warning:".yellow().bold(),
            ),
        }
    }
    let mut remaining = Vec::new();
    update_pending_followups(project_root, |ids| {
        ids.retain(|id| !filed.contains(id));
        remaining = ids.clone();
    });
    remaining
}

// trace:BUG-1633 | ai:claude
pub(crate) fn clear_reconcile_state(project_root: &std::path::Path, name: &str) {
    if let Some(p) = reconcile_state_path(project_root, name) {
        let _ = std::fs::remove_file(p);
    }
}

/// BUG-1633: the scan start a previous pull persisted when its store leg
/// failed — only while it is still an ancestor of HEAD (a rewritten history
/// makes it meaningless, and the normal range takes over).
// trace:BUG-1633 | ai:claude
pub(crate) fn load_pending_reconcile_base(project_root: &std::path::Path) -> Option<String> {
    let base = read_reconcile_state_lines(project_root, PENDING_RECONCILE_BASE_STATE)
        .into_iter()
        .next()?;
    if base.starts_with('-') {
        return None;
    }
    let is_ancestor = aida_core::git_ops::host_git_command(project_root)
        .args(["merge-base", "--is-ancestor", &base, "HEAD"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    is_ancestor.then_some(base)
}

/// BUG-1625: the store-touching half of `aida pull`'s code leg — the
/// merge-driven auto-bump (reconcile, closure steps, plan-followup extraction)
/// and the opt-in archive sweep. `handle_pull_command` calls this only AFTER
/// the store leg has pulled (or was legitimately skipped: `--code-only`, no
/// orphan worktree, no store `origin`), so every automated status transition
/// reads and writes the freshly pulled canonical store instead of a stale local
/// snapshot. Best-effort: failures warn and never change pull's exit code.
///
/// BUG-1633: `extract_followups = false` (a `--code-only` pull against a store
/// with a remote) flips specs but records them in [`PENDING_FOLLOWUPS_STATE`]
/// instead of filing their plan followups; the next reconcile that runs
/// against a synced store files them. A run whose auto-bump actually scanned
/// also clears the persisted scan start ([`PENDING_RECONCILE_BASE_STATE`]).
// trace:BUG-1625 trace:BUG-1633 | ai:claude
pub(crate) fn run_deferred_code_reconcile(
    project_root: &std::path::Path,
    store_path: &std::path::Path,
    scan_pre: Option<&str>,
    quiet: bool,
    extract_followups: bool,
) {
    let dbg_autobump = std::env::var("AIDA_DEBUG_AUTOBUMP")
        .map(|v| v != "0" && !v.is_empty())
        .unwrap_or(false);
    let storage = Storage::new(store_path);
    // BUG-1633: file the followups a prior `--code-only` pull deferred, now
    // that the store is synced. Idempotent via the followups marker.
    // trace:BUG-1633 | ai:claude
    if extract_followups {
        drain_pending_followups(project_root, &storage);
    }
    // BUG-1633: set only when the auto-bump actually walked a commit range.
    let mut scanned = false;
    // trace:STORY-86 | ai:claude
    if auto_bump_enabled() {
        match auto_bump_done_to_completed_with(
            project_root,
            store_path,
            scan_pre,
            &storage,
            extract_followups,
        ) {
            Ok((flips, did_scan)) => {
                scanned = did_scan;
                // BUG-1633: defer followup filing for these flips to the next
                // reconcile against a synced store. trace:BUG-1633 | ai:claude
                if !extract_followups && !flips.is_empty() {
                    let mut ids = Vec::new();
                    update_pending_followups(project_root, |pending| {
                        for f in &flips {
                            if !pending.contains(&f.spec_id) {
                                pending.push(f.spec_id.clone());
                            }
                        }
                        ids = pending.clone();
                    });
                    if !quiet {
                        eprintln!(
                            "  {} plan-followup filing deferred for {} — `--code-only` did not \
                             sync the store, so it could be stale. The next full `aida pull` \
                             files them.",
                            "Note:".dimmed(),
                            ids.join(", ")
                        );
                    }
                }
                if dbg_autobump {
                    eprintln!(
                        "  [autobump-debug] auto_bump_done_to_completed → {} flip(s): {:?}",
                        flips.len(),
                        flips.iter().map(|f| f.spec_id.clone()).collect::<Vec<_>>()
                    );
                }
                if !quiet {
                    print_auto_bump_summary(&flips);
                }
                // STORY-700: the first-run payoff — a spec the user filed just
                // auto-completed via the merge. Suppressed in --quiet runs.
                // trace:STORY-700 | ai:claude
                if !quiet && !flips.is_empty() {
                    first_run::after_spec_completed(project_root);
                }
            }
            Err(e) => {
                eprintln!(
                    "  {} auto-bump failed: {} (specs stay at Done; \
                     re-run `aida pull` after fixing)",
                    "Warning:".yellow().bold(),
                    e
                );
            }
        }
    }
    // BUG-1633: drop the persisted start point only once a scan actually
    // covered it. Kept when the auto-bump errored, is disabled, or returned
    // without scanning (HEAD off the default branch, `git log` failed), so a
    // later pull on the default branch still resumes from it.
    // trace:BUG-1633 | ai:claude
    if scanned {
        clear_reconcile_state(project_root, PENDING_RECONCILE_BASE_STATE);
    }
    // STORY-441: opt-in auto-archive sweep after the auto-bump settles. Reads
    // `[archive] auto_after_days` from `.aida/config.toml`; absent → no-op.
    // AIDA_AUTO_ARCHIVE=0 disables it. Best-effort: errors are warnings.
    // trace:STORY-441 | ai:claude
    let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
    if let Ok(sweep_backend) = aida_core::CachedGitBackend::open(store_path, &cache_path) {
        maybe_auto_archive_sweep(project_root, &sweep_backend, quiet);
    }
}

/// STORY-248: after `aida pull` lands new commits on main, rebase every
/// stacked branch whose parent was just merged (= branch no longer
/// exists locally or on origin) onto origin/main. Bottom-up: entries
/// whose parent is "main" itself are obviously not affected; entries
/// whose parent vanished are. After each successful rebase we update
/// the recorded SHA for any dependent still pointing at the rebased
/// branch (its branch is the same, its HEAD moved) and `repoint`
/// dependents that pointed at the now-merged parent onto main.
///
/// Safety rails:
///   - `aida_core::rebase::classify` is consulted first; a
///     `DivergedRisky` (file-overlap) classification skips this branch
///     with a clear error pointing at `/aida-rebase`. The user opted in
///     to `--auto`, but file-overlap is the boundary where a human
///     decision matters.
///   - Without `--auto` and stdin attached, prompt per branch; without
///     `--auto` and no stdin (`! IsTerminal::is_terminal`), print a
///     summary of what WOULD be rebased and skip.
///   - Worktree missing for a stacked entry → skip with a warning. The
///     entry stays in the graph; `aida stack show --prune-stale` is the
///     way to clean it up.
///
/// trace:STORY-248 | ai:claude
pub(crate) fn cascade_rebase_stacked_branches(
    project_root: &std::path::Path,
    auto: bool,
) -> anyhow::Result<()> {
    let mut graph = stacks::load(project_root);
    if graph.is_empty() {
        return Ok(());
    }
    // Default-branch short name (e.g. "main") — entries whose parent IS
    // main are not stacked behind anything, so the cascade skips them.
    let default_short = detect_default_branch_ref(project_root)
        .as_deref()
        .and_then(|s| {
            s.strip_prefix("origin/")
                .map(|x| x.to_string())
                .or(Some(s.to_string()))
        })
        .unwrap_or_else(|| "main".to_string());

    // Affected = entries whose parent vanished from the local + origin
    // refs (the signature of `gh pr merge --squash --delete-branch`).
    // Don't flag entries whose parent IS the default branch; main never
    // vanishes.
    let mut affected: Vec<stacks::StackEntry> = graph
        .entries
        .values()
        .filter(|e| e.parent_branch != default_short)
        .filter(|e| !branch_exists_anywhere(project_root, &e.parent_branch))
        .cloned()
        .collect();
    if affected.is_empty() {
        return Ok(());
    }
    // Older entries (deeper in chain) come first.
    affected.sort_by_key(|e| e.created_at);

    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if !auto && !interactive {
        eprintln!(
            "  {} {} stacked branch{} have merged bases; pass `--auto` to rebase \
             (or run `/aida-rebase` interactively): {}",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            affected.len(),
            if affected.len() == 1 { "" } else { "es" },
            affected
                .iter()
                .map(|e| e.branch.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(());
    }

    eprintln!(
        "{} {} stacked branch{} to rebase onto {}",
        crate::glyph(crate::glyphs::Glyph::FlowActive).cyan().bold(),
        affected.len(),
        if affected.len() == 1 { "" } else { "es" },
        default_short.cyan()
    );

    for entry in affected {
        let lease = list_leases(project_root)
            .into_iter()
            .find(|l| l.branch == entry.branch);
        let Some(lease) = lease else {
            eprintln!(
                "  {} skipping `{}` — no lease found (worktree may have been removed; \
                 run `aida stack show --prune-stale` to clean up the graph)",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                entry.branch.cyan()
            );
            continue;
        };
        if !lease.worktree_path.exists() {
            eprintln!(
                "  {} skipping `{}` — worktree {} is missing",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                entry.branch.cyan(),
                lease.worktree_path.display()
            );
            continue;
        }

        // Per-entry prompt when not --auto. A `n` answer skips this
        // branch but continues the cascade — the user might want to
        // handle just one manually.
        if !auto && interactive {
            use std::io::Write;
            eprint!(
                "  Rebase `{}` onto {} (parent `{}` merged)? [y/N] ",
                entry.branch.cyan(),
                default_short.cyan(),
                entry.parent_branch
            );
            let _ = std::io::stderr().flush();
            let mut ans = String::new();
            if std::io::stdin().read_line(&mut ans).is_err() {
                eprintln!(
                    "  {} cascade aborted at stdin EOF",
                    crate::glyph(crate::glyphs::Glyph::Cross).red()
                );
                return Ok(());
            }
            if !matches!(ans.trim(), "y" | "Y") {
                continue;
            }
        }

        // `.aida/stacks.json` is a file on disk, not git output: only a
        // commit ID and a non-option branch may reach `git rebase`.
        // trace:BUG-1622 | ai:claude
        if !git_arg_guard::is_hex_sha(entry.parent_branch_sha.trim())
            || git_arg_guard::is_option_like(&entry.branch)
        {
            eprintln!(
                "  {} skipping `{}` — its stack record is malformed (parent sha `{}`)",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                entry.branch,
                entry.parent_branch_sha
            );
            continue;
        }

        // Refresh origin/<default> in the entry's worktree so the
        // rebase target is the freshly-merged tip, not a stale ref.
        // Best-effort — offline / fetch failure still tries the rebase
        // against whatever cached ref we have.
        let _ = aida_core::git_ops::host_git_command(&lease.worktree_path)
            .args([
                "fetch",
                git_arg_guard::END_OF_OPTIONS,
                "origin",
                &default_short,
            ])
            .output();

        // Run the rebase: `git rebase --onto origin/<default> <parent_sha> <branch>`
        // from the entry's worktree. The 3-arg form skips commits in
        // `parent_sha..` so the parent's pre-squash commits aren't
        // re-applied. We skip the pre-classify-then-rebase split (it'd
        // need a checkout first to read the right branch's ahead/behind)
        // and rely on `git rebase`'s exit status: success means clean,
        // non-zero means conflict, which the conflict arm below handles.
        // trace:STORY-248 | ai:claude
        let onto = format!("origin/{}", default_short);
        let res = aida_core::git_ops::host_git_command(&lease.worktree_path)
            .args([
                "rebase",
                "--onto",
                &onto,
                git_arg_guard::END_OF_OPTIONS,
                entry.parent_branch_sha.trim(),
                &entry.branch,
            ])
            .status();
        match res {
            Ok(s) if s.success() => {
                // New HEAD of the rebased branch — used to update
                // dependents' recorded parent_branch_sha.
                let new_sha = aida_core::git_ops::head_sha(&lease.worktree_path).ok();
                eprintln!(
                    "  {} rebased `{}` onto {} ({})",
                    crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                    entry.branch.cyan(),
                    default_short.cyan(),
                    new_sha
                        .as_deref()
                        .map(|s| &s[..s.len().min(8)])
                        .unwrap_or("<unknown sha>")
                        .dimmed()
                );
                // `entry.branch` is no longer stacked — its parent is
                // now `main`. Drop it from the graph; dependents (whose
                // parent_branch is still entry.branch) get their
                // recorded SHA bumped so their next cascade pass is
                // correct.
                stacks::remove(&mut graph, &entry.branch);
                if let Some(sha) = new_sha.as_deref() {
                    stacks::update_parent_sha(&mut graph, &entry.branch, sha);
                }
                // Any other entries that pointed at the now-merged
                // parent get their `parent_branch` repointed at main
                // (their fork point has effectively moved to main).
                stacks::repoint(
                    &mut graph,
                    &entry.parent_branch,
                    &default_short,
                    new_sha.as_deref().unwrap_or(""),
                );
                stacks::save(project_root, &graph)?;
            }
            Ok(_s) => {
                // Conflict — leave the worktree mid-rebase so the user
                // can resolve via `/aida-rebase` (cleanest signal); the
                // alternative (auto-abort) would re-create the same
                // problem next pull.
                eprintln!(
                    "  {} rebase of `{}` hit conflicts; left in mid-rebase state at {}. \
                     Run `/aida-rebase` or `git rebase --abort` to recover.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    entry.branch.cyan(),
                    lease.worktree_path.display()
                );
                // Cascade aborts here — subsequent branches likely depend on this one.
                return Ok(());
            }
            Err(e) => {
                eprintln!(
                    "  {} could not spawn `git rebase` for `{}`: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    entry.branch.cyan(),
                    e
                );
                continue;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/cascade_rebase_tests.rs"]
mod cascade_rebase_tests;

/// `aida fetch` — read-only refresh of remote refs for both legs (code
/// branch + orphan store) in one shot. No merge, no rebase, no worktree
/// change. Stamps `~/.aida/cache/last-fetch.toml` so the statusline
/// freshness indicator picks up the fetch immediately (same cache key
/// the STORY-79 background fetcher writes). Prints a one-line
/// "N new commits visible on origin" summary per leg unless `--quiet`.
/// trace:TASK-107 | ai:claude
pub(crate) fn handle_fetch_command(
    store_path: &std::path::Path,
    code_only: bool,
    store_only: bool,
    quiet: bool,
) -> Result<()> {
    use aida_core::git_ops;

    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // ---- Code fetch (current branch on the project repo) ----
    if !store_only {
        if !git_ops::has_remote(&project_root, "origin") {
            if !quiet {
                println!(
                    "  {} no `origin` remote — skipping code fetch",
                    "Note:".dimmed()
                );
            }
        } else {
            let branch =
                git_ops::current_branch(&project_root).unwrap_or_else(|_| "HEAD".to_string());
            if !quiet {
                println!("{} {} ← origin", "Fetching code".cyan().bold(), branch);
            }
            let pre = rev_parse_remote(&project_root, &branch);
            match fetch_branch(&project_root, &branch, quiet) {
                Ok(()) => {
                    if !quiet {
                        let post = rev_parse_remote(&project_root, &branch);
                        print_fetch_delta(
                            &project_root,
                            "code",
                            &branch,
                            pre.as_deref(),
                            post.as_deref(),
                        );
                        // STORY-127 detector (2): now that the remote ref is
                        // fresh, a cheap local-vs-origin SHA compare. If they
                        // already match, a subsequent `git pull` / `aida pull`
                        // would be a no-op — say so in one line so the user
                        // doesn't run catch-up commands expecting changes.
                        // trace:STORY-127 | ai:claude
                        let local = git_ops::head_sha(&project_root).ok();
                        if local_main_already_at_origin(local.as_deref(), post.as_deref()) {
                            println!(
                                "  {} local {} already at origin/{} — `aida pull` would be a no-op.",
                                "Note:".dimmed(),
                                branch,
                                branch
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("  {} code fetch failed: {}", "Warning:".yellow().bold(), e);
                }
            }
        }
    }

    // ---- Store fetch (orphan branch) ----
    if !code_only {
        if !git_ops::is_git_repo(store_path) {
            if !quiet {
                println!(
                    "  {} no orphan worktree — skipping store fetch",
                    "Note:".dimmed()
                );
            }
            return Ok(());
        }
        if !git_ops::has_remote(store_path, "origin") {
            if !quiet {
                println!(
                    "  {} orphan store has no `origin` — skipping store fetch",
                    "Note:".dimmed()
                );
            }
            return Ok(());
        }
        let branch =
            git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());
        if !quiet {
            println!("{} aida-store ← origin", "Fetching store".cyan().bold());
        }
        let pre = rev_parse_remote(store_path, &branch);
        match fetch_branch(store_path, &branch, quiet) {
            Ok(()) => {
                // Cache-invalidation hook (shared with STORY-79 bg
                // fetcher): stamp the per-project last-fetch timestamp so
                // statusline / `--sync` decisions see a fresh signal.
                // Best-effort — losing this write only costs a stale
                // freshness indicator for one render cycle.
                let _ = touch_last_fetch_ok(store_path);
                if !quiet {
                    let post = rev_parse_remote(store_path, &branch);
                    print_fetch_delta(
                        store_path,
                        "store",
                        &branch,
                        pre.as_deref(),
                        post.as_deref(),
                    );
                }
            }
            Err(e) => {
                eprintln!("  {} store fetch failed: {}", "Warning:".yellow().bold(), e);
                let _ = write_last_fetch_entry(
                    store_path,
                    &format!(
                        "error: {}",
                        e.to_string().lines().next().unwrap_or("unknown")
                    ),
                );
            }
        }
    }

    Ok(())
}

/// Run `git -C <repo> fetch origin <branch> --prune`. Returns Ok on
/// success, Err with the first line of stderr on failure. Inherits stdio
/// when not quiet so progress bars / hint lines reach the user; pipes
/// stdio to null otherwise. trace:TASK-107 | ai:claude
pub(crate) fn fetch_branch(repo: &std::path::Path, branch: &str, quiet: bool) -> Result<()> {
    // The branch can be a forge-reported PR head a fork author named, so
    // refuse a dash-led one and end options before it. trace:BUG-1622 | ai:claude
    git_arg_guard::reject_option_like("branch", branch)?;
    // trace:BUG-1929 | ai:claude
    let mut cmd = aida_core::git_ops::host_git_command(repo);
    cmd.args([
        "fetch",
        "--prune",
        git_arg_guard::END_OF_OPTIONS,
        "origin",
        branch,
    ]);
    if quiet {
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        let out = cmd.output()?;
        if !out.status.success() {
            let msg = String::from_utf8_lossy(&out.stderr)
                .lines()
                .next()
                .unwrap_or("unknown error")
                .to_string();
            anyhow::bail!("{}", msg);
        }
    } else {
        let status = cmd.status()?;
        if !status.success() {
            anyhow::bail!("git fetch exited with status {}", status);
        }
    }
    Ok(())
}

/// Best-effort `git rev-parse origin/<branch>`. Returns None when the
/// ref doesn't exist locally yet (e.g. first ever fetch) or git fails;
/// the caller uses None to mean "no comparison baseline".
/// trace:TASK-107 | ai:claude
pub(crate) fn rev_parse_remote(repo: &std::path::Path, branch: &str) -> Option<String> {
    let out = aida_core::git_ops::host_git_command(repo)
        .args(["rev-parse", &format!("origin/{}", branch)])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Print one line summarizing what landed on `origin/<branch>` since the
/// pre-fetch snapshot. Silent when pre/post match (nothing new) or when
/// we can't compute the count. Format keeps both legs visually distinct
/// via the `<leg>:` prefix (`code:` or `store:`).
/// trace:TASK-107 | ai:claude
pub(crate) fn print_fetch_delta(
    repo: &std::path::Path,
    leg: &str,
    branch: &str,
    pre: Option<&str>,
    post: Option<&str>,
) {
    let post = match post {
        Some(p) => p,
        None => {
            // No remote-tracking ref after fetch — likely upstream is
            // gone. Stay silent rather than printing a confusing "0 new
            // commits" line.
            return;
        }
    };
    let n = match pre {
        Some(pre) if pre == post => 0,
        Some(pre) => {
            let range = format!("{}..{}", pre, post);
            aida_core::git_ops::host_git_command(repo)
                .args(["rev-list", "--count", &range])
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        String::from_utf8_lossy(&o.stdout)
                            .trim()
                            .parse::<u64>()
                            .ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0)
        }
        None => {
            // First-ever sight of the remote ref. Don't claim a count we
            // can't justify — say "now visible" instead.
            println!(
                "  {} {} origin/{} now visible",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                leg,
                branch
            );
            return;
        }
    };
    if n == 0 {
        println!(
            "  {} {} origin/{} already up-to-date",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            leg,
            branch
        );
    } else {
        println!(
            "  {} {} {} new commit{} on origin/{}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            leg,
            n,
            if n == 1 { "" } else { "s" },
            branch
        );
    }
}
