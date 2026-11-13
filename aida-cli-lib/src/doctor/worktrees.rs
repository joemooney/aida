// trace:TASK-3781 | ai:codex

use super::*;

/// One-line result of the post-ship GC. Names the worktrees it passed over for
/// want of a local merge signal (a full `aida doctor` run may still reclaim
/// them through the forge lookup) and, when the time budget ran out, how many
/// were left unexamined or unremoved for `aida doctor` to finish.
// trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
pub(crate) fn post_ship_gc_summary(
    reclaimable: usize,
    skipped: usize,
    unexamined: usize,
    unremoved: usize,
) -> String {
    let mut line = format!(
        "  ✓ merged-agent-worktrees cleanup checked ({reclaimable} reclaimable, \
         {skipped} without a local merge signal skipped)"
    );
    if unexamined + unremoved > 0 {
        let mut left = Vec::new();
        if unexamined > 0 {
            left.push(format!("{unexamined} worktree(s) not examined"));
        }
        if unremoved > 0 {
            left.push(format!("{unremoved} reclaimable worktree(s) not removed"));
        }
        line.push_str(&format!(
            "; stopped at the {}s budget with {} — \
             run `aida doctor --category merged-agent-worktrees` to finish",
            POST_SHIP_GC_BUDGET.as_secs(),
            left.join(" and ")
        ));
    } else {
        line.push_str("; details: `aida doctor --category merged-agent-worktrees`");
    }
    line
}

/// Resolve where interactive agent launches will find their worktree, and warn when that
/// directory carries no Claude Code folder-trust record.
///
/// Claude Code gates a directory it has never been launched in behind an interactive
/// folder-trust modal ("Quick safety check: Is this a project you created or one you trust?").
/// A fresh per-spec worktree is exactly such a directory, so `aida agent new claude --spec <ID>`
/// stops dead on a modal in a PTY nobody is watching. Trust INHERITS from a parent directory
/// (probed 2026-09-27 with a control), which is what `[worktree_pool] worktree_parent` exists to
/// exploit: nest every worktree under ONE directory, trust that once, and no future worktree
/// prompts again.
///
/// This check is strictly READ-ONLY. The only writer of `~/.claude.json` is the opt-in
/// per-seat pre-trust at launch (`[worktree_pool] trust_seat_worktrees`, BUG-049,
/// `crate::claude_trust`); this check only reports whether it is on and whether it
/// left entries for seats that no longer exist.
///
/// Silent when trust cannot be determined (no readable `~/.claude.json`), so an unreadable
/// config reads as unknown rather than as a false alarm.
// trace:BUG-1700 | ai:claude
pub(super) fn scan_agent_worktree_trust(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    // BUG-2394: the Claude Code folder-trust modal only matters where Claude
    // launches; a project whose `[agents] enabled` list omits claude must not
    // be warned about ~/.claude.json.
    // trace:BUG-2394 | ai:claude
    if !claude_launch_active(
        crate::init_cmd::read_enabled_agent_selection(project_root).map(|s| s.claude),
    ) {
        return Vec::new();
    }
    let configured = crate::worktree_pool_config_worktree_parent(project_root);
    let resolved = resolve_worktree_parent(project_root, configured.as_deref());
    let parent = normalize_path_lexically(&resolved);
    let trusted = claude_trusted_paths();
    let breadth = trusted.as_deref().and_then(|trusted| {
        covering_trusted_ancestor(&parent, trusted).map(|ancestor| TrustBreadth {
            unrelated_dirs: count_unrelated_child_dirs(&ancestor, project_root, &parent),
        })
    });
    let mut findings =
        worktree_trust_findings(&resolved, configured.is_some(), trusted.as_deref(), breadth);
    // BUG-049: report the per-seat pre-trust — off: say how to turn it on in the
    // finding that needs it; either way: entries left for seats now gone.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude
    let enabled = crate::claude_trust::trust_enabled(project_root);
    let stale = crate::home_dir()
        .map(|home| crate::claude_trust::stale_seat_entries(&home, project_root))
        .unwrap_or_default();
    seat_trust_findings(&mut findings, enabled, &stale);
    findings
}

/// BUG-2394: whether Claude is a launch vendor for this project. Only an
/// explicit `[agents] enabled` list that omits claude turns it off
/// (`enabled_claude == Some(false)`). `[agents] vendor` names the DEFAULT
/// vendor and deactivates nothing: with no enabled list every vendor is
/// enabled, and a `vendor = "codex"` project still launches Claude (reviewer
/// lanes, `--vendor claude`, bg seats). Pure.
// trace:BUG-2394 | ai:claude
pub(crate) fn claude_launch_active(enabled_claude: Option<bool>) -> bool {
    enabled_claude.unwrap_or(true)
}

/// BUG-049: fold the per-seat pre-trust state into the worktree-trust findings.
/// Pure over its inputs.
// trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude
pub(super) fn seat_trust_findings(
    findings: &mut Vec<DoctorFinding>,
    enabled: bool,
    stale: &[std::path::PathBuf],
) {
    // trace:BUG-B6B1278890774EF993D2EBC327862470-049.ac891c92 | ai:claude
    if enabled {
        // On: reported whatever the parent's trust, since AIDA then writes
        // ~/.claude.json at every implementer launch.
        findings.push(DoctorFinding {
            category: "agent-launch".to_string(),
            id: "worktree-seat-pre-trust".to_string(),
            summary: format!(
                "per-seat pre-trust is on (`[worktree_pool] {}`): AIDA trusts each \
                 implementer seat and compete lane in ~/.claude.json before claude starts \
                 there, so the lane keeps the project's permissions.allow list (review \
                 trees are never trusted)",
                crate::claude_trust::CONFIG_KEY,
            ),
            action: format!(
                "none needed; set `[worktree_pool] {} = false` (or {}=0 on this host) to stop \
                 AIDA writing ~/.claude.json",
                crate::claude_trust::CONFIG_KEY,
                crate::claude_trust::ENV_OVERRIDE,
            ),
            safe_heal: false,
        });
    }
    for finding in findings
        .iter_mut()
        .filter(|f| f.id == "worktree-folder-trust")
    {
        if !enabled {
            finding.summary.push_str(
                ", and a headless lane there ignores the project's permissions.allow list",
            );
            finding.action.push_str(&format!(
                "; or set `[worktree_pool] {} = true` in .aida/config.toml (or {}=1 on this \
                 host) so AIDA trusts each seat in ~/.claude.json before it launches claude there",
                crate::claude_trust::CONFIG_KEY,
                crate::claude_trust::ENV_OVERRIDE,
            ));
        }
    }
    if !stale.is_empty() {
        findings.push(DoctorFinding {
            category: "agent-launch".to_string(),
            id: "worktree-seat-trust-stale".to_string(),
            summary: format!(
                "~/.claude.json holds {} Claude Code trust entr{} for worktrees that no longer \
                 exist (e.g. {})",
                stale.len(),
                if stale.len() == 1 { "y" } else { "ies" },
                stale[0].display()
            ),
            action: if enabled {
                "the next seat launch prunes them automatically; or remove those `projects` \
                 entries from ~/.claude.json by hand"
            } else {
                "remove those `projects` entries from ~/.claude.json by hand (pre-trust is off, \
                 so AIDA does not edit the file)"
            }
            .to_string(),
            safe_heal: false,
        });
    }
}

// trace:BUG-1744 | ai:codex
pub(super) fn count_unrelated_child_dirs(
    ancestor: &std::path::Path,
    project_root: &std::path::Path,
    worktree_parent: &std::path::Path,
) -> usize {
    let Ok(entries) = std::fs::read_dir(ancestor) else {
        return 0;
    };
    let project_root = normalize_path_lexically(project_root);
    let worktree_parent = normalize_path_lexically(worktree_parent);
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let entry_path = normalize_path_lexically(&entry.path());
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // TASK-2053: seat clones are this project's own trees.
            // trace:TASK-2053 | ai:claude
            !name.starts_with('.')
                && !name.starts_with(aida_core::seat_clone_detect::CLONE_DIR_PREFIX)
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry_path != project_root
                && entry_path != worktree_parent
        })
        .count()
}

/// The directory newly created pool worktrees land in — mirrors `worktree_pool::pool_path_for`'s
/// parent selection: the configured `worktree_parent` (relative resolved against the project
/// root), else the project root's own parent (the historical sibling layout).
// trace:BUG-1700 | ai:claude
pub(super) fn resolve_worktree_parent(
    project_root: &std::path::Path,
    configured: Option<&std::path::Path>,
) -> std::path::PathBuf {
    match configured {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => project_root.join(p),
        None => project_root
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| project_root.to_path_buf()),
    }
}

/// Resolve `.` and `..` components WITHOUT touching the filesystem.
///
/// TASK-1561: delegates to `worktree_pool::normalize_path_lexically`. This used
/// to be a second copy; the placement rule needs the same normalisation for the
/// same reason (a relative `worktree_parent` is the shipped example, and the
/// resulting path is both printed and lexically compared), and two copies of a
/// path rule is how `worktree_parent` came to be honoured by the pool and by
/// nothing else.
// trace:BUG-1700 | ai:claude
// trace:TASK-1561 | ai:claude
pub(super) fn normalize_path_lexically(path: &std::path::Path) -> std::path::PathBuf {
    aida_core::worktree_pool::normalize_path_lexically(path)
}

/// Paths with an accepted Claude Code folder-trust record, from `~/.claude.json`. `None` when the
/// file is absent or unparseable — the caller must then stay silent rather than guess.
// trace:BUG-1700 | ai:claude
pub(super) fn claude_trusted_paths() -> Option<Vec<std::path::PathBuf>> {
    // `crate::home_dir()`, not a direct HOME read: it is the crate's single home
    // resolver and carries the cfg(test) redirect to a temp home, enforced by the
    // `no_direct_home_resolution_in_crate` guard (TASK-1513). That redirect also
    // keeps this check from reading a developer's REAL ~/.claude.json under test.
    let home = crate::home_dir()?;
    let body = std::fs::read_to_string(home.join(".claude.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&body).ok()?;
    let projects = value.get("projects")?.as_object()?;
    Some(
        projects
            .iter()
            .filter(|(_, v)| {
                v.get("hasTrustDialogAccepted")
                    .and_then(|t| t.as_bool())
                    .unwrap_or(false)
            })
            .map(|(k, _)| std::path::PathBuf::from(k))
            .collect(),
    )
}

/// Breadth of unrelated directories covered by a trusted ancestor.
// trace:BUG-1744 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TrustBreadth {
    /// Direct child directories of the covering trusted ancestor that are neither
    /// the project root nor the resolved worktree parent — the unrelated projects
    /// the one grant also covers.
    pub(super) unrelated_dirs: usize,
}

/// Find the trusted ancestor that covers the resolved worktree parent.
// trace:BUG-1744 | ai:codex
pub(super) fn covering_trusted_ancestor(
    parent: &std::path::Path,
    trusted: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    trusted
        .iter()
        .map(|path| normalize_path_lexically(path))
        .find(|path| parent.starts_with(path))
}

/// Pure verdict: does `parent` (or an ancestor of it) carry a trust record?
///
/// Pure over its inputs so the inheritance rule is testable without a real `~/.claude.json` or a
/// spawned agent — which is what makes this assertable at all, since the modal only appears in an
/// interactive TUI.
// trace:BUG-1700 | ai:claude
pub(super) fn worktree_trust_findings(
    parent: &std::path::Path,
    parent_configured: bool,
    trusted: Option<&[std::path::PathBuf]>,
    breadth: Option<TrustBreadth>,
) -> Vec<DoctorFinding> {
    let Some(trusted) = trusted else {
        return Vec::new();
    };
    // `starts_with` is purely LEXICAL, so it must not see unresolved `..`. A
    // relative `worktree_parent = "../aida-worktrees"` resolves to
    // `<root>/../aida-worktrees`, which does not lexically start with the
    // `<parent-of-root>/aida-worktrees` spelling `~/.claude.json` records — so
    // an already-trusted directory would be reported as untrusted. Normalise
    // both sides before comparing. trace:BUG-1700 | ai:claude
    let parent = normalize_path_lexically(parent);
    if let Some(ancestor) = covering_trusted_ancestor(&parent, trusted) {
        if let Some(breadth) = breadth {
            if !parent_configured && breadth.unrelated_dirs >= 1 {
                let finding = DoctorFinding {
                    category: "agent-launch".to_string(),
                    id: "worktree-folder-trust-breadth".to_string(),
                    summary: format!(
                        "the grant on {} also covers {} unrelated directories, though nothing is broken today",
                        ancestor.display(), breadth.unrelated_dirs
                    ),
                    action: "set `[worktree_pool] worktree_parent` in .aida/config.toml to one directory (e.g. \"../aida-worktrees\") and accept the folder-trust prompt there once".to_string(),
                    safe_heal: false,
                };
                return vec![finding];
            }
        }
        return Vec::new();
    }
    let parent = parent.as_path();
    let action = if parent_configured {
        format!(
            "launch an interactive agent in {} ONCE and accept the folder-trust prompt; every \
             worktree nests under it, so trust inherits and no later worktree prompts again",
            parent.display()
        )
    } else {
        format!(
            "set `[worktree_pool] worktree_parent` in .aida/config.toml to one directory (e.g. \
             \"../aida-worktrees\") and accept the folder-trust prompt there ONCE — otherwise \
             every new spec worktree under {} is a fresh untrusted path with its own modal",
            parent.display()
        )
    };
    vec![DoctorFinding {
        category: "agent-launch".to_string(),
        id: "worktree-folder-trust".to_string(),
        summary: format!(
            "worktrees are created under {}, which has no Claude Code folder-trust record — an \
             interactive `aida agent new claude --spec <ID>` there stops on a folder-trust \
             modal instead of starting work",
            parent.display()
        ),
        action,
        safe_heal: false,
    }]
}

// The classification verdict for one agent-managed worktree. trace:TASK-878
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
// trace:BUG-1719 | ai:codex
pub(crate) enum AgentWorktreeVerdict {
    /// Verified merged AND clean AND no unique unmerged commits → safe to GC.
    Removable(String),
    /// Dirty, carrying unmerged work, or no merge signal → keep, flag operator.
    Keep { reason: String, actionable: bool },
}

// trace:BUG-1719 | ai:codex
pub(super) fn agent_worktree_keep_action(actionable: bool) -> String {
    if actionable {
        "operator decision: review and keep, open a PR, or remove by hand".to_string()
    } else {
        "no action required — kept because batched content is undecidable".to_string()
    }
}

// BUG-1800: Cache per-worktree verdicts keyed by branch tip SHA so expensive
// content verification probes only run once per un-updated undecidable tree.
// trace:BUG-1800 | ai:antigravity
pub(super) fn worktree_verdicts_cache_path(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida/cache/worktree-verdicts.json")
}

pub(super) fn load_worktree_verdicts(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, AgentWorktreeVerdict> {
    let path = worktree_verdicts_cache_path(project_root);
    if let Ok(data) = std::fs::read_to_string(path) {
        if let Ok(parsed) = serde_json::from_str(&data) {
            return parsed;
        }
    }
    std::collections::HashMap::new()
}

pub(super) fn save_worktree_verdicts(
    project_root: &std::path::Path,
    verdicts: &std::collections::HashMap<String, AgentWorktreeVerdict>,
) {
    let path = worktree_verdicts_cache_path(project_root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_string_pretty(verdicts) {
        let _ = std::fs::write(path, data);
    }
}

/// Inputs to the pure agent-worktree classifier — every git/forge probe result
/// the safety model needs, gathered once per worktree by the scanner. Keeping
/// the classification pure makes the squash-aware safety model unit-testable
// without git or a forge. trace:TASK-878
#[derive(Debug, Clone)]
pub(crate) struct AgentWorktreeFacts {
    /// True when the worktree has uncommitted changes (tracked or staged dirt,
    /// or untracked files). A dirty worktree is NEVER removed — clean != no work,
    /// and dirty is unambiguously "has work".
    pub(crate) dirty: bool,
    /// True when the branch HEAD is an ancestor of origin/main (fully merged —
    /// covers fast-forward / non-squash merges; implies zero unique commits).
    pub(crate) ancestor_of_main: bool,
    /// True when the forge reports a MERGED PR for this branch (covers squash
    /// merges, where the branch tip keeps a different SHA but the work shipped).
    pub(crate) pr_merged: bool,
    /// Count of commits on the branch not reachable from origin/main. Zero means
    /// nothing unique is at risk; non-zero with no ancestor signal means the
    /// branch carries unique unmerged work that must be KEPT — UNLESS
    /// `content_fully_landed` clears it (see below).
    pub(crate) unique_unmerged_commits: u32,
    /// True when a CONTENT comparison (not ancestry) proves every file the
    /// branch's own commits touched already matches origin/main's current
    /// tree byte-for-byte. A squash merge gives the branch's original commits
    /// a permanently different SHA from the squash commit on main, so plain
    /// ancestry — and a naive commit count derived from it — can NEVER clear
    /// a squash-merged branch; `unique_unmerged_commits` stays positive
    /// forever even when nothing unique remains (BUG-1287). This field is the
    /// squash-aware refinement: computed only when `pr_merged` is true, it
    /// lets a confirmed-merged branch with a positive commit count still be
    /// classified Removable when content proves the count is an ancestry
    /// artifact, while a branch that genuinely carries post-merge work (extra
    /// commits whose content is NOT yet reflected on main) still Keeps.
    /// Never set true when there is any doubt — the caller (`scan_*`) leaves
    /// it false whenever the git probes needed to prove it are inconclusive.
    // trace:BUG-1287 | ai:claude
    pub(crate) content_fully_landed: bool,
    /// True when the default branch carries a landed commit whose subject or
    /// body-line trailer names this branch's (finished) spec — the shape a
    /// batched integration PR leaves behind: the spec's own branch is never
    /// merged or PR'd directly, so neither `ancestor_of_main` nor `pr_merged`
    /// can fire. Like `pr_merged` it is only a merge SIGNAL: a positive
    /// `unique_unmerged_commits` still needs `content_fully_landed` to clear.
    // trace:BUG-1657 | ai:claude
    pub(crate) spec_trailer_on_main: bool,
}

/// Pure squash-aware classification of one agent-managed worktree. No git/forge
/// — every input is pre-gathered in `AgentWorktreeFacts`. This is the safety
/// model: a dirty worktree is always kept; removal needs a positive merged
/// signal AND zero unique unmerged commits; otherwise the worktree is kept and
// flagged. trace:TASK-878 | ai:claude
// trace:BUG-1719 | ai:codex
pub(crate) fn classify_agent_worktree(facts: &AgentWorktreeFacts) -> AgentWorktreeVerdict {
    // KEEP first — uncommitted work is unambiguously "has work". Clean != no
    // work, but dirty is definitely work; never delete it.
    if facts.dirty {
        return AgentWorktreeVerdict::Keep {
            reason: "uncommitted changes present — never auto-removed".to_string(),
            actionable: true,
        };
    }

    // A positive "this work has shipped" signal — either suffices.
    let merged_reason = if facts.ancestor_of_main {
        Some("branch is an ancestor of origin/main (merged)")
    } else if facts.pr_merged {
        Some("its PR is merged (squash-merged)")
    } else if facts.spec_trailer_on_main {
        // trace:BUG-1657 | ai:claude
        Some("its spec landed on origin/main through an integration merge")
    } else {
        None
    };

    let Some(merged_reason) = merged_reason else {
        // No merged signal at all → never remove; flag for the operator.
        return AgentWorktreeVerdict::Keep {
            reason: "no merge signal — operator decision".to_string(),
            actionable: true,
        };
    };

    // Even with a merged signal, a branch carrying genuinely-unique unmerged
    // commits (e.g. extra commits added after the PR squash-merged) must be KEPT
    // — removing it would lose work. Ancestor-of-main implies zero unique
    // commits, so this only bites the squash-merge path.
    if facts.unique_unmerged_commits > 0 && !facts.ancestor_of_main {
        // BUG-1287: a positive commit count on the squash-merge path is not
        // proof of unique WORK — it is guaranteed positive forever by ancestry
        // alone, squash or no squash. `content_fully_landed` is the content-level
        // check that clears the ancestry artifact: every file the branch's own
        // commits touched already matches main byte-for-byte, so nothing is at
        // risk despite the non-zero count.
        if facts.content_fully_landed {
            return AgentWorktreeVerdict::Removable(format!(
                "{merged_reason}, {} commit(s) content-verified fully landed on origin/main \
                 (ancestry alone can't clear a squash-merged branch)",
                facts.unique_unmerged_commits
            ));
        }
        if facts.spec_trailer_on_main {
            // trace:BUG-1718 | ai:codex
            return AgentWorktreeVerdict::Keep { reason: format!(
                "{merged_reason}; {} unique unmerged commit(s) are a squash-ancestry artifact from a \
                 batched integration merge, and content is undecidable — no action required",
                facts.unique_unmerged_commits
            ), actionable: false };
        }
        return AgentWorktreeVerdict::Keep {
            reason: format!(
                "{merged_reason}, but {} unique unmerged commit(s) — keep, operator decision",
                facts.unique_unmerged_commits
            ),
            actionable: true,
        };
    }

    AgentWorktreeVerdict::Removable(merged_reason.to_string())
}

/// Content-level (not ancestry) proof that `branch`'s own commits are already
/// represented by patch-equivalent commits in `default_ref`. This accepts both
/// per-commit equivalence and a branch's cumulative patch matching one squash
/// commit, because a multi-commit branch has no individual patch equivalent to
/// that combined commit. Patch equivalence is durable: later default-branch
/// commits may change the same files without making already-landed work appear
/// unique again. Every comparison is bounded to commits since the merge-base.
///
/// Conservative on any doubt: an unresolvable merge-base or a failed git call
/// returns `false` (stay KEPT), never `true`. Read-only — no writes, no
/// network.
// trace:BUG-1287 | ai:claude
// trace:BUG-1424 | ai:codex
pub(crate) fn branch_content_fully_landed(
    project_root: &std::path::Path,
    default_ref: &str,
    branch: &str,
) -> bool {
    use std::io::Write;
    use std::process::{Command as PCmd, Stdio};

    let run = |args: &[&str]| -> Option<std::process::Output> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
    };
    let patch_id = |patch: &[u8]| -> Option<Option<String>> {
        let mut child = PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["patch-id", "--stable"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(patch).ok()?;
        let output = child.wait_with_output().ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8(output.stdout).ok()?;
        let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
        let Some(line) = lines.next() else {
            return Some(None);
        };
        let id = line.split_whitespace().next()?.to_string();
        if lines.next().is_some() {
            return None;
        }
        Some(Some(id))
    };

    // trace:BUG-1622 | ai:claude
    let Some(mb_out) = run(&[
        "merge-base",
        git_arg_guard::END_OF_OPTIONS,
        default_ref,
        branch,
    ]) else {
        return false;
    };
    if !mb_out.status.success() {
        return false;
    }
    let merge_base = String::from_utf8_lossy(&mb_out.stdout).trim().to_string();
    if merge_base.is_empty() {
        return false;
    }

    // List branch-side commits with no patch-equivalent commit on the default
    // side. `^merge_base` explicitly bounds both histories to the divergent
    // range. Empty output means all branch work landed, even if later commits
    // changed the same paths. Any git failure stays conservative (KEPT).
    let range = format!("{default_ref}...{branch}");
    let Some(unique_out) = run(&[
        "rev-list",
        "--cherry-pick",
        "--right-only",
        &range,
        &format!("^{merge_base}"),
    ]) else {
        return false;
    };
    if !unique_out.status.success() {
        return false;
    }
    if String::from_utf8_lossy(&unique_out.stdout)
        .trim()
        .is_empty()
    {
        return true;
    }

    // A forge commonly combines every branch commit into one squash commit.
    // `rev-list --cherry-pick` cannot recognize that N-to-1 equivalence, so
    // compare the branch's combined diff with each bounded default-side commit.
    // trace:BUG-1622 | ai:claude
    let Some(branch_diff) = run(&[
        "diff",
        "--binary",
        git_arg_guard::END_OF_OPTIONS,
        &merge_base,
        branch,
        "--",
    ]) else {
        return false;
    };
    if !branch_diff.status.success() {
        return false;
    }
    let Some(Some(branch_patch_id)) = patch_id(&branch_diff.stdout) else {
        return false;
    };

    let default_range = format!("{merge_base}..{default_ref}");

    // BUG-1288: this fallback used to spawn a `git show | git patch-id` PAIR
    // per default-side commit (`branch_unshipped_patch_count_default`'s
    // sibling cost). On a long-lived repo an old branch's merge-base can sit
    // thousands of commits behind the default branch, so that per-commit
    // fan-out was the dominant cost of both `aida awaiting --json` and `aida
    // status --full` (measured: one 2,496-commit range took 3m41s of wall
    // clock for THIS SINGLE BRANCH's landed-check, serialized N times across
    // every candidate branch in `collect_unshipped_work_items`). That is "the
    // probe's setup" the BUG-1288 review flagged — not the candidate set,
    // which stays exactly as wide as PR #1999 left it.
    //
    // Two changes, kept independent so each is auditable on its own:
    //
    // 1. Bound the walk. A default-side range wider than
    //    `MAX_SQUASH_FALLBACK_COMMITS` is too expensive to exhaust patch-id
    //    matching over, so it is skipped rather than paid for on every read.
    //    The conservative branch is `false` ("not confirmed landed") — the
    //    branch STAYS in the unshipped-work report rather than being
    //    silently hidden on unproven equivalence (PRIN-5); at worst a
    //    genuinely-landed old branch is reported once more than necessary,
    //    never the reverse.
    // 2. When under the bound, replace the N subprocess PAIRS with exactly
    //    two processes total: one `git log -p` streaming every default-side
    //    commit's diff (each preceded by its full hash, from `--format=%H`),
    //    piped into one `git patch-id --stable`, which associates each
    //    computed id with the commit-hash line that precedes it. This is the
    //    same diff text `git show --format= --binary <commit>` produced per
    //    commit — including the same "no diff" empty patch for a merge
    //    commit `git log -p` doesn't expand by default — so the match result
    //    is unchanged; only the process count drops from O(range) to O(1).
    // trace:BUG-1288 | ai:claude
    const MAX_SQUASH_FALLBACK_COMMITS: usize = 500;
    let Some(count_out) = run(&["rev-list", "--count", &default_range]) else {
        return false;
    };
    if !count_out.status.success() {
        return false;
    }
    let Ok(commit_count) = String::from_utf8_lossy(&count_out.stdout)
        .trim()
        .parse::<usize>()
    else {
        return false;
    };
    if commit_count == 0 {
        return false;
    }
    if commit_count > MAX_SQUASH_FALLBACK_COMMITS {
        return false;
    }

    let Some(log_out) = run(&["log", "--format=%H", "-p", "--binary", &default_range]) else {
        return false;
    };
    if !log_out.status.success() {
        return false;
    }
    let Some(default_side_ids) = patch_id_pairs(project_root, &log_out.stdout) else {
        return false;
    };
    default_side_ids.iter().any(|id| id == &branch_patch_id)
}

/// Per-path proof that everything `branch` changed since its merge-base with
/// `default_ref` is already on `default_ref`, byte for byte: for every path ANY
/// branch commit touched, the default branch holds the same blob id and mode
/// as the branch tip, or both lack the path. This is the
/// batched-integration complement to [`branch_content_fully_landed`]: when an
/// integration branch folds several specs' work into ONE squash commit, no
/// patch-id matches anything on the default side, yet every change the branch
/// makes is already there.
///
/// Deliberately NOT a three-way merge: `git merge-tree` honours
/// `.gitattributes` merge drivers, so a `merge=ours` path would merge
/// "cleanly" while dropping an edit that never shipped. Tree-to-tree raw diffs
/// compare object ids and modes only — no attributes, drivers or textconv.
///
/// Conservative on any doubt: a failed git call, an unparsable diff, or any
/// touched path whose content differs returns `false` (stay KEPT). A change
/// that landed and was later reverted or edited again on the default branch
/// also stays KEPT. Read-only.
// trace:BUG-1657 | ai:claude
pub(crate) fn branch_paths_match_default(
    project_root: &std::path::Path,
    default_ref: &str,
    branch: &str,
) -> bool {
    let run = |args: &[&str]| -> Option<Vec<u8>> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
    };
    let Some(merge_base) = run(&[
        "merge-base",
        git_arg_guard::END_OF_OPTIONS,
        default_ref,
        branch,
    ]) else {
        return false;
    };
    let merge_base = String::from_utf8_lossy(&merge_base).trim().to_string();
    if merge_base.is_empty() {
        return false;
    }
    let changed_paths = |from: &str, to: &str| -> Option<std::collections::HashSet<Vec<u8>>> {
        let raw = run(&[
            "diff",
            "--raw",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=none",
            git_arg_guard::END_OF_OPTIONS,
            from,
            to,
            "--",
        ])?;
        parse_raw_diff_paths(&raw)
    };
    // Every path ANY branch commit touched — the union over each commit's own
    // diff, not the net merge-base..tip diff, which would miss a path changed
    // and then changed back (a deletion or revert made after an integration
    // batch picked up an earlier tip). Merge commits are diffed against each
    // parent (`-m`), which can only widen the set.
    let range = format!("{merge_base}..{branch}");
    let Some(log_raw) = run(&[
        "log",
        "--raw",
        "-z",
        "-m",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=none",
        "--format=",
        git_arg_guard::END_OF_OPTIONS,
        &range,
        "--",
    ]) else {
        return false;
    };
    let Some(touched) = parse_raw_diff_paths(&log_raw) else {
        return false;
    };
    if touched.is_empty() {
        return false;
    }
    // Paths where the default branch's tree differs from the branch tip
    // (blob id or mode). Nothing the branch touched may appear here.
    let Some(differs) = changed_paths(default_ref, branch) else {
        return false;
    };
    touched.is_disjoint(&differs)
}

/// Parse `git diff --raw -z --no-renames` output into its set of paths. Each
/// record is a `:`-prefixed metadata field followed by one NUL-terminated
/// path. Returns `None` on any record that does not fit that shape.
// trace:BUG-1657 | ai:claude
pub(super) fn parse_raw_diff_paths(raw: &[u8]) -> Option<std::collections::HashSet<Vec<u8>>> {
    let mut paths = std::collections::HashSet::new();
    let mut fields = raw.split(|b| *b == 0).filter(|f| !f.is_empty());
    while let Some(meta) = fields.next() {
        if meta.first() != Some(&b':') {
            return None;
        }
        paths.insert(fields.next()?.to_vec());
    }
    Some(paths)
}

/// BUG-1288: batched sibling of the per-commit `patch_id` closure in
/// [`branch_content_fully_landed`] — feeds a whole `git log -p` stream (one
/// commit hash line followed by that commit's diff, repeated) through a
/// SINGLE `git patch-id --stable` process and returns every resulting patch
/// id, instead of spawning one `git patch-id` per commit. `git patch-id`
/// associates each id with the commit-hash line it saw immediately before
/// that diff, so the id/commit pairing this repository doesn't currently need
/// (only the id set is consulted) still falls out of the same single pass.
// trace:BUG-1288 | ai:claude
pub(super) fn patch_id_pairs(
    project_root: &std::path::Path,
    log_p_output: &[u8],
) -> Option<Vec<String>> {
    use std::io::Write;
    use std::process::{Command as PCmd, Stdio};

    let mut child = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = log_p_output.to_vec();
    // BUG-1288 fix-up: writing the WHOLE stream to stdin before reading any
    // stdout deadlocks once the log is large enough to fill both the stdin
    // and stdout OS pipe buffers at once (patch-id blocks writing output
    // because we haven't read it yet; we block writing input because it
    // hasn't read enough of it yet) — a real risk here, since the very point
    // of this function is to hand it a big `git log -p` stream. Write on a
    // separate thread so `wait_with_output` can drain stdout concurrently;
    // the thread exits (dropping `stdin`, closing the pipe so patch-id sees
    // EOF) whether or not the write fully succeeds.
    // trace:BUG-1288 | ai:claude
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let output = child.wait_with_output().ok()?;
    let _ = writer.join();
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    Some(
        stdout
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_string)
            .collect(),
    )
}

// trace:BUG-1719 | ai:codex
pub(crate) struct AgentWorktreeScan {
    pub(crate) findings: Vec<DoctorFinding>,
    pub(super) reclaimable_count: usize,
    /// Worktrees passed over without a `git status` because no cheap local
    /// merge signal held (`merged_only` scans only). trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
    pub(super) skipped_unmerged: usize,
    /// Worktrees left unexamined because the scan's deadline passed.
    pub(super) deferred: usize,
}

/// How much work a merged-agent-worktree scan may do. The default is the full
/// `aida doctor` scan; the post-ship GC narrows it so a ship never waits on a
/// `git status` of every registered worktree.
// trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AgentWorktreeScanLimits {
    /// Examine only worktrees whose branch shows a cheap local merge signal
    /// (ancestor of the default branch, or its spec's landing trailer on it).
    /// Every other worktree is skipped before any `git status` or forge call.
    pub(crate) merged_only: bool,
    /// Stop examining worktrees once this instant passes; the rest are
    /// counted as deferred to a full `aida doctor` run.
    pub(crate) deadline: Option<std::time::Instant>,
}

/// Time budget for the post-ship merged-worktree GC.
// trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
pub(crate) const POST_SHIP_GC_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);

// trace:BUG-1719 | ai:codex
pub(crate) fn scan_merged_agent_worktrees_with_count(
    project_root: &std::path::Path,
) -> AgentWorktreeScan {
    scan_merged_agent_worktrees_limited(project_root, AgentWorktreeScanLimits::default())
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
pub(super) fn scan_merged_agent_worktrees_limited(
    project_root: &std::path::Path,
    limits: AgentWorktreeScanLimits,
) -> AgentWorktreeScan {
    use std::process::Command as PCmd;

    let git = |args: &[&str]| -> Option<u32> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
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

    // Without a resolvable default branch we cannot corroborate merges; stay
    // silent rather than risk flagging live worktrees.
    let Some(default_ref) = resolve_default_branch_ref(project_root) else {
        return AgentWorktreeScan {
            findings: Vec::new(),
            reclaimable_count: 0,
            skipped_unmerged: 0,
            deferred: 0,
        };
    };

    let project_canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());

    let leases = list_leases(project_root);
    let mut verdicts_cache = load_worktree_verdicts(project_root);
    let mut cache_modified = false;

    let mut findings = Vec::new();
    let mut reclaimable_count = 0;
    let mut skipped_unmerged = 0;
    let mut deferred = 0;
    let worktrees = list_worktrees(project_root);
    let total = worktrees.len();
    for (index, wt) in worktrees.into_iter().enumerate() {
        // trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
        if limits
            .deadline
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
        {
            deferred = total - index;
            break;
        }
        // Missing registrations are stale bookkeeping, not actionable findings.
        // trace:BUG-1718 | ai:codex
        if !wt.path.exists() {
            continue;
        }
        let wt_canon = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
        // Never touch the main worktree or the store worktree.
        if wt_canon == project_canon || wt.branch.as_deref() == Some("aida-store") {
            continue;
        }
        if !is_agent_managed_worktree(&wt.path, wt.branch.as_deref()) {
            continue;
        }
        let Some(branch) = wt.branch.as_deref() else {
            // A detached agent worktree carries no branch to verify against
            // origin/main — keep it; the operator can inspect by hand.
            findings.push(DoctorFinding {
                category: "merged-agent-worktrees".to_string(),
                id: wt.path.display().to_string(),
                summary: format!(
                    "agent worktree {} is detached (no branch) — flagged",
                    wt.path.display()
                ),
                action: "operator decision: inspect, then `git worktree remove` by hand"
                    .to_string(),
                safe_heal: false,
            });
            continue;
        };

        let ancestor_of_main = git(&["rev-list", "--count", &format!("{default_ref}..{branch}")])
            .map(|n| n == 0)
            .unwrap_or(false);
        let unique_unmerged_commits =
            git(&["rev-list", "--count", &format!("{default_ref}..{branch}")]).unwrap_or(0);

        // Candidate spec IDs for this worktree / branch.
        let mut candidate_specs = Vec::new();
        if let Some(spec) = spec_id_from_work_branch(branch) {
            candidate_specs.push(spec);
        }
        for id in crate::pr_ship::extract_spec_ids_from_text(branch) {
            if !candidate_specs.iter().any(|s| s.eq_ignore_ascii_case(&id)) {
                candidate_specs.push(id);
            }
        }
        for lease in &leases {
            let lease_wt = lease
                .worktree_path
                .canonicalize()
                .unwrap_or_else(|_| lease.worktree_path.clone());
            if lease.branch == branch
                || (!lease.worktree_path.as_os_str().is_empty() && lease_wt == wt_canon)
            {
                let scope = lease.scope.trim().to_string();
                if !scope.is_empty()
                    && !candidate_specs
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case(&scope))
                {
                    candidate_specs.push(scope);
                }
            }
        }
        if let Some(log_raw) = PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args([
                "log",
                "--format=%B%x00",
                "-n",
                "25",
                &format!("{default_ref}..{branch}"),
            ])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        {
            for msg in log_raw.split('\0').map(str::trim).filter(|m| !m.is_empty()) {
                if is_plan_commit_subject(msg.lines().next().unwrap_or("")) {
                    continue;
                }
                for id in extract_spec_ids_from_commit(msg)
                    .into_iter()
                    .chain(crate::extract_referenced_spec_ids_from_commit(msg))
                {
                    if !candidate_specs.iter().any(|s| s.eq_ignore_ascii_case(&id)) {
                        candidate_specs.push(id);
                    }
                }
            }
        }
        if candidate_specs.is_empty() {
            if let Some(log_raw) = PCmd::new("git")
                .arg("-C")
                .arg(project_root)
                .args(["log", "--format=%B%x00", "-n", "10", branch])
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            {
                for msg in log_raw.split('\0').map(str::trim).filter(|m| !m.is_empty()) {
                    if is_plan_commit_subject(msg.lines().next().unwrap_or("")) {
                        continue;
                    }
                    for id in extract_spec_ids_from_commit(msg)
                        .into_iter()
                        .chain(crate::extract_referenced_spec_ids_from_commit(msg))
                    {
                        if !candidate_specs.iter().any(|s| s.eq_ignore_ascii_case(&id)) {
                            candidate_specs.push(id);
                        }
                    }
                }
            }
        }

        // BUG-1657 / TASK-1534: a spec landed through a batched integration PR
        // has no merge of its own branch and no PR of its own; the landing commit
        // on the default branch names it in a trailer instead. Local and cheap, so
        // it runs before (and can spare) the forge lookup.
        // trace:TASK-1534 | ai:antigravity
        // trace:BUG-1718 | ai:codex
        let find_landing = || {
            candidate_specs.iter().find_map(|spec| {
                crate::session_reap::spec_landing_commit(project_root, &default_ref, branch, spec)
            })
        };
        // A merged-only scan (the post-ship GC) settles "is this merged?" from
        // local refs first and never runs `git status` or the forge lookup on
        // a worktree without a merge signal — the unmerged majority.
        // trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
        let early_landing = if limits.merged_only && !ancestor_of_main {
            let landing = find_landing();
            if landing.is_none() {
                skipped_unmerged += 1;
                continue;
            }
            landing
        } else {
            None
        };
        let dirty = !worktree_dirty_entries(&wt.path).is_empty();
        let landing_commit = if early_landing.is_some() {
            early_landing
        } else if !ancestor_of_main && !dirty {
            find_landing()
        } else {
            None
        };
        let spec_trailer_on_main = landing_commit.is_some();
        // Only consult the forge when the cheap ancestry/trailer probes were
        // inconclusive (covers the single-PR squash-merge case) and the worktree
        // is clean — a dirty worktree is kept regardless, so skip the network call.
        let pr_merged = if !ancestor_of_main && !dirty && !spec_trailer_on_main {
            matches!(
                detect_merged_pr_for_branch_via_forge(project_root, branch),
                PrLookup::Found(_)
            )
        } else {
            false
        };

        let tip_sha_opt = PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["rev-parse", branch])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

        let cached_verdict = if !dirty {
            if let Some(ref sha) = tip_sha_opt {
                verdicts_cache.get(sha).cloned()
            } else {
                None
            }
        } else {
            None
        };

        let verdict = if let Some(v) = cached_verdict {
            v
        } else {
            let merge_signal = pr_merged || spec_trailer_on_main;

            // Only pay for the content probe when it could actually change the
            // verdict: a confirmed merged PR or spec landing trailer with a
            // positive (ancestry-only) commit count is exactly the case ancestry
            // can never clear on its own. `branch_content_fully_landed` proves
            // patch-id equivalence; `branch_paths_match_default` proves per-path
            // equivalence when batched squashes combine multiple specs into one commit.
            // trace:TASK-1534 | ai:antigravity
            let content_fully_landed = if merge_signal && unique_unmerged_commits > 0 {
                branch_content_fully_landed(project_root, &default_ref, branch)
                    || branch_paths_match_default(project_root, &default_ref, branch)
            } else {
                false
            };

            let facts = AgentWorktreeFacts {
                dirty,
                ancestor_of_main,
                pr_merged,
                unique_unmerged_commits,
                content_fully_landed,
                spec_trailer_on_main,
            };

            let calculated_verdict = classify_agent_worktree(&facts);
            // Cache if it's undecidable (which means we did the expensive work and it didn't land)
            if !dirty {
                if let AgentWorktreeVerdict::Keep {
                    actionable: false, ..
                } = &calculated_verdict
                {
                    if let Some(ref sha) = tip_sha_opt {
                        verdicts_cache.insert(sha.clone(), calculated_verdict.clone());
                        cache_modified = true;
                    }
                }
            }
            calculated_verdict
        };

        match verdict {
            AgentWorktreeVerdict::Removable(reason) => {
                reclaimable_count += 1;
                findings.push(DoctorFinding {
                    category: "merged-agent-worktrees".to_string(),
                    id: wt.path.display().to_string(),
                    summary: format!(
                        "agent worktree {} on `{branch}` is mergeable-and-gone ({reason})",
                        wt.path.display()
                    ),
                    action: format!(
                        "remove worktree + delete branch `{branch}` (`aida doctor --heal \
                         --category merged-agent-worktrees --yes --force`)"
                    ),
                    // DESTRUCTIVE (worktree removal + branch deletion) → gated
                    // behind --yes --force AND the STORY-666 autonomous-context
                    // refusal in `heal_doctor_findings`.
                    safe_heal: false,
                });
            }
            AgentWorktreeVerdict::Keep { reason, actionable } => {
                // trace:BUG-1718 | ai:codex
                // trace:BUG-1719 | ai:codex
                let action = agent_worktree_keep_action(actionable);
                findings.push(DoctorFinding {
                    category: "merged-agent-worktrees".to_string(),
                    id: wt.path.display().to_string(),
                    summary: format!(
                        "agent worktree {} on `{branch}` flagged: {reason}",
                        wt.path.display()
                    ),
                    action,
                    // Never auto-removed — flag-only.
                    safe_heal: false,
                });
            }
        }
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    if cache_modified {
        save_worktree_verdicts(project_root, &verdicts_cache);
    }
    AgentWorktreeScan {
        findings,
        reclaimable_count,
        skipped_unmerged,
        deferred,
    }
}

/// TASK-878: remove one merged agent worktree + delete its branch. Only
/// reachable when the finding surfaced it as Removable AND the caller passed
/// --yes --force under an interactive (non-autonomous) context — the STORY-666
/// destructive-heal gate in `heal_doctor_findings` fails closed otherwise. A
/// Keep-flagged worktree never routes here (its action string is operator-only).
/// Re-verifies the worktree is still clean right before removing (a salvage
/// patch is written if anything appeared since the scan) and only deletes the
// branch after the worktree is gone. trace:TASK-878 | ai:claude
pub(super) fn heal_doctor_merged_agent_worktree(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    // Keep-flagged findings carry an operator-only action and must never be
    // auto-removed, even under --yes --force.
    if !finding.action.contains("--category merged-agent-worktrees") {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some("flagged for operator decision — not auto-removed".to_string()),
        });
    }
    let worktree = std::path::PathBuf::from(&finding.id);
    if !worktree.exists() {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!("{} already gone", worktree.display())),
        });
    }

    // Resolve the worktree's branch so we can delete it after removal.
    let branch = list_worktrees(project_root)
        .into_iter()
        .find(|wt| {
            wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone())
                == worktree.canonicalize().unwrap_or_else(|_| worktree.clone())
        })
        .and_then(|wt| wt.branch);

    // Re-check: never delete uncommitted work that appeared between scan and
    // heal. Salvage anything dirty, then refuse to remove. trace:TASK-878
    if !worktree_dirty_entries(&worktree).is_empty() {
        let salvage =
            salvage_worktree_patch(project_root, "merged-agent-worktree", None, &worktree)?;
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!(
                "worktree became dirty since scan — NOT removed{}",
                salvage
                    .map(|p| format!(" (salvage patch: {})", p.display()))
                    .unwrap_or_default()
            )),
        });
    }

    // STORY-714: shared teardown — pre_destroy cargo-clean hook (TASK-0396)
    // fires before removal, and a pooled tree is deregistered from the registry.
    let removed = aida_core::worktree_pool_destroy::teardown_worktree_path(
        project_root,
        &worktree,
        &worktree_pool_global_hooks("pre_destroy"),
    );
    if let Err(e) = removed {
        // A live process inside the tree: keep it and say who holds it.
        // trace:BUG-1947 | ai:claude
        if let Some(in_use) = e.downcast_ref::<aida_core::worktree_holders::WorktreeInUse>() {
            return Ok(DoctorHealResult {
                category: finding.category.clone(),
                id: finding.id.clone(),
                action: finding.action.clone(),
                status: "skipped".to_string(),
                detail: Some(format!("worktree {} — NOT removed", in_use.summary())),
            });
        }
        // Its mail could not be retired: keep it. trace:STORY-1843 | ai:claude
        if let Some(refusal) =
            e.downcast_ref::<aida_core::mailbox_lifecycle::guard::RetentionRefusal>()
        {
            return Ok(DoctorHealResult {
                category: finding.category.clone(),
                id: finding.id.clone(),
                action: finding.action.clone(),
                status: "skipped".to_string(),
                detail: Some(crate::mail_retention::refusal_lines(refusal).join("; ")),
            });
        }
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "failed".to_string(),
            detail: Some(format!("git worktree remove {} failed", worktree.display())),
        });
    }

    // Delete the now-unused local branch (the stale-branch source of the
    // `aida human` false-positives). `-D` because a squash-merged branch isn't
    // recognized as merged by `-d`, and the scan already verified it shipped.
    let mut detail = None;
    if let Some(branch) = branch {
        // trace:BUG-2211 | ai:claude
        let deleted = aida_core::git_ops::output_healing_config_lock(
            project_root,
            || {
                let mut cmd = std::process::Command::new("git");
                cmd.arg("-C").arg(project_root);
                cmd
            },
            &["branch", "-D", &branch],
        )
        .map(|o| o.status.success())
        .unwrap_or(false);
        detail = Some(if deleted {
            format!("removed worktree + deleted branch `{branch}`")
        } else {
            format!("removed worktree (branch `{branch}` delete failed or already gone)")
        });
    }

    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "removed merged agent worktree".to_string(),
        status: "healed".to_string(),
        detail,
    })
}

/// STORY-496: reap a stale agent-registry entry. The finding id is
/// `{agent_type}#{pid}`. Re-checks registry liveness before removing — a pid can
/// be reused, and role-enter shell seats can go stale while the shell pid lives,
/// so heal must share the status predicate.
// trace:STORY-496 | ai:claude
// trace:BUG-1156 | ai:codex
pub(super) fn heal_doctor_dead_agent(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let (agent_type, pid_str) = finding.id.split_once('#').ok_or_else(|| {
        anyhow::anyhow!(
            "malformed dead-agent id `{}` (expected type#pid)",
            finding.id
        )
    })?;
    let pid: u32 = pid_str
        .parse()
        .with_context(|| format!("bad pid in dead-agent id `{}`", finding.id))?;
    if agent_registry::agent_is_live(project_root, agent_type, pid) {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!(
                "agent {agent_type}#{pid} is now live — not reaping"
            )),
        });
    }
    let removed = agent_registry::remove_agent(project_root, agent_type, pid)?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: finding.action.clone(),
        status: if removed { "healed" } else { "skipped" }.to_string(),
        detail: if removed {
            None
        } else {
            Some("registry entry already gone".to_string())
        },
    })
}

pub(super) fn heal_doctor_lease(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    // BUG-471: a lease already ended (e.g. removed earlier in this same heal
    // run, or by a concurrent session) is a no-op, not an error — mirror the
    // dead-agent "already gone" handling so heal stays idempotent.
    // trace:BUG-471 | ai:claude
    let Some(lease) = list_leases(project_root)
        .into_iter()
        .find(|lease| lease.id == finding.id)
    else {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some("lease already ended".to_string()),
        });
    };
    // A worktree a live process is using (or one the scan could not verify)
    // is skipped before the salvage step: nothing is salvaged, ended or
    // removed. trace:BUG-1947 | ai:claude
    if let Err(in_use) = aida_core::worktree_holders::check_worktree_removal(&lease.worktree_path) {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!(
                "worktree {} — lease and worktree kept",
                in_use.summary()
            )),
        });
    }
    let salvage = salvage_worktree_patch(
        project_root,
        &lease.scope,
        lease.role.as_deref(),
        &lease.worktree_path,
    )?;
    let removed = force_cleanup_lease(project_root, &lease);
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "ended lease with salvage-first cleanup".to_string(),
        status: if removed { "healed" } else { "partial" }.to_string(),
        detail: salvage.map(|p| format!("salvage patch: {}", p.display())),
    })
}

pub(crate) fn heal_doctor_orphan_worktree(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let worktree = std::path::PathBuf::from(&finding.id);
    let salvage = salvage_worktree_patch(project_root, "orphan-worktree", None, &worktree)?;
    // STORY-714: route through the shared teardown so the pre_destroy
    // cargo-clean hook fires (TASK-0396) and a pooled tree is deregistered.
    let teardown = aida_core::worktree_pool_destroy::teardown_worktree_path(
        project_root,
        &worktree,
        &worktree_pool_global_hooks("pre_destroy"),
    );
    // A live process inside the tree: keep it and say who holds it.
    // trace:BUG-1947 | ai:claude
    // A tree whose mail could not be retired is kept the same way.
    // trace:STORY-1843 | ai:claude
    let in_use = teardown.as_ref().err().and_then(|e| {
        e.downcast_ref::<aida_core::worktree_holders::WorktreeInUse>()
            .map(|u| u.summary())
            .or_else(|| {
                e.downcast_ref::<aida_core::mailbox_lifecycle::guard::RetentionRefusal>()
                    .map(|r| crate::mail_retention::refusal_lines(r).join("; "))
            })
    });
    let status = match (&teardown, &in_use) {
        (Ok(_), _) => "healed",
        (Err(_), Some(_)) => "skipped",
        (Err(_), None) => "failed",
    };
    let salvage_note = salvage.map(|p| format!("salvage patch: {}", p.display()));
    let detail = match (in_use, salvage_note) {
        (Some(u), Some(s)) => Some(format!("worktree {u} — NOT removed; {s}")),
        (Some(u), None) => Some(format!("worktree {u} — NOT removed")),
        (None, s) => s,
    };
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "removed orphan worktree".to_string(),
        status: status.to_string(),
        detail,
    })
}

pub(super) fn heal_doctor_orphan_branch(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    // trace:BUG-2211 | ai:claude
    let status = aida_core::git_ops::output_healing_config_lock(
        project_root,
        || {
            let mut cmd = std::process::Command::new("git");
            cmd.arg("-C").arg(project_root);
            cmd
        },
        &["branch", "-D", &finding.id],
    )
    .with_context(|| format!("deleting branch {}", finding.id))?
    .status;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "deleted orphan branch".to_string(),
        status: if status.success() { "healed" } else { "failed" }.to_string(),
        detail: None,
    })
}
