// trace:TASK-3428 | ai:codex
use crate::git_arg_guard;
use colored::Colorize;
// ----------------------------------------------------------------------------
// `aida statusline` — fast one-liner for shell prompts and Claude Code's
// statusLine.command setting. Cache-only; no git operations, no API calls.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

// ── Sandbox store (SPIKE-48) ─────────────────────────────────────────────
//
// A throwaway, discardable git-canonical store for drain-testing and scenario
// play. It is an ORDINARY store directory targeted via the `AIDA_STORE` env
// override (resolved in `aida_store_override`), so every `aida` command run in a
// shell that exports it operates on the sandbox instead of the project's real
// `aida-store` orphan branch. Lowest-machinery by design: a temp dir + a git
// repo + the same `GitBackend` init the real store uses — no daemon, no orphan
// branch, no remote. This is the dev-playground slice of SPIKE-48; the
// first-user/tutorial layer is deferred. trace:SPIKE-48 | ai:claude

pub(crate) fn short_sha(sha: &str) -> String {
    if sha.len() >= 7 {
        sha[..7].to_string()
    } else {
        sha.to_string()
    }
}

/// Verdict for the code↔store SHA pairing. trace:BUG-584 | ai:claude
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum StoreDriftVerdict {
    /// Nothing to compare — no trailer, or no `.aida-store/`.
    NoVerdict,
    /// Paired SHA == current store HEAD.
    Aligned,
    /// Current store HEAD is a fast-forward descendant of the paired SHA —
    /// the store simply moved forward via normal spec writes (`aida add` /
    /// `aida done`). This is the **normal, healthy** state, NOT drift: the
    /// store SHOULD be ahead of a code commit's pin once you file specs.
    StoreAhead,
    /// The paired SHA is NOT an ancestor of the current store HEAD — a
    /// genuine divergence: a store-side history rewrite, or a paired SHA that
    /// no longer exists locally. This is the only state worth warning about.
    Diverged,
}

/// Pure drift verdict: given the code commit's paired store SHA (from the
/// `Aida-Store:` trailer), the current orphan-store HEAD, and whether the
/// paired SHA is an ancestor of the current store HEAD, decide the pairing
/// state.
///
/// The pin's purpose (STORY-49) is to detect when the store has genuinely
/// *diverged* from what a code commit was paired against (a rewind/rewrite).
/// But the orphan store legitimately advances ahead of the pin on every
/// normal `aida add` / `aida done` write, so a bare `paired != current`
/// comparison fires on day one after ordinary use (BUG-584). We only treat a
/// difference as drift when the paired SHA is **not** an ancestor of the
/// current store HEAD — i.e. the store has truly diverged, not merely moved
/// forward.
///
/// `paired_is_ancestor_of_current` is the caller-supplied ancestry fact
/// (`git merge-base --is-ancestor <paired> <current>`); kept as a parameter
/// so this stays a pure, unit-testable function.
/// trace:STORY-49 trace:BUG-584 | ai:claude
pub(crate) fn store_drift_verdict(
    paired_store_sha: Option<&str>,
    current_store_head: Option<&str>,
    paired_is_ancestor_of_current: bool,
) -> StoreDriftVerdict {
    match (paired_store_sha, current_store_head) {
        (Some(p), Some(c)) if p.trim() == c.trim() => StoreDriftVerdict::Aligned,
        (Some(_), Some(_)) if paired_is_ancestor_of_current => StoreDriftVerdict::StoreAhead,
        (Some(_), Some(_)) => StoreDriftVerdict::Diverged,
        _ => StoreDriftVerdict::NoVerdict,
    }
}

/// True when `ancestor` is an ancestor of (or equal to) `descendant` in the
/// orphan store's git history — i.e. the store fast-forwarded from `ancestor`
/// to `descendant`. Resolved via `git merge-base --is-ancestor` inside the
/// `.aida-store/` worktree. Returns `false` on any git error or when either
/// SHA is missing locally (which is itself a divergence signal, so the
/// caller correctly reports drift). trace:BUG-584 | ai:claude
pub(crate) fn store_sha_is_ancestor(
    store_path: &std::path::Path,
    ancestor: &str,
    descendant: &str,
) -> bool {
    if !store_path.exists() {
        return false;
    }
    aida_core::git_ops::host_git_command(store_path)
        .args([
            "merge-base",
            "--is-ancestor",
            git_arg_guard::END_OF_OPTIONS,
            ancestor,
            descendant,
        ]) // trace:BUG-1622 | ai:claude
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Read the `Aida-Store:` trailer SHA from the code HEAD commit message at
/// `project_root`, if present. Returns `None` on any git error, no trailer,
/// or no commits. trace:STORY-49 | ai:claude
pub(crate) fn paired_store_sha_for_head(project_root: &std::path::Path) -> Option<String> {
    let head_msg = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", "-1", "--format=%B"])
        .output()
        .ok()?;
    if !head_msg.status.success() {
        return None;
    }
    let head_msg = String::from_utf8_lossy(&head_msg.stdout).to_string();
    let trailers = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["interpret-trailers", "--parse"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    {
        use std::io::Write;
        if let Some(mut stdin) = trailers.stdin.as_ref() {
            let _ = stdin.write_all(head_msg.as_bytes());
        }
    }
    let trailer_output = trailers.wait_with_output().ok()?;
    let trailer_text = String::from_utf8_lossy(&trailer_output.stdout).to_string();
    trailer_text
        .lines()
        .find_map(|l| l.strip_prefix("Aida-Store:").map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
}

/// Current orphan-store HEAD SHA from `<project_root>/.aida-store`, if the
/// worktree exists and git can resolve it. trace:STORY-49 | ai:claude
pub(crate) fn current_store_head_sha(project_root: &std::path::Path) -> Option<String> {
    let store_path = project_root.join(".aida-store");
    if !store_path.exists() {
        return None;
    }
    aida_core::git_ops::host_git_command(&store_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
}

/// STORY-49: one-line store/code SHA-drift warning for `aida status`.
///
/// Warn-only, and only on **genuine** divergence. The code HEAD's paired
/// store SHA (`Aida-Store:` trailer) is a pin recorded at commit time; the
/// orphan store legitimately moves ahead of it on every normal `aida add` /
/// `aida done`, so a bare "paired != current" comparison nagged on day one
/// after ordinary use (BUG-584). We surface the yellow line only when the
/// store has truly diverged (paired SHA is not an ancestor of the current
/// store HEAD — a rewind/rewrite, or a paired SHA missing locally). Silent
/// when aligned, when the store is simply ahead (the normal, healthy state),
/// when there is no trailer (hook not installed / pre-hook commit), or when
/// there is no `.aida-store/`. trace:STORY-49 trace:BUG-584
pub(crate) fn print_status_store_drift_section(project_root: &std::path::Path) {
    let paired = paired_store_sha_for_head(project_root);
    let current = current_store_head_sha(project_root);
    let store_path = project_root.join(".aida-store");
    let is_ancestor = match (paired.as_deref(), current.as_deref()) {
        (Some(p), Some(c)) => store_sha_is_ancestor(&store_path, p, c),
        _ => false,
    };
    if let StoreDriftVerdict::Diverged =
        store_drift_verdict(paired.as_deref(), current.as_deref(), is_ancestor)
    {
        println!(
            "  {} code HEAD pinned store {} but the current store {} diverged from it — run {} for detail",
            "Store drift:".bold().yellow(),
            short_sha(paired.as_deref().unwrap_or("")).dimmed(),
            short_sha(current.as_deref().unwrap_or("")).dimmed(),
            "aida store status".cyan()
        );
        println!();
    }
}

#[cfg(test)]
#[path = "tests/story_49_store_drift_tests.rs"]
mod story_49_store_drift_tests;

// ----------------------------------------------------------------------------
// EPIC-19 — shared `aida doctor` diagnostics-collection machinery. The command
// handlers (`handle_doctor_command`, `doctor_multi_agent`, the heal + migration
// paths) live in `doctor/`; the collection helpers below stay here because
// `aida status --full`/`--ci` (STORY-707) drive them too.
// ----------------------------------------------------------------------------

// ============================================================================
// TASK-878: auto-GC merged Agent-tool worktrees.
//
// The Agent tool (and `aida agent new --isolation worktree`) leaves an isolated
// worktree under `.claude/worktrees/agent-<id>` on a `worktree-agent-<id>`
// branch. `git worktree prune` only removes worktrees whose DIRECTORY is gone —
// these keep their dir + branch, so they accumulate (98 piled up in one fan-out
// session, BUG-582) and the stale local branches make `aida human`'s
// reviews-awaiting view false-positive on already-merged work.
//
// This adds a `merged-agent-worktrees` doctor category that surfaces those
// worktrees and verify-and-removes them under a squash-aware safety model:
//
//   removable iff a positive "this work has shipped" signal AND no risk:
//     - the branch HEAD is an ancestor of origin/main (ff / non-squash merge), OR
//     - the branch's PR is MERGED on the forge (covers squash merges, where the
//       branch tip keeps a different SHA but the work shipped)
//   AND none of the KEEP conditions apply:
//     - the worktree has uncommitted changes (clean != no work — never delete), OR
//     - the branch carries genuinely-unique unmerged commits not in main
//   When NO merged signal exists at all, the worktree is KEPT and flagged for the
//   operator (never auto-removed).
//
// Read-only by default (list + per-worktree verdict). The heal is DESTRUCTIVE
// (removes a worktree + deletes its branch) so `safe_heal=false`: it routes
// through the STORY-666 destructive-heal gate — refused in an unattended /
// autonomous context, performed only under explicit interactive --yes --force
// sign-off. trace:TASK-878
// ============================================================================

// ============================================================================
// BUG-614: conservative worktree GC.
//
// `git worktree prune` (lossless, run on session-end) removes only the
// administrative entries whose directories are already gone — never a live
// worktree. The explicit operator GC (`aida session gc`) is the destructive
// half: it removes a `.claude/worktrees/agent-*` worktree ONLY when ALL of
//   - its branch is merged into the default branch OR gone, AND
//   - the worktree is CLEAN (no uncommitted/untracked changes), AND
//   - it is NOT locked (no `locked` line in `git worktree list --porcelain`),
//     AND
//   - no live process / active session-lease references it.
// hold. Anything else is PRESERVED and reported. Removal uses
// `git worktree remove` (NOT `--force`) so git's own safety checks add a second
// guard; if git refuses we SKIP (never `-f`) and report. trace:BUG-614
// ============================================================================
