//! `aida doctor` command cluster (EPIC-19) — the heavy diagnostics + migration
//! command. `handle_doctor_command` dispatches the read-only multi-agent drift
//! report (`doctor_multi_agent`), the `--heal` remediation path
//! (`heal_doctor_*`), the report renderers, and the migration/repair
//! subcommands (`doctor_fsck`, `doctor_verify_relationships`,
//! `doctor_validate_trace_comments`, `doctor_repair_stale_blocks`,
//! `doctor_scrub_collisions`, `doctor_migrate_counter_scope`, `doctor_fix_sandbox`).
//!
//! Extracted verbatim from `main.rs` (SPIKE-78; pure movement, no behavior
//! change). The shared diagnostics-collection machinery — `collect_doctor_findings`,
//! `scan_completed_without_commit`, `referenced_spec_ids_on_default_branch`,
//! `which_binary`, `normalize_doctor_category`, the worktree-GC classifiers, and
//! the utility helpers (`bwrap_status_line`, `git_config_value`,
//! `humanize_duration_secs`, `salvage_worktree_patch`, `session_gc`, …) — stays
//! in `main.rs` because `aida status --full`/`--ci` (STORY-707) drive it too;
//! this module reaches those via `crate::`.

#![allow(clippy::too_many_arguments)]

use anyhow::Result;
use colored::Colorize;

use crate::*;

pub(crate) fn handle_doctor_command(
    heal: bool,
    yes: bool,
    category: Option<&str>,
    json: bool,
    force: bool,
    all: bool,
    since: Option<&str>,
    cmd: Option<&cli::DoctorCommand>,
) -> Result<()> {
    let Some(cmd) = cmd else {
        return doctor_multi_agent(DoctorRunOptions {
            heal,
            yes,
            category: category.map(str::to_string),
            json,
            force,
            all,
            since: since.map(str::to_string),
        });
    };
    match cmd {
        cli::DoctorCommand::Check {
            category,
            all: sub_all,
            json,
        } => doctor_multi_agent(DoctorRunOptions {
            heal: false,
            yes,
            category: Some(category.clone()),
            json: *json,
            force,
            all: all || *sub_all,
            since: since.map(str::to_string),
        }),
        cli::DoctorCommand::Heal {
            category,
            yes,
            force,
            all: sub_all,
            json,
        } => doctor_multi_agent(DoctorRunOptions {
            heal: true,
            yes: *yes,
            category: Some(category.clone()),
            json: *json,
            force: *force,
            all: all || *sub_all,
            since: since.map(str::to_string),
        }),
        cli::DoctorCommand::MigrateCounterScope {
            to,
            dry_run,
            yes,
            size,
        } => doctor_migrate_counter_scope(to, *dry_run, *yes, *size),
        cli::DoctorCommand::RepairStaleBlocks { dry_run, yes } => {
            doctor_repair_stale_blocks(*dry_run, *yes)
        }
        cli::DoctorCommand::ScrubCollisions => doctor_scrub_collisions(),
        cli::DoctorCommand::VerifyRelationships { repair, yes } => {
            doctor_verify_relationships(*repair, *yes)
        }
        cli::DoctorCommand::ValidateTraceComments {
            strip_dangling,
            dry_run,
            yes,
        } => doctor_validate_trace_comments(*strip_dangling, *dry_run, *yes),
        cli::DoctorCommand::Fsck => doctor_fsck(),
        cli::DoctorCommand::ConventionCheck { quiet } => doctor_convention_check(*quiet),
    }
}

// ----------------------------------------------------------------------------
// STORY-462 — `aida doctor`: multi-agent state drift diagnostics + healing.
// ----------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct DoctorRunOptions {
    heal: bool,
    yes: bool,
    category: Option<String>,
    json: bool,
    force: bool,
    all: bool,
    /// TASK-673: cutoff for the completed-without-commit integrity check —
    /// specs completed before this ref/date are exempt (legacy history).
    since: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct DoctorHealResult {
    category: String,
    id: String,
    action: String,
    status: String,
    detail: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct DoctorReport {
    total: usize,
    findings: Vec<DoctorFinding>,
    #[serde(skip_serializing_if = "is_zero_usize")]
    hidden_completed_without_commit: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    healed: Vec<DoctorHealResult>,
    /// TASK-865: read-only bubblewrap OS-sandbox availability status line.
    #[serde(skip_serializing_if = "Option::is_none")]
    bwrap: Option<String>,
}

impl DoctorReport {
    fn from_findings(findings: Vec<DoctorFinding>) -> Self {
        Self {
            total: findings.len(),
            findings,
            hidden_completed_without_commit: 0,
            healed: Vec::new(),
            bwrap: Some(bwrap_status_line()),
        }
    }
}

fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

fn doctor_multi_agent(opts: DoctorRunOptions) -> Result<()> {
    let project_root = main_worktree_root_from(&find_project_root()?);
    let store_path = project_root.join(".aida-store");
    let store = Storage::new(&store_path)
        .load()
        .with_context(|| format!("loading AIDA store at {}", store_path.display()))?;
    let mut findings = collect_doctor_findings(&project_root, &store, opts.category.as_deref())?;
    let mut hidden_completed_without_commit = 0;

    // TASK-673: the completed-without-commit integrity check runs git scans
    // (a default-branch `git log` + `git grep`) so it is kept OUT of the hot
    // `collect_doctor_findings` path (which `aida status` calls on every
    // invocation) and appended here, where only `aida doctor` reaches it.
    // It honours the same `--category` filter as the built-in categories.
    // trace:TASK-673 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "completed-without-commit")? {
        let since = opts
            .since
            .clone()
            .or_else(|| std::env::var("AIDA_DOCTOR_COMPLETED_SINCE").ok())
            .or_else(default_completed_without_commit_recent_cutoff)
            .filter(|s| !s.trim().is_empty());
        let scan = scan_completed_without_commit_with_options(
            &project_root,
            &store,
            since.as_deref(),
            opts.all,
        );
        hidden_completed_without_commit = scan.hidden_older;
        findings.extend(scan.findings);
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-717: stale-remote-branch scan. Like completed-without-commit it runs
    // git ancestry probes + a `gh pr list`, so it is kept OUT of the hot
    // `collect_doctor_findings` path and appended here where only `aida doctor`
    // reaches it. Honours the same `--category` filter. trace:TASK-717
    if doctor_category_selected(opts.category.as_deref(), "stale-remote-branches")? {
        findings.extend(scan_stale_remote_branches(&project_root, &store));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-878: merged Agent-tool worktree GC. Runs git ancestry probes + a forge
    // merged-PR lookup, so it is kept OUT of the hot `collect_doctor_findings`
    // path and appended here where only `aida doctor` reaches it. Honours the
    // same `--category` filter. trace:TASK-878 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "merged-agent-worktrees")? {
        findings.extend(scan_merged_agent_worktrees(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-1095: remote-drift scan — do the shared branches (trunk + store) hold
    // the same tip on every configured remote? Uses cheap `ls-remote` (refs
    // only, no object transfer), so it is kept OUT of the hot
    // `collect_doctor_findings` path and appended here. Honours the same
    // `--category` filter. trace:TASK-1095 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "remote-drift")? {
        findings.extend(scan_remote_drift(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-1124: rule-delivery-rot — hash the DEPLOYED vendor prompts/skills
    // (project `.claude/`+`.codex/` and the machine-global `~/.codex/prompts`)
    // against the binary's embedded source templates and flag drift. Turns the
    // invisible "stale scaffolding at the seams" failure class (a contributing
    // cause of the TASK-1123 reviewer-bypass incident) into a checkable one.
    // Detection only — the fix is the re-scaffold command, not a heal.
    // trace:TASK-1124 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "scaffold-drift")? {
        findings.extend(scan_scaffold_drift(&project_root, &store));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-1122: store-scrub — when identity redaction is configured, has the
    // raw system identity already leaked into the store (a leak that landed
    // before redaction was turned on)? Reads store files + commit authors.
    // trace:TASK-1122 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "store-scrub")? {
        findings.extend(scan_store_scrub(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    let mut report = DoctorReport::from_findings(findings);
    report.hidden_completed_without_commit = hidden_completed_without_commit;

    if opts.heal {
        report.healed = heal_doctor_findings(&project_root, &report.findings, &opts)?;
    }

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_doctor_report(&report, opts.heal)?;
        // STORY-707: `aida doctor` is the check-everything home. The heavy
        // orientation diagnostics that used to ride bare `aida status` — PR/CI
        // (a `gh` network call), live-session/lease liveness, worktree probes,
        // the full fleet roster, and cross-clone coordination — now surface
        // HERE, not on the fast default `aida status`. Gated to the full,
        // unfiltered text run (no `--category` narrow filter) so a targeted
        // `aida doctor check --category X` stays focused. trace:STORY-707
        if opts.category.is_none() {
            print_doctor_status_diagnostics(&project_root, &store);
        }
    }

    // BUG-471: heal now continues past a single finding's failure (no more
    // first-error abort), so the failure signal moves to the exit code — bail
    // non-zero after the report so scripts/automation still notice.
    // trace:BUG-471 | ai:claude
    let failed = report
        .healed
        .iter()
        .filter(|r| r.status == "failed")
        .count();
    if failed > 0 {
        anyhow::bail!("{failed} finding(s) failed to heal — see the report above");
    }
    Ok(())
}

/// TASK-1124: which deployed Codex prompts in `dir` (normally `~/.codex/prompts`)
/// have drifted from the current source templates. A prompt that EXISTS but
/// whose content differs from `expected_codex_prompts()` is rot (a stale
/// delivery — e.g. a fix that never re-scaffolded); a MISSING prompt is an
/// opt-out, not rot, so it is never flagged. Pure over `dir`, so unit-testable
/// without touching $HOME.
// trace:TASK-1124 | ai:claude
fn codex_prompts_drift(dir: &std::path::Path) -> Vec<String> {
    let mut drifted = Vec::new();
    for (name, expected) in aida_core::scaffolding::codex_prompts::expected_codex_prompts() {
        let path = dir.join(format!("{name}.md"));
        if let Ok(actual) = std::fs::read_to_string(&path) {
            if actual != expected {
                drifted.push(name);
            }
        }
    }
    drifted.sort();
    drifted
}

/// TASK-1124: rule-delivery-rot detection. Flags deployed vendor prompts/skills
/// that have drifted from the binary's embedded source templates, in two
/// places: (1) the project-local `.claude/`+`.codex/` scaffold (via
/// `check_scaffold_status`, which already ignores the dev-repo symlink layout
/// per BUG-917), and (2) the machine-global `~/.codex/prompts` (the stale-prompt
/// case that contributed to the TASK-1123 reviewer-bypass incident). Detection
/// only — each finding names the re-scaffold command, no auto-heal.
// trace:TASK-1124 | ai:claude
fn scan_scaffold_drift(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
) -> Vec<DoctorFinding> {
    let mut findings = Vec::new();

    // (1) Project-local vendor prompts/skills (Template-category, AIDA-owned).
    let config = ScaffoldConfig::default();
    let db_path = project_root.join(".aida/cache.db");
    let status = check_scaffold_status(store, project_root, &config, &db_path);
    let drifted: Vec<String> = status
        .modified
        .iter()
        .filter_map(|(p, _)| {
            let s = p.to_string_lossy();
            let is_vendor = s.starts_with(".claude/commands/")
                || s.starts_with(".claude/skills/")
                || s.starts_with(".codex/skills/");
            is_vendor.then(|| s.into_owned())
        })
        .collect();
    if !drifted.is_empty() {
        findings.push(DoctorFinding {
            category: "scaffold-drift".to_string(),
            id: "scaffold-drift/project".to_string(),
            summary: format!(
                "{} deployed vendor prompt/skill file(s) drifted from the source templates (stale scaffolding)",
                drifted.len()
            ),
            action: "aida scaffold upgrade   (or `aida scaffold diff` to inspect)".to_string(),
            safe_heal: false,
        });
    }

    // (2) Machine-global ~/.codex/prompts — the TASK-1123 incident case.
    if let Some(dir) = dirs::home_dir().map(|h| h.join(".codex").join("prompts")) {
        let drifted_prompts = codex_prompts_drift(&dir);
        if !drifted_prompts.is_empty() {
            findings.push(DoctorFinding {
                category: "scaffold-drift".to_string(),
                id: "scaffold-drift/codex-prompts".to_string(),
                summary: format!(
                    "~/.codex/prompts is stale — {} prompt(s) drifted from the current source templates",
                    drifted_prompts.len()
                ),
                action: "aida scaffold codex-prompts --force".to_string(),
                safe_heal: false,
            });
        }
    }

    findings
}

/// TASK-1122: store-scrub detection. When identity redaction is configured
/// (`[node] public_email` / `public_hostname` in `~/.aida/config.toml`), verify
/// the RAW system identity has not ALREADY leaked into the store — a leak that
/// landed before redaction was enabled is otherwise invisible on a public
/// mirror. Reads the identity-bearing store files + recent store commit authors
/// and flags any raw value present. Detection only, no auto-heal (removing an
/// already-landed value needs a history rewrite).
// trace:TASK-1122 | ai:claude
fn scan_store_scrub(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    let (pub_host, pub_email) = aida_core::git_ops::public_identity();
    // Redaction not configured → nothing is expected to be redacted, nothing to check.
    if pub_host.is_none() && pub_email.is_none() {
        return Vec::new();
    }

    // The raw identity this machine writes.
    let raw_host = hostname();
    let raw_host = (!raw_host.is_empty()).then_some(raw_host);
    let raw_email = git_config_value(project_root, "user.email");

    // Gather the store text that carries identity: the registration + block
    // files, plus recent store commit authors on the orphan branch.
    let store_dir = project_root.join(".aida-store");
    let mut store_text = String::new();
    for f in ["nodes.toml", "blocks.yaml", "oplog.yaml"] {
        if let Ok(s) = std::fs::read_to_string(store_dir.join(f)) {
            store_text.push_str(&s);
            store_text.push('\n');
        }
    }
    if let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", "aida-store", "--format=%ae%n%an", "-n", "1000"])
        .output()
    {
        if out.status.success() {
            store_text.push_str(&String::from_utf8_lossy(&out.stdout));
        }
    }

    let leaks = aida_core::git_ops::detect_identity_leaks(
        &store_text,
        raw_host.as_deref(),
        raw_email.as_deref(),
        pub_host.as_deref(),
        pub_email.as_deref(),
    );
    leaks
        .into_iter()
        .enumerate()
        .map(|(i, leak)| DoctorFinding {
            category: "store-scrub".to_string(),
            id: format!("store-scrub/{i}"),
            summary: format!("raw machine identity already in the store — {leak}"),
            action:
                "landed before redaction was enabled; scrub it from the store history before it propagates (see docs/security/)"
                    .to_string(),
            safe_heal: false,
        })
        .collect()
}

/// Whether `category` (a normalized doctor category) is in scope given the
/// user's `--category` filter. `None` filter selects everything. Errors only
// if the filter itself is an unknown category. trace:TASK-673 | ai:claude
fn doctor_category_selected(filter: Option<&str>, category: &str) -> Result<bool> {
    match filter {
        None => Ok(true),
        Some(raw) => Ok(normalize_doctor_category(raw)? == category),
    }
}

#[cfg(test)]
mod story_762_vendor_binary_tests {
    use super::*;

    // STORY-762: the codex-only doctor category resolves through the
    // normalizer with its aliases; which_binary finds a real binary and
    // returns None for a nonsense name.
    #[test]
    fn vendor_binary_category_normalizes_with_aliases() {
        for raw in ["vendor-binary", "vendor-binaries", "vendor", "VENDORS"] {
            assert_eq!(normalize_doctor_category(raw).unwrap(), "vendor-binary");
        }
    }

    #[test]
    fn which_binary_finds_sh_and_misses_nonsense() {
        // `sh` exists on every unix CI runner; a random token does not.
        #[cfg(unix)]
        assert!(which_binary("sh").is_some());
        assert!(which_binary("definitely-not-a-real-binary-xyzzy").is_none());
    }
}

/// TASK-1089: the git-canonical storage migration cutoff. Completions BEFORE
/// this date predate the reliable `(SPEC-ID)` trailer + trace convention, and
/// the bulk YAML import stamped every pre-migration spec's `created_at` at
/// import time — so flagging them as "completed without commit" is pure noise
/// (the observed 353→212 import-cohort false positives). Completions on/after it
/// are expected to carry git corroboration. A FIXED migration date, not a
/// rolling window: a window would slide forward and eventually hide genuinely
/// stranded recent completions. Override per-run with `--since` /
/// `AIDA_DOCTOR_COMPLETED_SINCE`.
// trace:TASK-1089 | ai:claude
const GIT_CANONICAL_MIGRATION_CUTOFF: &str = "2026-06-01";

fn default_completed_without_commit_recent_cutoff() -> Option<String> {
    Some(GIT_CANONICAL_MIGRATION_CUTOFF.to_string())
}

// ============================================================================
// TASK-717 — `aida doctor`: verify-and-prune stale REMOTE branches.
//
// `aida doctor --heal --category orphan-branches` only deletes LOCAL orphan
// branches; stale `origin/*` branches accumulate and must be pruned by hand.
// This adds a `stale-remote-branches` category that surfaces remote branches
// and verify-and-prunes them with a squash-aware safety model:
//
//   safe-to-delete iff ANY of:
//     - the branch's spec is Completed/Rejected, OR
//     - the branch HEAD is an ancestor of origin/main (already merged), OR
//     - origin/main carries a commit referencing the spec `(SPEC-ID)`
//   AND none of the EXCLUDE conditions apply:
//     - the branch is a protected ref (main/master/aida-store), OR
//     - the branch has an open PR
//   AND the branch has NO genuinely-unique unmerged commits (those are KEPT
//   and flagged for the operator, never deleted).
//
// Dry-run by default (read-only list + per-branch verdict/reason); the heal
// is gated behind --yes --force (destructive remote deletion). trace:TASK-717
// ============================================================================

// The classification verdict for one remote branch. trace:TASK-717
#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteBranchVerdict {
    /// Protected or open-PR — excluded from any consideration.
    Excluded(String),
    /// Verified merged/terminal AND no unique unmerged commits → safe to prune.
    SafeToDelete(String),
    /// Has genuinely-unique unmerged commits → keep, flag for the operator.
    Keep(String),
}

/// Inputs to the pure remote-branch classifier — every git/gh probe result the
/// safety model needs, gathered once per branch by the scanner. Keeping the
/// classification pure makes the squash-aware safety model unit-testable
// without git or gh. trace:TASK-717
#[derive(Debug, Clone)]
struct RemoteBranchFacts {
    /// True when the branch is a protected ref (main/master/aida-store).
    protected: bool,
    /// True when an open PR currently has this branch as its head.
    has_open_pr: bool,
    /// True when the branch HEAD is an ancestor of origin/main (fully merged —
    /// covers fast-forward / non-squash merges).
    ancestor_of_main: bool,
    /// True when origin/main carries a commit whose subject/trailer references
    /// the branch's spec id (covers squash merges, where the branch tip keeps a
    /// different SHA but the work shipped).
    spec_referenced_on_main: bool,
    /// The branch's spec status, when the branch name maps to a known spec and
    /// that spec is Completed or Rejected (terminal). None otherwise.
    spec_terminal: bool,
    /// Count of commits on the branch not reachable from origin/main. Zero means
    /// nothing unique is at risk; non-zero with no other "merged" signal means
    /// the branch carries unique unmerged work that must be KEPT.
    unique_unmerged_commits: u32,
}

/// Pure squash-aware classification of one remote branch. No git/gh — every
/// input is pre-gathered in `RemoteBranchFacts`. This is the safety model:
/// excludes protected + open-PR branches first; declares safe-to-delete only on
/// a positive "merged/terminal" signal AND zero unique unmerged commits;
// otherwise keeps the branch and flags it. trace:TASK-717
fn classify_stale_remote_branch(facts: &RemoteBranchFacts) -> RemoteBranchVerdict {
    // EXCLUDE first — protected and open-PR branches are never candidates.
    if facts.protected {
        return RemoteBranchVerdict::Excluded("protected ref".to_string());
    }
    if facts.has_open_pr {
        return RemoteBranchVerdict::Excluded("has an open PR".to_string());
    }

    // A positive "this work has shipped / is terminal" signal — any one suffices.
    let merged_reason = if facts.ancestor_of_main {
        Some("HEAD is an ancestor of origin/main (merged)")
    } else if facts.spec_referenced_on_main {
        Some("origin/main carries a commit referencing its spec (squash-merged)")
    } else if facts.spec_terminal {
        Some("its spec is Completed/Rejected")
    } else {
        None
    };

    let Some(merged_reason) = merged_reason else {
        // No merged/terminal signal at all → never delete; flag for the operator.
        return RemoteBranchVerdict::Keep(
            "no merge/terminal signal — operator decision".to_string(),
        );
    };

    // Even with a merged signal, a branch carrying genuinely-unique unmerged
    // commits (e.g. an unmerged migration-guide on a squash-merged branch) must
    // be KEPT — deleting it would lose work. Ancestor-of-main implies zero unique
    // commits, so this only bites the squash/terminal paths.
    if facts.unique_unmerged_commits > 0 && !facts.ancestor_of_main {
        return RemoteBranchVerdict::Keep(format!(
            "{merged_reason}, but {} unique unmerged commit(s) — keep, operator decision",
            facts.unique_unmerged_commits
        ));
    }

    RemoteBranchVerdict::SafeToDelete(merged_reason.to_string())
}

/// Derive the candidate spec id from a work-branch name (`task-281-foo` →
/// `TASK-281`). Returns None when the branch doesn't follow the work-branch
// convention. trace:TASK-717
fn spec_id_from_work_branch(branch: &str) -> Option<String> {
    if !is_work_spec_branch_name(branch) {
        return None;
    }
    // `<type>-<digits>[-suffix...]` → keep `<type>-<digits>`.
    let mut parts = branch.splitn(3, '-');
    let kind = parts.next()?;
    let num = parts.next()?;
    if kind.is_empty() || num.is_empty() || !num.chars().next()?.is_ascii_digit() {
        return None;
    }
    // The number segment can carry a trailing `.suffix` (e.g. `epic-20.batch7`);
    // trim at the first non-digit so we land on the bare spec id.
    let digits: String = num.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    Some(format!("{}-{}", kind.to_ascii_uppercase(), digits))
}

/// Do the shared branches (code trunk + orphan store) hold the same tip on
/// every configured remote? A mismatch means the substrate has forked across
/// hubs (e.g. a clone pushed the store to gitlab but not github). Uses cheap
/// `ls-remote` (refs only, no object transfer) for current truth; a remote we
/// can't reach is skipped (not a false alarm). Never writes. Emits one finding
/// per diverged branch.
// trace:TASK-1095 | ai:claude
fn scan_remote_drift(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    let remotes: Vec<String> = aida_core::git_ops::list_remotes(project_root)
        .into_iter()
        .filter(|r| r != "all")
        .collect();
    if remotes.len() < 2 {
        return Vec::new();
    }

    let mut findings = Vec::new();
    for branch in ["main", "aida-store"] {
        // (remote, short-sha) for every remote that currently has the branch.
        let tips: Vec<(String, String)> = remotes
            .iter()
            .filter_map(|r| {
                aida_core::git_ops::remote_branch_head_sha(project_root, r, branch)
                    .map(|sha| (r.clone(), sha.chars().take(12).collect::<String>()))
            })
            .collect();
        if tips.len() < 2 {
            continue; // can't compare (offline, or only one hub has it)
        }
        let distinct: std::collections::BTreeSet<&String> = tips.iter().map(|(_, s)| s).collect();
        if distinct.len() > 1 {
            let detail = tips
                .iter()
                .map(|(r, s)| format!("{r}={s}"))
                .collect::<Vec<_>>()
                .join(" ");
            findings.push(DoctorFinding {
                category: "remote-drift".to_string(),
                id: format!("remote-drift-{branch}"),
                summary: format!("branch `{branch}` differs across remotes: {detail}"),
                action: if branch == "aida-store" {
                    "run `aida remote reconcile` (dry-run; --execute to union-merge and push every hub); \
                     never force-push a shared branch to resolve"
                        .to_string()
                } else {
                    "reconcile the divergent tips and push to every remote (see `aida remote status`); \
                     never force-push a shared branch to resolve"
                        .to_string()
                },
                safe_heal: false,
            });
        }
    }
    findings
}

/// TASK-717: scan stale `origin/*` branches and classify each under the
/// squash-aware safety model. Read-only — performs git ancestry/rev-list probes
/// and one `gh pr list` for the open-PR set, never mutates anything. Returns a
/// `stale-remote-branches` finding for every branch that is either SafeToDelete
/// (verdict carries the merge reason; `safe_heal=false` so deletion stays gated
/// behind --yes --force) or Keep (flagged for the operator, never auto-healed).
// Excluded branches produce no finding. trace:TASK-717
fn scan_stale_remote_branches(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
) -> Vec<DoctorFinding> {
    use std::process::Command as PCmd;

    let git = |args: &[&str]| -> Option<String> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };

    // No resolvable default branch → we cannot corroborate merges, so stay
    // silent rather than risk flagging every remote branch.
    let Some(default_ref) = resolve_default_branch_ref(project_root) else {
        return Vec::new();
    };

    // Spec ids referenced by commits on origin/main (squash-merge signal).
    let mut referenced_on_main: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    if let Some(log) = git(&["log", "--pretty=format:%s", &default_ref]) {
        for subject in log.lines() {
            let subject = subject.trim();
            if subject.is_empty() || is_plan_commit_subject(subject) {
                continue;
            }
            for id in extract_spec_ids_from_commit(subject) {
                referenced_on_main.insert(id.to_ascii_uppercase());
            }
        }
    }

    let open_pr_branches = collect_open_prs(project_root).by_branch;
    const PROTECTED: &[&str] = &["main", "master", "aida-store", "HEAD"];

    let mut findings = Vec::new();
    for rc in collect_remote_branch_commits(project_root) {
        let branch = rc.branch;
        let protected = PROTECTED.iter().any(|p| p.eq_ignore_ascii_case(&branch));
        let has_open_pr = open_pr_branches.contains_key(&branch);

        // Ancestor-of-main check: empty `origin/main..origin/branch` rev-list.
        let ancestor_of_main = git(&[
            "rev-list",
            "--count",
            &format!("{default_ref}..origin/{branch}"),
        ])
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|n| n == 0)
        .unwrap_or(false);

        // unique_unmerged_commits == ancestor-count above; reuse it.
        let unique_unmerged_commits = git(&[
            "rev-list",
            "--count",
            &format!("{default_ref}..origin/{branch}"),
        ])
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

        let spec_id = spec_id_from_work_branch(&branch);
        let spec_referenced_on_main = spec_id
            .as_deref()
            .map(|id| referenced_on_main.contains(&id.to_ascii_uppercase()))
            .unwrap_or(false);
        let spec_terminal = spec_id
            .as_deref()
            .and_then(|id| store.get_requirement_by_spec_id(id))
            .map(|req| {
                matches!(
                    req.status,
                    RequirementStatus::Completed | RequirementStatus::Rejected
                )
            })
            .unwrap_or(false);

        let facts = RemoteBranchFacts {
            protected,
            has_open_pr,
            ancestor_of_main,
            spec_referenced_on_main,
            spec_terminal,
            unique_unmerged_commits,
        };

        match classify_stale_remote_branch(&facts) {
            RemoteBranchVerdict::Excluded(_) => {
                // Protected / open-PR — never surfaced as a finding.
            }
            RemoteBranchVerdict::SafeToDelete(reason) => {
                findings.push(DoctorFinding {
                    category: "stale-remote-branches".to_string(),
                    id: branch.clone(),
                    summary: format!("remote branch `origin/{branch}` is stale ({reason})"),
                    action: format!(
                        "delete remote branch (`aida doctor --heal --category \
                         stale-remote-branches --yes --force`, or `git push origin \
                         --delete {branch}`)"
                    ),
                    // Remote deletion is destructive — gate behind --yes --force.
                    safe_heal: false,
                });
            }
            RemoteBranchVerdict::Keep(reason) => {
                findings.push(DoctorFinding {
                    category: "stale-remote-branches".to_string(),
                    id: branch.clone(),
                    summary: format!("remote branch `origin/{branch}` flagged: {reason}"),
                    action: "operator decision: review and keep, open a PR, or delete by hand"
                        .to_string(),
                    // Never auto-deleted — flag-only.
                    safe_heal: false,
                });
            }
        }
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    findings
}

/// TASK-717: delete one stale remote branch via `git push origin --delete`.
/// Only reachable when the finding's action surfaced it as SafeToDelete and the
/// caller passed --yes --force (see `heal_doctor_finding`). A Keep-flagged
// branch never routes here — its action string is operator-only. trace:TASK-717
fn heal_doctor_stale_remote_branch(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    // Keep-flagged findings carry an operator-only action and must never be
    // auto-deleted, even under --yes --force.
    if !finding.action.contains("--category stale-remote-branches") {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some("flagged for operator decision — not auto-deleted".to_string()),
        });
    }
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["push", "origin", "--delete", &finding.id])
        .status()
        .with_context(|| format!("deleting remote branch origin/{}", finding.id))?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "deleted stale remote branch".to_string(),
        status: if status.success() { "healed" } else { "failed" }.to_string(),
        detail: None,
    })
}

// The classification verdict for one agent-managed worktree. trace:TASK-878
#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentWorktreeVerdict {
    /// Verified merged AND clean AND no unique unmerged commits → safe to GC.
    Removable(String),
    /// Dirty, carrying unmerged work, or no merge signal → keep, flag operator.
    Keep(String),
}

/// Inputs to the pure agent-worktree classifier — every git/forge probe result
/// the safety model needs, gathered once per worktree by the scanner. Keeping
/// the classification pure makes the squash-aware safety model unit-testable
// without git or a forge. trace:TASK-878
#[derive(Debug, Clone)]
struct AgentWorktreeFacts {
    /// True when the worktree has uncommitted changes (tracked or staged dirt,
    /// or untracked files). A dirty worktree is NEVER removed — clean != no work,
    /// and dirty is unambiguously "has work".
    dirty: bool,
    /// True when the branch HEAD is an ancestor of origin/main (fully merged —
    /// covers fast-forward / non-squash merges; implies zero unique commits).
    ancestor_of_main: bool,
    /// True when the forge reports a MERGED PR for this branch (covers squash
    /// merges, where the branch tip keeps a different SHA but the work shipped).
    pr_merged: bool,
    /// Count of commits on the branch not reachable from origin/main. Zero means
    /// nothing unique is at risk; non-zero with no ancestor signal means the
    /// branch carries unique unmerged work that must be KEPT.
    unique_unmerged_commits: u32,
}

/// Pure squash-aware classification of one agent-managed worktree. No git/forge
/// — every input is pre-gathered in `AgentWorktreeFacts`. This is the safety
/// model: a dirty worktree is always kept; removal needs a positive merged
/// signal AND zero unique unmerged commits; otherwise the worktree is kept and
// flagged. trace:TASK-878 | ai:claude
fn classify_agent_worktree(facts: &AgentWorktreeFacts) -> AgentWorktreeVerdict {
    // KEEP first — uncommitted work is unambiguously "has work". Clean != no
    // work, but dirty is definitely work; never delete it.
    if facts.dirty {
        return AgentWorktreeVerdict::Keep(
            "uncommitted changes present — never auto-removed".to_string(),
        );
    }

    // A positive "this work has shipped" signal — either suffices.
    let merged_reason = if facts.ancestor_of_main {
        Some("branch is an ancestor of origin/main (merged)")
    } else if facts.pr_merged {
        Some("its PR is merged (squash-merged)")
    } else {
        None
    };

    let Some(merged_reason) = merged_reason else {
        // No merged signal at all → never remove; flag for the operator.
        return AgentWorktreeVerdict::Keep("no merge signal — operator decision".to_string());
    };

    // Even with a merged signal, a branch carrying genuinely-unique unmerged
    // commits (e.g. extra commits added after the PR squash-merged) must be KEPT
    // — removing it would lose work. Ancestor-of-main implies zero unique
    // commits, so this only bites the squash-merge path.
    if facts.unique_unmerged_commits > 0 && !facts.ancestor_of_main {
        return AgentWorktreeVerdict::Keep(format!(
            "{merged_reason}, but {} unique unmerged commit(s) — keep, operator decision",
            facts.unique_unmerged_commits
        ));
    }

    AgentWorktreeVerdict::Removable(merged_reason.to_string())
}

/// TASK-878: scan AIDA/Agent-tool managed worktrees and classify each under the
/// squash-aware safety model. Read-only — performs git ancestry/rev-list probes
/// and one forge merged-PR lookup per branch, never mutates anything. Returns a
/// `merged-agent-worktrees` finding for every agent worktree that is either
/// Removable (verified merged + clean + no unique commits; `safe_heal=false` so
/// removal stays gated behind --yes --force + the STORY-666 sign-off) or Keep
/// (flagged for the operator, never auto-removed). The project's own worktree
// and the `aida-store` worktree are skipped. trace:TASK-878
fn scan_merged_agent_worktrees(project_root: &std::path::Path) -> Vec<DoctorFinding> {
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
        return Vec::new();
    };

    let project_canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());

    let mut findings = Vec::new();
    for wt in list_worktrees(project_root) {
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

        let dirty = !worktree_dirty_entries(&wt.path).is_empty();
        let ancestor_of_main = git(&["rev-list", "--count", &format!("{default_ref}..{branch}")])
            .map(|n| n == 0)
            .unwrap_or(false);
        let unique_unmerged_commits =
            git(&["rev-list", "--count", &format!("{default_ref}..{branch}")]).unwrap_or(0);
        // Only consult the forge when the cheap ancestry probe was inconclusive
        // (covers the squash-merge case) and the worktree is clean — a dirty
        // worktree is kept regardless, so skip the network call.
        let pr_merged = if !ancestor_of_main && !dirty {
            matches!(
                detect_merged_pr_for_branch_via_forge(project_root, branch),
                PrLookup::Found(_)
            )
        } else {
            false
        };

        let facts = AgentWorktreeFacts {
            dirty,
            ancestor_of_main,
            pr_merged,
            unique_unmerged_commits,
        };

        match classify_agent_worktree(&facts) {
            AgentWorktreeVerdict::Removable(reason) => {
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
            AgentWorktreeVerdict::Keep(reason) => {
                findings.push(DoctorFinding {
                    category: "merged-agent-worktrees".to_string(),
                    id: wt.path.display().to_string(),
                    summary: format!(
                        "agent worktree {} on `{branch}` flagged: {reason}",
                        wt.path.display()
                    ),
                    action: "operator decision: review and keep, or remove by hand".to_string(),
                    // Never auto-removed — flag-only.
                    safe_heal: false,
                });
            }
        }
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    findings
}

/// TASK-878: remove one merged agent worktree + delete its branch. Only
/// reachable when the finding surfaced it as Removable AND the caller passed
/// --yes --force under an interactive (non-autonomous) context — the STORY-666
/// destructive-heal gate in `heal_doctor_findings` fails closed otherwise. A
/// Keep-flagged worktree never routes here (its action string is operator-only).
/// Re-verifies the worktree is still clean right before removing (a salvage
/// patch is written if anything appeared since the scan) and only deletes the
// branch after the worktree is gone. trace:TASK-878 | ai:claude
fn heal_doctor_merged_agent_worktree(
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
    if removed.is_err() {
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
        let deleted = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["branch", "-D", &branch])
            .status()
            .map(|s| s.success())
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

/// The heavy orientation diagnostics moved off bare `aida status`
/// and onto `aida doctor`. Prints the PR/CI status (a `gh` network call),
/// live-session/lease liveness + fleet roster, the Claude Code presence line,
/// the worktree pane, the open-PR roster, and cross-clone coordination claims —
/// the same section printers `aida status --full` uses. Constructs the cached
/// backend on demand (the doctor's own load goes through the legacy `Storage`
/// path) so `collect_user_context` has the cache-backed handle it needs. Each
/// section graceful-degrades on its own data; best-effort — a backend that can't
/// be opened simply prints nothing here rather than failing the doctor run.
// trace:STORY-707 | ai:claude
fn print_doctor_status_diagnostics(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
) {
    let store_path = project_root.join(".aida-store");
    let Ok(dispenser) = load_dispenser(&store_path) else {
        return;
    };
    let Ok(inner) = aida_core::GitBackend::new(&store_path).map(|b| b.with_dispenser(dispenser))
    else {
        return;
    };
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let Ok(backend) = aida_core::CachedGitBackend::with_inner(inner, &cache_path) else {
        return;
    };

    println!();
    println!("{}", "─── Status diagnostics ───".bold());
    println!(
        "  {}",
        "(moved off `aida status` so the default snapshot stays instant — STORY-707)".dimmed()
    );
    println!();

    // The full user-context gather: PR/CI (gh), branch facts, queue snapshot,
    // and the live-session-probed agent roster. This is the ~16s the fast
    // `aida status` no longer pays — it lives here now.
    let user_ctx = collect_user_context(project_root, store, &backend, false);

    print_status_pr_section(&user_ctx, false);
    print_status_queue_section(&user_ctx, false);
    print_status_presence_line(project_root);
    print_status_agents_section(&user_ctx, true);
    print_status_claude_code_section(project_root);
    print_status_worktrees_section(project_root, true);
    print_status_open_prs_section(project_root, true);
    print_status_coordination_section(&store_path, chrono::Utc::now(), true);
}

fn render_doctor_report(report: &DoctorReport, healed: bool) -> Result<()> {
    println!("{}", "─── AIDA doctor ───".bold());
    if report.findings.is_empty() && report.hidden_completed_without_commit == 0 {
        println!(
            "  {} no multi-agent state drift detected",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        let mut current = "";
        for finding in &report.findings {
            if current != finding.category {
                current = &finding.category;
                println!();
                println!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::Bullet).cyan(),
                    current.bold()
                );
            }
            let safety = if finding.safe_heal { "safe" } else { "manual" };
            println!("  - {} [{}]", finding.summary, safety.dimmed());
            println!("    → {}", finding.action.dimmed());
        }
        if report.hidden_completed_without_commit > 0 {
            if current != "completed-without-commit" {
                println!();
                println!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::Bullet).cyan(),
                    "completed-without-commit".bold()
                );
            }
            println!(
                "  ({} older completed-without-commit finding{} hidden — pass --all to list)",
                report.hidden_completed_without_commit,
                if report.hidden_completed_without_commit == 1 {
                    ""
                } else {
                    "s"
                }
            );
        }
        println!();
        println!(
            "  Total: {} visible finding{}",
            report.findings.len(),
            if report.findings.len() == 1 { "" } else { "s" }
        );
    }

    if healed {
        println!();
        println!("{}", "─── Heal results ───".bold());
        if report.healed.is_empty() {
            println!("  (no heal actions applied)");
        }
        for result in &report.healed {
            println!(
                "  - {} {}: {}",
                result.status.as_str().bold(),
                result.id,
                result.action
            );
            if let Some(detail) = &result.detail {
                println!("    {}", detail.dimmed());
            }
        }
    } else if !report.findings.is_empty() {
        println!("  Re-run with {} to apply safe fixes.", "--heal".cyan());
    }

    // TASK-865: report read-only environment facts the operator wants when
    // triaging launch confinement — currently the bubblewrap OS-sandbox status.
    // Availability only; this never enables `os_wrap`. trace:TASK-865 | ai:claude
    println!();
    println!("{}", "─── Environment ───".bold());
    render_doctor_bwrap_row();
    render_doctor_forge_row();
    Ok(())
}

/// Render the forge-CLI availability row in `aida doctor`'s environment section.
/// `aida pr auto-queue-review` (and the rest of the PR/CI lifecycle) fails hard
/// when the project's forge CLI (`gh` for GitHub, `glab` for GitLab) is not on
/// PATH, so surface it here — OK / missing-with-install-hint / none-needed
/// (pure-git) — rather than letting it stay silent until the first `pr` command.
// Colourised to match the doctor-check output style. trace:TASK-860 | ai:claude
fn render_doctor_forge_row() {
    let project_root = match find_project_root() {
        Ok(root) => main_worktree_root_from(&root),
        // No project context (e.g. run outside a repo) — fall back to CWD so the
        // row still reports something useful rather than panicking.
        Err(_) => std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    };
    let (kind, msg) = crate::forge::forge_cli_status(&project_root);
    let glyph = if kind == crate::forge::ForgeKind::None {
        // Pure-git needs no forge CLI — informational, not a warning.
        crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
    } else if kind.cli_on_path() {
        crate::glyph(crate::glyphs::Glyph::Check).green()
    } else {
        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
    };
    println!("  {} {}", glyph, msg);
}

/// Render the bwrap availability row in `aida doctor`'s environment section,
/// colourised to match the doctor-check output style. When confinement is
/// blocked or bwrap is missing, print the EXACT copy-pasteable remediation
/// (not just a one-line prose hint) plus a pointer at the guided setup command;
// when it's ready, confirm how to opt in. trace:TASK-865 | ai:claude
// trace:STORY-665 | ai:claude
fn render_doctor_bwrap_row() {
    let avail = crate::session::bwrap_availability();
    let glyph = match avail {
        crate::session::BwrapAvailability::Ok => crate::glyph(crate::glyphs::Glyph::Check).green(),
        crate::session::BwrapAvailability::NotInstalled => {
            crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
        }
        crate::session::BwrapAvailability::UsernsBlocked { .. } => {
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        }
    };
    println!("  {} {}", glyph, bwrap_status_line());
    match avail {
        crate::session::BwrapAvailability::Ok => {
            // Ready — confirm + point at how to opt in (it's off by default).
            println!(
                "    {}",
                "OS sandbox is ready. Opt in per-host with `export AIDA_OS_WRAP=1` (recommended — \
                 no shared-config change), or repo-wide with [contained] os_wrap = true in .aida/config.toml."
                    .dimmed()
            );
        }
        crate::session::BwrapAvailability::NotInstalled => {
            // Exact install command, then the guided-setup pointer.
            println!("    {}", "Install bubblewrap, then enable:".dimmed());
            println!("      {}", crate::session::BWRAP_INSTALL_DEBIAN.cyan());
            println!(
                "    {}",
                "Then run `aida doctor --fix-sandbox` for the full setup steps.".dimmed()
            );
        }
        crate::session::BwrapAvailability::UsernsBlocked { .. } => {
            // The kernel blocks unprivileged userns — print the EXACT runtime +
            // persist sysctl commands, clearly marked as sudo. trace:STORY-665
            println!(
                "    {}",
                "Kernel blocks unprivileged user namespaces. Fix (run these yourself):".dimmed()
            );
            println!(
                "      {}  {}",
                crate::session::BWRAP_USERNS_SYSCTL_RUNTIME.cyan(),
                "# this boot".dimmed()
            );
            println!(
                "      {}  {}",
                crate::session::BWRAP_USERNS_SYSCTL_PERSIST.cyan(),
                "# persist".dimmed()
            );
            println!(
                "    {}",
                "Then run `aida doctor --fix-sandbox` for the full setup + verify steps.".dimmed()
            );
        }
    }
}

/// `aida doctor --fix-sandbox` — guided, copy-pasteable bring-up of the OS
/// sandbox (bubblewrap write-confinement) on the current host. A PRINTER, not a
/// silent sudo-runner: it detects the current state, prints the exact ordered
/// sequence (install / persist-sysctl / opt-in / verify) with sudo steps marked
/// "run this yourself", and runs the NON-sudo availability re-probe as a smoke
/// check. Honest + safe — it never escalates privileges on the user's behalf.
// trace:STORY-665 | ai:claude
pub(crate) fn doctor_fix_sandbox() -> Result<()> {
    use crate::session::{
        bwrap_availability, BwrapAvailability, BWRAP_INSTALL_DEBIAN, BWRAP_USERNS_SYSCTL_PERSIST,
        BWRAP_USERNS_SYSCTL_RUNTIME,
    };

    println!(
        "{}",
        "Guided OS-sandbox setup (bubblewrap write-confinement)"
            .bold()
            .cyan()
    );
    println!(
        "{}",
        "AIDA can run the agent it launches under an OS-level write-confinement \n\
         sandbox (bwrap). It is opt-in and off by default. This walks you through \n\
         bringing it up on THIS host. sudo steps are yours to run."
            .dimmed()
    );
    println!();

    let avail = bwrap_availability();

    // Step 1 — detected state.
    println!("{}", "1. Detected state".bold());
    match &avail {
        BwrapAvailability::Ok => println!(
            "   {} bwrap is installed and the unprivileged-userns self-test passes.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        ),
        BwrapAvailability::NotInstalled => println!(
            "   {} bwrap is not installed on this host.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
        BwrapAvailability::UsernsBlocked { .. } => println!(
            "   {} bwrap is installed, but the kernel is blocking the unprivileged \n      user namespace it needs (Ubuntu 23.10+/24.04 AppArmor default).",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
    }
    println!();

    let mut step = 2;

    // Step 2 — install (only if missing).
    if avail == BwrapAvailability::NotInstalled {
        println!("{}", format!("{step}. Install bubblewrap").bold());
        println!("   {}", "Run this yourself (sudo):".dimmed());
        println!("      {}", BWRAP_INSTALL_DEBIAN.cyan());
        println!(
            "   {}",
            "(non-Debian: use your package manager's `bubblewrap` package)".dimmed()
        );
        println!();
        step += 1;
    }

    // Step (conditional) — lift + persist the userns sysctl.
    if matches!(avail, BwrapAvailability::UsernsBlocked { .. })
        || avail == BwrapAvailability::NotInstalled
    {
        println!(
            "{}",
            format!("{step}. Permit unprivileged user namespaces").bold()
        );
        println!(
            "   {}",
            "Run these yourself (sudo). First applies it now, second persists across reboots:"
                .dimmed()
        );
        println!(
            "      {}  {}",
            BWRAP_USERNS_SYSCTL_RUNTIME.cyan(),
            "# this boot".dimmed()
        );
        println!(
            "      {}  {}",
            BWRAP_USERNS_SYSCTL_PERSIST.cyan(),
            "# persist".dimmed()
        );
        println!();
        step += 1;
    }

    // Step — opt in (the knob is off by default regardless of host state).
    println!("{}", format!("{step}. Enable the sandbox (opt-in)").bold());
    println!(
        "   {}",
        "Set the knob in .aida/config.toml (off by default):".dimmed()
    );
    println!("      {}", "[contained]".cyan());
    println!("      {}", "os_wrap = true".cyan());
    println!(
        "   {}",
        "Recommended: enable per-host with `export AIDA_OS_WRAP=1` (no shared-config change). The config knob above enables it repo-wide.".dimmed()
    );
    println!();
    step += 1;

    // Step — verify.
    println!("{}", format!("{step}. Verify").bold());
    println!(
        "   {}",
        "Re-run this command (or `aida doctor`) — step 1 should report a passing self-test:"
            .dimmed()
    );
    println!("      {}", "aida doctor --fix-sandbox".cyan());
    println!(
        "   {}",
        "Once enabled, `aida config show` renders the resolved [contained] posture.".dimmed()
    );
    println!();

    // Non-sudo smoke: re-probe availability and report the live verdict so the
    // operator sees whether the host is already there without touching sudo.
    println!("{}", "Live smoke check (non-sudo re-probe)".bold());
    match bwrap_availability() {
        BwrapAvailability::Ok => println!(
            "   {} Confinement self-test PASSES — this host is ready; just set os_wrap = true.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        ),
        BwrapAvailability::NotInstalled => println!(
            "   {} bwrap still not on PATH — run the install step above.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
        BwrapAvailability::UsernsBlocked { .. } => println!(
            "   {} Self-test still failing — run the sudo sysctl step above, then re-check.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
    }
    println!();
    println!(
        "   {}",
        "Full reference: docs/agents/claude-bubblewrap-sandbox.md".dimmed()
    );

    Ok(())
}

/// STORY-666: is THIS process an autonomous / unattended context for the
/// purposes of the destructive-heal gate? Pure so it is unit-testable without
/// touching the real terminal or environment — `doctor_running_autonomously`
/// (below) feeds it the live signals.
///
/// **Autonomous = no interactive TTY OR a corroborated live orchestrator run.**
///
/// Deliberately NOT keyed on `--yes` alone: a human at a keyboard typing
/// `aida doctor --heal --force --yes` IS the explicit sign-off, and that
/// interactive path must stay exactly as it is today (STORY-666 req #3). The
/// `--yes` flag only ever *reaches* a destructive heal together with `--force`,
/// and at a TTY that combination is a deliberate human decision; what we must
/// fail-closed against is the genuinely unattended case — piped/CI stdin (no
// TTY) or an `--auto-complete` drain (orchestrator token live). trace:STORY-666
fn doctor_context_is_autonomous(stdin_is_tty: bool, orchestrated: bool) -> bool {
    !stdin_is_tty || orchestrated
}

/// STORY-666: live-signal wrapper for [`doctor_context_is_autonomous`] — reads
/// the real TTY state + the corroborated orchestrator verdict for `project_root`.
// trace:STORY-666 | ai:claude
fn doctor_running_autonomously(project_root: &std::path::Path) -> bool {
    let orchestrated = orchestrator::detect(project_root).is_orchestrated();
    doctor_context_is_autonomous(std::io::stdin().is_terminal(), orchestrated)
}

/// STORY-666: the heal disposition for one finding-category, decided by its
/// safe/destructive classification, the requested flags, and whether we are in
/// an autonomous context. Pure → unit-testable. The single place the
// fail-closed invariant lives. trace:STORY-666 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealDisposition {
    /// Apply the category's heals (safe always; destructive only with explicit
    /// interactive sign-off).
    Proceed,
    /// A destructive category was requested via `--force --yes` but we are in a
    /// non-interactive/autonomous context — refuse and report (fail-closed).
    GateAutonomous,
    /// A destructive category WITHOUT the `--force --yes` opt-in — the
    /// pre-existing "needs a manual decision" skip (unchanged).
    SkipNeedsForce,
}

fn doctor_heal_disposition(
    safe: bool,
    force: bool,
    yes: bool,
    autonomous: bool,
) -> HealDisposition {
    if safe {
        // Safe, reversible heals proceed in every context — the safe
        // classification IS the bouncer. Never over-gate routine fixes.
        return HealDisposition::Proceed;
    }
    // Destructive from here down.
    if !(force && yes) {
        return HealDisposition::SkipNeedsForce;
    }
    if autonomous {
        // Fail-closed: a destructive heal must not execute silently with no one
        // to make the check-before-delete judgment.
        return HealDisposition::GateAutonomous;
    }
    // Destructive + --force --yes + interactive TTY = explicit human sign-off.
    HealDisposition::Proceed
}

fn heal_doctor_findings(
    project_root: &std::path::Path,
    findings: &[DoctorFinding],
    opts: &DoctorRunOptions,
) -> Result<Vec<DoctorHealResult>> {
    // STORY-666: detect the autonomous/unattended context ONCE for the whole
    // run — the destructive-heal gate keys off it. trace:STORY-666 | ai:claude
    let autonomous = doctor_running_autonomously(project_root);
    let mut out = Vec::new();
    let mut by_category: std::collections::BTreeMap<String, Vec<&DoctorFinding>> =
        std::collections::BTreeMap::new();
    for finding in findings {
        by_category
            .entry(finding.category.clone())
            .or_default()
            .push(finding);
    }
    for (category, items) in by_category {
        let safe = items.iter().all(|item| item.safe_heal);
        match doctor_heal_disposition(safe, opts.force, opts.yes, autonomous) {
            HealDisposition::Proceed => {}
            HealDisposition::SkipNeedsForce => {
                out.push(DoctorHealResult {
                    category: category.clone(),
                    id: category.clone(),
                    action: "skipped category requiring manual decision".to_string(),
                    status: "skipped".to_string(),
                    detail: Some(
                        "pass --yes --force only when you want destructive branch cleanup".into(),
                    ),
                });
                continue;
            }
            HealDisposition::GateAutonomous => {
                // STORY-666: fail-closed. A destructive heal was requested
                // (--force --yes) but we are unattended (no TTY / live drain).
                // Refuse it, name exactly what was skipped, and print the precise
                // interactive command to run it under human sign-off. The
                // `skipped` status keeps the audit trail legible and the heal
                // never executes. trace:STORY-666 | ai:claude
                out.push(DoctorHealResult {
                    category: category.clone(),
                    id: category.clone(),
                    action: format!(
                        "gated — destructive heal of {} finding(s) withheld (unattended context)",
                        items.len()
                    ),
                    status: "skipped".to_string(),
                    detail: Some(format!(
                        "destructive fixes require sign-off and were NOT applied in this \
                         unattended context. Re-run it at an interactive terminal: \
                         `aida doctor --heal --force --yes --category {category}`"
                    )),
                });
                continue;
            }
        }
        if !opts.yes && !confirm_doctor_category(&category, items.len())? {
            out.push(DoctorHealResult {
                category: category.clone(),
                id: category.clone(),
                action: "operator declined".to_string(),
                status: "skipped".to_string(),
                detail: None,
            });
            continue;
        }
        for finding in items {
            // BUG-471: a single finding's heal failure must not abort the whole
            // run (the resilient-drain discipline). Record it as a `failed`
            // result and continue; the caller surfaces failures + exits non-zero.
            // trace:BUG-471 | ai:claude
            match heal_doctor_finding(project_root, finding, opts) {
                Ok(result) => out.push(result),
                Err(e) => out.push(DoctorHealResult {
                    category: category.clone(),
                    id: finding.id.clone(),
                    action: finding.action.clone(),
                    status: "failed".to_string(),
                    detail: Some(e.to_string()),
                }),
            }
        }
    }
    Ok(out)
}

fn confirm_doctor_category(category: &str, count: usize) -> Result<bool> {
    use std::io::Write;
    // BUG-407: never block on a prompt nobody can answer. In a non-interactive
    // shell (no TTY — a background task, CI, or piped stdin) `stdin.read_line`
    // blocks forever on an open-but-empty socket (the observed `aida doctor
    // --heal` hang: 7.5min, 0 progress, WCHAN unix_stream_read_generic).
    // Decline fast with guidance instead; `--heal --yes` skips this prompt
    // entirely (the caller only calls us when !opts.yes). trace:BUG-407
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "  {} non-interactive shell — skipping '{}' ({} finding(s)). Re-run \
             `aida doctor --heal --yes` (add --force for destructive categories) \
             to apply without prompting.",
            "Note:".yellow().bold(),
            category,
            count
        );
        return Ok(false);
    }
    print!("Heal {} finding(s) in {}? [y/N] ", count, category);
    std::io::stdout().flush()?;
    let mut ans = String::new();
    std::io::stdin().read_line(&mut ans)?;
    Ok(matches!(
        ans.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn heal_doctor_finding(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
    opts: &DoctorRunOptions,
) -> Result<DoctorHealResult> {
    match finding.category.as_str() {
        "stale-leases" | "abandoned-leases" | "stale-reviewer-leases" => {
            heal_doctor_lease(project_root, finding)
        }
        "brief-spec-drift" | "OBE-briefs" => heal_doctor_brief(finding),
        "spec-status-drift" if finding.action.starts_with("confirm and end lease") => {
            heal_doctor_lease(project_root, finding)
        }
        "spec-status-drift" => heal_doctor_spec_status(project_root, finding),
        "orphan-worktrees" => heal_doctor_orphan_worktree(project_root, finding),
        // TASK-752: git-rm the tracked legacy-store artifact + gitignore it —
        // exactly PR-651's resolution. safe_heal (the detector self-gates on
        // git-canonical mode, so this never touches an active centralized
        // backend). trace:TASK-752 | ai:claude
        "legacy-store-cruft" => heal_doctor_legacy_store_cruft(project_root, finding),
        // BUG-563: git-rm --cached the per-clone runtime file in the STORE
        // worktree + gitignore it + commit on the store worktree. safe_heal (the
        // detector self-gates on git-canonical mode + an attached store
        // worktree). trace:BUG-563 | ai:claude
        "store-tracked-runtime" => heal_doctor_store_tracked_runtime(project_root, finding),
        "orphan-queue-entries" => heal_doctor_orphan_queue_entry(project_root, finding),
        "stale-locks" => heal_doctor_stale_lock(finding),
        "dead-agents" => heal_doctor_dead_agent(project_root, finding),
        "orphan-branches" if opts.force && opts.yes => {
            heal_doctor_orphan_branch(project_root, finding)
        }
        // TASK-717: prune a verified-stale REMOTE branch. DESTRUCTIVE (deletes
        // origin/*) so force+yes gated like local orphan-branch deletion.
        // trace:TASK-717
        "stale-remote-branches" if opts.force && opts.yes => {
            heal_doctor_stale_remote_branch(project_root, finding)
        }
        // TASK-878: GC a merged Agent-tool worktree + its branch. DESTRUCTIVE
        // (worktree removal + branch deletion) so force+yes gated like the
        // stale-remote-branch deletion above, AND routed through the STORY-666
        // autonomous-context refusal in heal_doctor_findings (safe_heal=false).
        // trace:TASK-878
        "merged-agent-worktrees" if opts.force && opts.yes => {
            heal_doctor_merged_agent_worktree(project_root, finding)
        }
        // TASK-673: re-open an uncorroborated Completed spec to Done so it
        // surfaces in the queue's "awaiting commit" lane for triage rather than
        // silently asserting work git never saw. Force-gated (see heal_doctor_findings).
        "completed-without-commit" if opts.force && opts.yes => {
            heal_doctor_completed_without_commit(project_root, finding)
        }
        // TASK-699: remove a stray ancestor instruction file whose @-imports
        // escape the project. DESTRUCTIVE — it deletes a file OUTSIDE the repo —
        // so it's force+yes gated (see heal_doctor_findings's safe-heal gate).
        // trace:TASK-699
        "external-import-bleed" if opts.force && opts.yes => {
            heal_doctor_external_import_bleed(project_root, finding)
        }
        _ => Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some("diagnostic-only category; follow the printed action".to_string()),
        }),
    }
}

/// STORY-496: reap a dead-PID agent-registry entry. The finding id is
/// `{agent_type}#{pid}`. Re-checks the pid is still dead before removing — a
/// pid can be reused by an unrelated process between scan and heal, and we
// must never reap a live agent. trace:STORY-496 | ai:claude
fn heal_doctor_dead_agent(
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
    if process_probe::pid_is_alive(pid) {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!("pid {pid} is now alive — not reaping")),
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

/// TASK-699: opt-in heal for the `external-import-bleed` category — remove a
/// stray ancestor CLAUDE.md / CLAUDE.local.md / AGENTS.md whose @-imports escape
/// the project (the classic accidental-`aida init`-in-a-parent-of-projects
/// scaffold). DESTRUCTIVE: it deletes a file OUTSIDE the repo, so the caller
/// gates it behind --heal --force --yes. Guardrails honored here:
///   - removes ONLY the stray instruction file (the finding id), never the
///     ancestor's docs/ or anything else;
///   - re-reads the file and re-verifies at least one @-import STILL escapes the
///     project right before deleting (mirrors heal_doctor_dead_agent's re-check
///     so a file edited/fixed between scan and heal is left alone);
///   - prints exactly what will be deleted before removing it.
// trace:TASK-699 | ai:claude
fn heal_doctor_external_import_bleed(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let file = std::path::PathBuf::from(&finding.id);
    // Re-check: the file may have been removed or edited since the scan.
    let Ok(content) = std::fs::read_to_string(&file) else {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!("{} already gone", file.display())),
        });
    };
    let file_dir = file.parent().unwrap_or_else(|| std::path::Path::new("/"));
    if !external_import_bleed::file_still_escapes(file_dir, &content, project_root) {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!(
                "{} no longer has @-imports escaping the project — not removing",
                file.display()
            )),
        });
    }
    // Print EXACTLY what will be deleted before acting (guardrail #4).
    println!(
        "  {} removing stray ancestor instruction file: {}",
        "→".red().bold(),
        file.display()
    );
    std::fs::remove_file(&file).with_context(|| format!("failed to remove {}", file.display()))?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: format!("removed stray ancestor instruction file {}", file.display()),
        status: "healed".to_string(),
        detail: None,
    })
}

fn heal_doctor_lease(
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

fn heal_doctor_brief(finding: &DoctorFinding) -> Result<DoctorHealResult> {
    let path = std::path::Path::new(&finding.id);
    if !path.exists() {
        let acked = path.with_file_name(format!(
            "{}.acked",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
        ));
        if acked.exists() {
            return Ok(DoctorHealResult {
                category: finding.category.clone(),
                id: finding.id.clone(),
                action: "brief already acked".to_string(),
                status: "skipped".to_string(),
                detail: Some(acked.display().to_string()),
            });
        }
    }
    ack_agent_brief(path)?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "acked obsolete brief".to_string(),
        status: "healed".to_string(),
        detail: None,
    })
}

fn heal_doctor_spec_status(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let storage = Storage::new(project_root.join(".aida-store"));
    let mut store = storage.load()?;
    let mut action = None;
    for req in &mut store.requirements {
        if req.spec_id.as_deref() != Some(finding.id.as_str()) {
            continue;
        }
        if matches!(req.status, RequirementStatus::Approved)
            && finding.action.starts_with("bump spec to In Progress")
        {
            req.status = RequirementStatus::InProgress;
            req.modified_at = chrono::Utc::now();
            action = Some("bumped Approved spec to In Progress".to_string());
        } else if matches!(req.status, RequirementStatus::InProgress)
            && finding.action.contains("no active lease")
        {
            req.status = RequirementStatus::Approved;
            req.modified_at = chrono::Utc::now();
            action = Some(format!(
                "reverted {} from In Progress to Approved (no active lease found)",
                finding.id
            ));
        }
    }
    if action.is_some() {
        storage.save(&store)?;
    }
    let changed = action.is_some();
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: action.unwrap_or_else(|| "no automatic status change applied".to_string()),
        status: if changed { "healed" } else { "skipped" }.to_string(),
        detail: None,
    })
}

/// TASK-673: re-open a Completed-without-corroboration spec to Done. Done is the
/// "finished on a branch, awaiting merge" state, so the spec lands in the
/// queue's "awaiting commit" lane where the missing corroboration is visible and
/// actionable — far better than a Completed row git can't back up. Re-checks the
/// status is still Completed (a concurrent `aida pull` may have legitimately
/// corroborated it since the scan). The finding id matches either spec_id or
// agreed_id. trace:TASK-673 | ai:claude
fn heal_doctor_completed_without_commit(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let storage = Storage::new(project_root.join(".aida-store"));
    let mut store = storage.load()?;
    let mut action = None;
    for req in &mut store.requirements {
        let matches_id = req.spec_id.as_deref() == Some(finding.id.as_str())
            || req.agreed_id.as_deref() == Some(finding.id.as_str());
        if !matches_id {
            continue;
        }
        if matches!(req.status, RequirementStatus::Completed) {
            req.status = RequirementStatus::Done;
            req.modified_at = chrono::Utc::now();
            action = Some(format!(
                "re-opened {} from Completed to Done (no corroborating commit) for triage",
                finding.id
            ));
        }
        break;
    }
    if action.is_some() {
        storage.save(&store)?;
    }
    let changed = action.is_some();
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: action
            .unwrap_or_else(|| "spec is no longer Completed — nothing to re-open".to_string()),
        status: if changed { "healed" } else { "skipped" }.to_string(),
        detail: None,
    })
}

fn heal_doctor_orphan_worktree(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let worktree = std::path::PathBuf::from(&finding.id);
    let salvage = salvage_worktree_patch(project_root, "orphan-worktree", None, &worktree)?;
    // STORY-714: route through the shared teardown so the pre_destroy
    // cargo-clean hook fires (TASK-0396) and a pooled tree is deregistered.
    let healed = aida_core::worktree_pool_destroy::teardown_worktree_path(
        project_root,
        &worktree,
        &worktree_pool_global_hooks("pre_destroy"),
    )
    .is_ok();
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "removed orphan worktree".to_string(),
        status: if healed { "healed" } else { "failed" }.to_string(),
        detail: salvage.map(|p| format!("salvage patch: {}", p.display())),
    })
}

/// TASK-752: heal a tracked legacy-store-cruft finding — exactly PR-651's
/// resolution: `git rm` the file from the tree, then append the gitignore block
/// (`requirements*.yaml`, `scaffold-report.html`) so the artifacts can't return.
/// Idempotent: a path already untracked (e.g. a prior heal removed it) reports
// `skipped`; the gitignore block is appended only once. trace:TASK-752 | ai:claude
fn heal_doctor_legacy_store_cruft(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let path = &finding.id;

    // Re-confirm the file is still tracked before removing — between scan and
    // heal another heal (or a manual `git rm`) may have already dropped it.
    let still_tracked = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["ls-files", "--error-unmatch", path])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !still_tracked {
        // Still make sure the gitignore guard is in place (idempotent).
        let gi = ensure_legacy_store_cruft_gitignore(project_root)?;
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: "already untracked".to_string(),
            status: "skipped".to_string(),
            detail: gi.then(|| "appended gitignore guard".to_string()),
        });
    }

    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rm", "--quiet", "--"])
        .arg(path)
        .status()
        .with_context(|| format!("git rm {path}"))?;
    if !status.success() {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "failed".to_string(),
            detail: Some(format!("git rm {path} failed")),
        });
    }

    let appended = ensure_legacy_store_cruft_gitignore(project_root)?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "git rm + gitignore (live store is the orphan aida-store branch)".to_string(),
        status: "healed".to_string(),
        detail: appended.then(|| "appended gitignore guard".to_string()),
    })
}

/// TASK-752: append PR-651's exact gitignore guard so the swept legacy-store
/// artifacts can't return. Idempotent — no-op if the patterns are already
// present. Returns whether it wrote anything. trace:TASK-752 | ai:claude
fn ensure_legacy_store_cruft_gitignore(project_root: &std::path::Path) -> Result<bool> {
    use std::io::Write;
    let gitignore_path = project_root.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    // Already guarded? `requirements*.yaml` is the load-bearing pattern.
    if existing.lines().any(|l| l.trim() == "requirements*.yaml") {
        return Ok(false);
    }
    let block = "\n# Legacy pre-git-canonical store snapshots (live store is the orphan aida-store branch)\n\
         requirements*.yaml\n\
         scaffold-report.html\n";
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)
        .with_context(|| format!("opening {}", gitignore_path.display()))?;
    f.write_all(block.as_bytes())
        .with_context(|| format!("appending to {}", gitignore_path.display()))?;
    Ok(true)
}

/// BUG-563: heal a per-clone runtime file wrongly tracked on the orphan
/// `aida-store` branch — `git rm --cached` it IN THE STORE WORKTREE (untrack but
/// keep the working copy, since the live clone still needs its own node.toml /
/// dispenser.toml / cache), ensure the store-worktree gitignore guards the
/// per-clone runtime set, then COMMIT the untrack on the store worktree so the
/// orphan branch stops carrying it. Idempotent: a path already untracked (e.g. a
/// prior heal removed it) reports `skipped`; the gitignore block is appended only
// once; a no-op commit is skipped. trace:BUG-563 | ai:claude
fn heal_doctor_store_tracked_runtime(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let store_worktree = project_root.join(".aida-store");
    let path = &finding.id;

    // Re-confirm the file is still tracked on the orphan branch before removing —
    // between scan and heal another heal (or a manual `git rm`) may have already
    // dropped it.
    let still_tracked = std::process::Command::new("git")
        .arg("-C")
        .arg(&store_worktree)
        .args(["ls-files", "--error-unmatch", path])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !still_tracked {
        // Still make sure the gitignore guard is in place (idempotent).
        let gi = ensure_store_tracked_runtime_gitignore(&store_worktree)?;
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: "already untracked".to_string(),
            status: "skipped".to_string(),
            detail: gi.then(|| "appended gitignore guard".to_string()),
        });
    }

    // Untrack but KEEP the working copy — this clone still needs its own
    // per-clone runtime file; only the orphan branch must stop carrying it.
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&store_worktree)
        .args(["rm", "--cached", "--quiet", "--"])
        .arg(path)
        .status()
        .with_context(|| format!("git rm --cached {path} in store worktree"))?;
    if !status.success() {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "failed".to_string(),
            detail: Some(format!("git rm --cached {path} failed")),
        });
    }

    let appended = ensure_store_tracked_runtime_gitignore(&store_worktree)?;

    // Stage the gitignore (if we touched it) and commit the untrack on the
    // store worktree so the orphan branch stops carrying the per-clone file.
    if appended {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&store_worktree)
            .args(["add", ".gitignore"])
            .status();
    }
    let commit = std::process::Command::new("git")
        .arg("-C")
        .arg(&store_worktree)
        .args([
            "commit",
            "--quiet",
            "-m",
            "chore(store): stop tracking per-clone runtime file (BUG-563)",
        ])
        .status()
        .with_context(|| "committing untrack on store worktree".to_string())?;
    if !commit.success() {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "failed".to_string(),
            detail: Some("commit on store worktree failed".to_string()),
        });
    }

    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "git rm --cached + gitignore + commit on store worktree (per-clone runtime stays untracked)"
            .to_string(),
        status: "healed".to_string(),
        detail: appended.then(|| "appended gitignore guard".to_string()),
    })
}

/// BUG-563: append the per-clone-runtime gitignore guard to the STORE worktree's
/// `.gitignore` so the untracked files can't return on the orphan branch.
/// Idempotent — no-op if the load-bearing patterns are already present. Returns
// whether it wrote anything. trace:BUG-563 | ai:claude
fn ensure_store_tracked_runtime_gitignore(store_worktree: &std::path::Path) -> Result<bool> {
    use std::io::Write;
    let gitignore_path = store_worktree.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    // Already guarded? `.aida/node.toml` is the load-bearing pattern (the one
    // that conflicts on every cross-clone rebase).
    if existing.lines().any(|l| l.trim() == ".aida/node.toml") {
        return Ok(false);
    }
    let block =
        "\n# Per-clone runtime state — must never be tracked on the orphan aida-store branch\n\
         .aida/node.toml\n\
         .aida/dispenser.toml\n\
         .aida/*.lock\n\
         .aida/cache.db\n\
         .aida/cache.db-journal\n\
         .aida/cache.db-shm\n\
         .aida/cache.db-wal\n";
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)
        .with_context(|| format!("opening {}", gitignore_path.display()))?;
    f.write_all(block.as_bytes())
        .with_context(|| format!("appending to {}", gitignore_path.display()))?;
    Ok(true)
}

// TASK-570: heal an orphan queue entry by routing through the same
// `storage.queue_remove` primitive that `aida queue prune --orphaned`
// (TASK-537) uses. Idempotent — a re-run after the entry is already gone
// reports `skipped`. trace:TASK-570 | ai:claude
fn heal_doctor_orphan_queue_entry(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let user_id = current_user_id(None);
    let storage = Storage::new(project_root.join(".aida-store"));
    let entry_uuid: uuid::Uuid = finding
        .id
        .parse()
        .with_context(|| format!("parsing queue entry id {}", finding.id))?;
    let entries = storage.queue_list(&user_id, /* include_completed */ false)?;
    if !entries.iter().any(|e| e.requirement_id == entry_uuid) {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: "orphan queue entry already removed".to_string(),
            status: "skipped".to_string(),
            detail: None,
        });
    }
    storage.queue_remove(&user_id, &entry_uuid)?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "removed orphan queue entry".to_string(),
        status: "healed".to_string(),
        detail: None,
    })
}

fn heal_doctor_stale_lock(finding: &DoctorFinding) -> Result<DoctorHealResult> {
    let path = std::path::Path::new(&finding.id);
    let removed = if path.exists() {
        std::fs::remove_file(path)
            .with_context(|| format!("removing stale lock-info file {}", path.display()))?;
        true
    } else {
        false
    };
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "removed stale cache lock-info file".to_string(),
        status: if removed { "healed" } else { "skipped" }.to_string(),
        detail: if removed {
            None
        } else {
            Some("lock-info file was already gone".to_string())
        },
    })
}

fn heal_doctor_orphan_branch(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["branch", "-D", &finding.id])
        .status()
        .with_context(|| format!("deleting branch {}", finding.id))?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "deleted orphan branch".to_string(),
        status: if status.success() { "healed" } else { "failed" }.to_string(),
        detail: None,
    })
}

#[cfg(test)]
mod story_462_doctor_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn doctor_default_flags_parse_without_subcommand() {
        let cli = Cli::try_parse_from([
            "aida",
            "doctor",
            "--heal",
            "--yes",
            "--category",
            "stale-leases",
            "--json",
        ])
        .unwrap();
        let Command::Doctor {
            heal,
            yes,
            category,
            json,
            force,
            all,
            since,
            fix_sandbox,
            cmd,
        } = cli.command
        else {
            panic!("expected doctor command");
        };
        assert!(heal);
        assert!(yes);
        assert_eq!(category.as_deref(), Some("stale-leases"));
        assert!(json);
        assert!(!force);
        assert!(!all);
        assert!(since.is_none());
        assert!(!fix_sandbox);
        assert!(cmd.is_none());
    }

    #[test]
    fn normalize_doctor_category_accepts_dead_agents() {
        // STORY-496
        for alias in ["dead-agents", "dead-agent", "stale-agents", "agents"] {
            assert_eq!(normalize_doctor_category(alias).unwrap(), "dead-agents");
        }
    }

    // ── TASK-717: stale-remote-branch classification ──

    #[test]
    fn normalize_doctor_category_accepts_stale_remote_branches() {
        // TASK-717
        for alias in [
            "stale-remote-branches",
            "stale-remote-branch",
            "remote-branch",
            "remote-branches",
            "remote-branch-prune",
        ] {
            assert_eq!(
                normalize_doctor_category(alias).unwrap(),
                "stale-remote-branches"
            );
        }
    }

    #[test]
    fn normalize_doctor_category_accepts_merged_agent_worktrees() {
        // TASK-878
        for alias in [
            "merged-agent-worktrees",
            "merged-agent-worktree",
            "agent-worktree",
            "agent-worktrees",
            "worktree-gc",
            "agent-worktree-gc",
        ] {
            assert_eq!(
                normalize_doctor_category(alias).unwrap(),
                "merged-agent-worktrees"
            );
        }
    }

    fn remote_facts() -> RemoteBranchFacts {
        RemoteBranchFacts {
            protected: false,
            has_open_pr: false,
            ancestor_of_main: false,
            spec_referenced_on_main: false,
            spec_terminal: false,
            unique_unmerged_commits: 0,
        }
    }

    #[test]
    fn classify_squash_merged_remote_branch_is_safe_to_delete() {
        // TASK-717: squash-merged → origin/main references the spec, branch tip
        // has a different SHA (so NOT an ancestor) but zero unique commits.
        let facts = RemoteBranchFacts {
            spec_referenced_on_main: true,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::SafeToDelete(_)
        ));
    }

    #[test]
    fn classify_ancestor_of_main_remote_branch_is_safe_to_delete() {
        // TASK-717: fast-forward / non-squash merge → HEAD is an ancestor.
        let facts = RemoteBranchFacts {
            ancestor_of_main: true,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::SafeToDelete(_)
        ));
    }

    #[test]
    fn classify_terminal_spec_remote_branch_is_safe_to_delete() {
        // TASK-717: spec is Completed/Rejected with no unique commits.
        let facts = RemoteBranchFacts {
            spec_terminal: true,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::SafeToDelete(_)
        ));
    }

    #[test]
    fn classify_unique_unmerged_remote_branch_is_kept() {
        // TASK-717: squash-merged BUT carries genuinely-unique unmerged commits
        // (the spock-dev migration-guide case) → KEEP, never delete.
        let facts = RemoteBranchFacts {
            spec_referenced_on_main: true,
            unique_unmerged_commits: 3,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::Keep(_)
        ));
    }

    #[test]
    fn classify_no_merge_signal_remote_branch_is_kept() {
        // TASK-717: no merged/terminal signal at all → never delete; flag.
        let facts = remote_facts();
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::Keep(_)
        ));
    }

    #[test]
    fn classify_open_pr_remote_branch_is_excluded() {
        // TASK-717: an open-PR branch is excluded even if it looks merged.
        let facts = RemoteBranchFacts {
            has_open_pr: true,
            spec_referenced_on_main: true,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::Excluded(_)
        ));
    }

    #[test]
    fn classify_protected_remote_branch_is_excluded() {
        // TASK-717: protected refs (main/master/aida-store) are excluded first,
        // even when every "merged" signal is set.
        let facts = RemoteBranchFacts {
            protected: true,
            ancestor_of_main: true,
            spec_referenced_on_main: true,
            spec_terminal: true,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::Excluded(_)
        ));
    }

    fn agent_wt_facts() -> AgentWorktreeFacts {
        AgentWorktreeFacts {
            dirty: false,
            ancestor_of_main: false,
            pr_merged: false,
            unique_unmerged_commits: 0,
        }
    }

    #[test]
    fn classify_ancestor_of_main_agent_worktree_is_removable() {
        // TASK-878: fast-forward / non-squash merge → branch is an ancestor of
        // origin/main, clean, zero unique commits → safe to GC.
        let facts = AgentWorktreeFacts {
            ancestor_of_main: true,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Removable(_)
        ));
    }

    #[test]
    fn classify_squash_merged_agent_worktree_is_removable() {
        // TASK-878: squash-merged → branch tip differs (NOT ancestor) but the
        // forge reports a merged PR and there are zero unique commits → removable.
        let facts = AgentWorktreeFacts {
            pr_merged: true,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Removable(_)
        ));
    }

    #[test]
    fn classify_unmerged_commits_agent_worktree_is_kept() {
        // TASK-878: PR merged BUT the branch carries genuinely-unique unmerged
        // commits (extra work added after the squash merge) → KEEP, never remove.
        let facts = AgentWorktreeFacts {
            pr_merged: true,
            unique_unmerged_commits: 2,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Keep(_)
        ));
    }

    #[test]
    fn classify_dirty_agent_worktree_is_kept_even_when_merged() {
        // TASK-878: clean != no work, but DIRTY is unambiguously work — a
        // worktree with uncommitted changes is KEPT even when its branch merged.
        let facts = AgentWorktreeFacts {
            dirty: true,
            ancestor_of_main: true,
            pr_merged: true,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Keep(_)
        ));
    }

    #[test]
    fn classify_no_merge_signal_agent_worktree_is_kept() {
        // TASK-878: no merge signal at all (unmerged work in flight) → never
        // remove; flag for the operator.
        let facts = AgentWorktreeFacts {
            unique_unmerged_commits: 5,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Keep(_)
        ));
    }

    // ----- BUG-614: conservative worktree GC predicate -----------------------

    /// The all-safe baseline: agent-managed, merged-or-gone, clean, unlocked,
    /// no active lease → the only combination the GC will remove.
    fn gc_eligible_facts() -> WorktreeGcFacts {
        WorktreeGcFacts {
            is_agent_managed: true,
            merged_or_gone: true,
            dirty: false,
            locked: false,
            has_active_lease: false,
        }
    }

    #[test]
    fn gc_merged_clean_unlocked_no_lease_is_eligible() {
        // BUG-614: all four safety gates pass → Eligible.
        assert_eq!(
            classify_worktree_gc(&gc_eligible_facts()),
            WorktreeGcVerdict::Eligible
        );
    }

    #[test]
    fn gc_dirty_worktree_is_preserved() {
        // BUG-614: uncommitted work is unambiguously "has work" — preserved even
        // when its branch merged.
        let facts = WorktreeGcFacts {
            dirty: true,
            ..gc_eligible_facts()
        };
        assert!(matches!(
            classify_worktree_gc(&facts),
            WorktreeGcVerdict::Preserve(_)
        ));
    }

    #[test]
    fn gc_locked_worktree_is_preserved() {
        // BUG-614: a locked worktree is operator-protected — never removed.
        let facts = WorktreeGcFacts {
            locked: true,
            ..gc_eligible_facts()
        };
        assert!(matches!(
            classify_worktree_gc(&facts),
            WorktreeGcVerdict::Preserve(_)
        ));
    }

    #[test]
    fn gc_unmerged_branch_worktree_is_preserved() {
        // BUG-614: a branch that is NOT merged into the default branch carries
        // unmerged work — preserved.
        let facts = WorktreeGcFacts {
            merged_or_gone: false,
            ..gc_eligible_facts()
        };
        assert!(matches!(
            classify_worktree_gc(&facts),
            WorktreeGcVerdict::Preserve(_)
        ));
    }

    #[test]
    fn gc_active_lease_worktree_is_preserved() {
        // BUG-614: a live process / active session-lease pins the worktree —
        // never removed, even merged + clean.
        let facts = WorktreeGcFacts {
            has_active_lease: true,
            ..gc_eligible_facts()
        };
        assert!(matches!(
            classify_worktree_gc(&facts),
            WorktreeGcVerdict::Preserve(_)
        ));
    }

    #[test]
    fn gc_non_agent_worktree_is_preserved() {
        // BUG-614: the GC only ever touches agent-managed worktrees; a normal
        // work-branch worktree is preserved regardless of the other facts.
        let facts = WorktreeGcFacts {
            is_agent_managed: false,
            ..gc_eligible_facts()
        };
        assert!(matches!(
            classify_worktree_gc(&facts),
            WorktreeGcVerdict::Preserve(_)
        ));
    }

    #[test]
    fn gc_dirty_takes_priority_in_reason_over_unmerged() {
        // BUG-614: when several gates fail, the costliest objection (dirty) is
        // reported — losing uncommitted work is the worst outcome.
        let facts = WorktreeGcFacts {
            dirty: true,
            locked: true,
            merged_or_gone: false,
            has_active_lease: true,
            ..gc_eligible_facts()
        };
        match classify_worktree_gc(&facts) {
            WorktreeGcVerdict::Preserve(reason) => assert!(
                reason.contains("uncommitted"),
                "dirty objection must win, got: {reason}"
            ),
            other => panic!("expected Preserve, got {other:?}"),
        }
    }

    #[test]
    fn gc_worktree_is_active_matches_self_and_descendants() {
        // BUG-614: an active path equal to the worktree OR beneath it pins it.
        let tmp = tempfile::TempDir::new().unwrap();
        let wt = tmp.path().join("agent-abc");
        std::fs::create_dir_all(wt.join("nested")).unwrap();
        let wt_canon = wt.canonicalize().unwrap();
        let nested_canon = wt.join("nested").canonicalize().unwrap();

        // Exact match.
        let mut active = HashSet::new();
        active.insert(wt_canon.clone());
        assert!(worktree_is_active(&wt, &active));

        // Descendant (a live claude one level down) still pins it.
        let mut active = HashSet::new();
        active.insert(nested_canon);
        assert!(worktree_is_active(&wt, &active));

        // Unrelated path does not pin it.
        let other = tmp.path().join("agent-other");
        std::fs::create_dir_all(&other).unwrap();
        let mut active = HashSet::new();
        active.insert(other.canonicalize().unwrap());
        assert!(!worktree_is_active(&wt, &active));
    }

    #[test]
    fn is_agent_managed_worktree_matches_path_and_branch() {
        // TASK-878: scoped to the Agent-tool isolation worktrees by EITHER the
        // path segment or the branch convention.
        use std::path::Path;
        assert!(is_agent_managed_worktree(
            Path::new("/repo/.claude/worktrees/agent-abc123"),
            Some("worktree-agent-abc123"),
        ));
        // Path-only match (branch missing / detached).
        assert!(is_agent_managed_worktree(
            Path::new("/repo/.claude/worktrees/agent-deadbeef"),
            None,
        ));
        // Branch-only match (oddly-pathed but conventional branch).
        assert!(is_agent_managed_worktree(
            Path::new("/tmp/scratch"),
            Some("worktree-agent-9f9f"),
        ));
        // A normal work-branch worktree is NOT in scope.
        assert!(!is_agent_managed_worktree(
            Path::new("/repo/wt/task-281"),
            Some("task-281-foo"),
        ));
    }

    #[test]
    fn merged_agent_worktree_heal_is_gated_in_autonomous_context() {
        // TASK-878 + STORY-666: a merged-agent-worktree finding is destructive
        // (safe_heal=false), so the per-category disposition GATES it in an
        // autonomous context even with --force --yes, and PROCEEDS interactively.
        // matches the finding's classification (destructive → never safe_heal)
        let safe_heal = false;
        // Unattended (autonomous) → fail-closed refusal.
        assert_eq!(
            doctor_heal_disposition(safe_heal, /*force*/ true, /*yes*/ true, /*auto*/ true),
            HealDisposition::GateAutonomous,
        );
        // Interactive TTY sign-off → proceeds.
        assert_eq!(
            doctor_heal_disposition(safe_heal, /*force*/ true, /*yes*/ true, /*auto*/ false),
            HealDisposition::Proceed,
        );
        // Without --force --yes it's the pre-existing manual-decision skip.
        assert_eq!(
            doctor_heal_disposition(safe_heal, /*force*/ false, /*yes*/ false, /*auto*/ false),
            HealDisposition::SkipNeedsForce,
        );
    }

    #[test]
    fn spec_id_from_work_branch_derives_spec_id() {
        // TASK-717
        assert_eq!(
            spec_id_from_work_branch("task-281-foo"),
            Some("TASK-281".to_string())
        );
        assert_eq!(
            spec_id_from_work_branch("bug-100"),
            Some("BUG-100".to_string())
        );
        assert_eq!(
            spec_id_from_work_branch("STORY-86"),
            Some("STORY-86".to_string())
        );
        // Non-work branches yield nothing.
        assert_eq!(spec_id_from_work_branch("spock-dev"), None);
        assert_eq!(spec_id_from_work_branch("pr-271"), None);
    }

    #[test]
    fn doctor_detects_and_reaps_dead_agent_registry_entry() {
        // STORY-496: a registry entry whose pid is dead is reported under
        // `dead-agents` and reaped by the heal.
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().to_path_buf();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        let dead_pid = 4_294_967_294u32; // far above any real pid → never alive
        assert!(!process_probe::pid_is_alive(dead_pid));
        agent_registry::register_spawned_agent(
            &project,
            "claude",
            dead_pid,
            None,
            None,
            project.clone(),
            None,
            None,
        )
        .unwrap();

        // Detect.
        let store = aida_core::models::RequirementsStore::new();
        let findings = collect_doctor_findings(&project, &store, Some("dead-agents")).unwrap();
        assert_eq!(findings.len(), 1, "one dead-agent finding");
        assert_eq!(findings[0].category, "dead-agents");
        assert_eq!(findings[0].id, format!("claude#{dead_pid}"));
        assert!(findings[0].safe_heal);

        // Heal.
        let result = heal_doctor_dead_agent(&project, &findings[0]).unwrap();
        assert_eq!(result.status, "healed");

        // Reaped → no longer reported.
        let after = collect_doctor_findings(&project, &store, Some("dead-agents")).unwrap();
        assert!(after.is_empty(), "reaped entry must not reappear");
    }

    #[test]
    fn heal_doctor_lease_is_idempotent_when_lease_already_gone() {
        // BUG-471: a stale-lease finding whose lease no longer exists (ended
        // earlier in the same heal run, or by a concurrent session) must heal to
        // a no-op skip — NOT an error that aborts the whole heal.
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().to_path_buf();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        let finding = DoctorFinding {
            category: "stale-leases".to_string(),
            id: "deadbeef0000".to_string(),
            summary: "stale lease".to_string(),
            action: "end lease deadbeef0000".to_string(),
            safe_heal: true,
        };
        let result = heal_doctor_lease(&project, &finding).expect("absent lease must not error");
        assert_eq!(result.status, "skipped");
        assert!(result
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("already ended"));
    }

    #[test]
    fn doctor_check_and_heal_subcommands_parse() {
        let check =
            Cli::try_parse_from(["aida", "doctor", "check", "OBE-briefs", "--json", "--all"])
                .unwrap();
        let Command::Doctor {
            cmd:
                Some(cli::DoctorCommand::Check {
                    category,
                    all,
                    json,
                }),
            ..
        } = check.command
        else {
            panic!("expected doctor check");
        };
        assert_eq!(category, "OBE-briefs");
        assert!(all);
        assert!(json);

        let heal = Cli::try_parse_from([
            "aida",
            "doctor",
            "heal",
            "orphan-branches",
            "--yes",
            "--force",
            "--all",
        ])
        .unwrap();
        let Command::Doctor {
            cmd:
                Some(cli::DoctorCommand::Heal {
                    category,
                    yes,
                    force,
                    all,
                    json,
                }),
            ..
        } = heal.command
        else {
            panic!("expected doctor heal");
        };
        assert_eq!(category, "orphan-branches");
        assert!(yes);
        assert!(force);
        assert!(all);
        assert!(!json);
    }

    // trace:TASK-1124 — codex_prompts_drift flags a stale deployed prompt,
    // ignores a matching one, and never nags about a prompt not deployed.
    #[test]
    fn codex_prompts_drift_flags_only_stale_deployed_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let expected = aida_core::scaffolding::codex_prompts::expected_codex_prompts();
        assert!(expected.len() >= 2, "need a couple of prompts to test");
        // Deploy the first prompt correctly (matches source).
        let (fresh_name, fresh_body) = &expected[0];
        std::fs::write(dir.path().join(format!("{fresh_name}.md")), fresh_body).unwrap();
        // Deploy the second prompt STALE (content differs).
        let (stale_name, _) = &expected[1];
        std::fs::write(
            dir.path().join(format!("{stale_name}.md")),
            "stale content\n",
        )
        .unwrap();
        // Every other prompt is never deployed (missing) — must NOT be flagged.

        let drifted = codex_prompts_drift(dir.path());
        assert!(
            drifted.contains(stale_name),
            "stale prompt must be flagged: {drifted:?}"
        );
        assert!(
            !drifted.contains(fresh_name),
            "a matching prompt must NOT be flagged: {drifted:?}"
        );
        assert_eq!(
            drifted.len(),
            1,
            "only the one stale prompt should be flagged (missing = opt-out): {drifted:?}"
        );
    }

    #[test]
    fn legacy_doctor_subcommands_still_parse() {
        let cli = Cli::try_parse_from(["aida", "doctor", "fsck"]).unwrap();
        let Command::Doctor {
            cmd: Some(cli::DoctorCommand::Fsck),
            ..
        } = cli.command
        else {
            panic!("expected legacy doctor fsck");
        };
    }

    // trace:TASK-956 | ai:claude — the TASK-935 surface cuts hide verbs from
    // `--help` with clap `hide = true` but MUST leave them dispatchable. These
    // parse-tests are the bouncer: a hidden command that stops parsing would be
    // a real capability regression, not just a help-text change.
    #[test]
    fn task_956_hidden_top_level_parents_still_dispatch() {
        // punts / worker / headless are hidden from `aida --help` but still parse.
        assert!(matches!(
            Cli::try_parse_from(["aida", "punts", "analyze"])
                .unwrap()
                .command,
            Command::Punts(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["aida", "worker", "directives"])
                .unwrap()
                .command,
            Command::Worker(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["aida", "headless", "tail"])
                .unwrap()
                .command,
            Command::Headless(_)
        ));
    }

    // trace:TASK-956 | ai:claude
    #[test]
    fn task_956_hidden_subcommands_still_dispatch() {
        // doctor verify-relationships — hidden under `doctor`, still parses.
        assert!(matches!(
            Cli::try_parse_from(["aida", "doctor", "verify-relationships", "--repair"])
                .unwrap()
                .command,
            Command::Doctor {
                cmd: Some(cli::DoctorCommand::VerifyRelationships { repair: true, .. }),
                ..
            }
        ));
        // queue load — hidden under `queue`, still parses.
        assert!(matches!(
            Cli::try_parse_from(["aida", "queue", "load"])
                .unwrap()
                .command,
            Command::Queue(cli::QueueCommand::Load { .. })
        ));
        // role repair — hidden under `role`, still parses.
        assert!(matches!(
            Cli::try_parse_from(["aida", "role", "repair"])
                .unwrap()
                .command,
            Command::Role(cli::RoleCommand::Repair { .. })
        ));
    }

    // trace:TASK-956 | ai:claude — the two "merge" twins keep their own
    // distinct behavior (they are NOT collapsed into the canonical sibling's
    // logic, which would drop flags / change the output sink) but are hidden
    // from their parent's `--help`. The canonical sibling stays visible.
    #[test]
    fn task_956_merged_twins_still_dispatch_their_own_logic() {
        // changelog generate — hidden, but keeps its stdout/range surface
        // distinct from the canonical `changelog refresh`.
        assert!(matches!(
            Cli::try_parse_from(["aida", "changelog", "generate", "--since", "v0.7.0"])
                .unwrap()
                .command,
            Command::Changelog(cli::ChangelogCommand::Generate { since: Some(_), .. })
        ));
        // The canonical `changelog refresh` is unchanged and still parses.
        assert!(matches!(
            Cli::try_parse_from(["aida", "changelog", "refresh"])
                .unwrap()
                .command,
            Command::Changelog(cli::ChangelogCommand::Refresh { .. })
        ));
        // doctor fsck — hidden, but keeps its own full-suite logic distinct
        // from the canonical `doctor check` (which requires a category arg).
        assert!(matches!(
            Cli::try_parse_from(["aida", "doctor", "fsck"])
                .unwrap()
                .command,
            Command::Doctor {
                cmd: Some(cli::DoctorCommand::Fsck),
                ..
            }
        ));
    }

    #[test]
    fn doctor_category_aliases_normalize() {
        assert_eq!(normalize_doctor_category("leases").unwrap(), "stale-leases");
        assert_eq!(
            normalize_doctor_category("OBE_briefs").unwrap(),
            "OBE-briefs"
        );
        assert_eq!(normalize_doctor_category("locks").unwrap(), "stale-locks");
        assert!(normalize_doctor_category("not-a-category").is_err());
    }

    // ---- TASK-673: completed-without-commit integrity tripwire ----

    #[test]
    fn normalize_doctor_category_accepts_completed_without_commit() {
        for alias in [
            "completed-without-commit",
            "completed-no-commit",
            "uncorroborated-completed",
            "integrity",
        ] {
            assert_eq!(
                normalize_doctor_category(alias).unwrap(),
                "completed-without-commit"
            );
        }
    }

    #[test]
    fn doctor_category_selected_honours_filter() {
        // No filter selects everything.
        assert!(doctor_category_selected(None, "completed-without-commit").unwrap());
        // Matching (including via alias) selects.
        assert!(doctor_category_selected(Some("integrity"), "completed-without-commit").unwrap());
        // Non-matching filter excludes.
        assert!(
            !doctor_category_selected(Some("stale-leases"), "completed-without-commit").unwrap()
        );
        // Unknown filter is an error, not a silent false.
        assert!(doctor_category_selected(Some("bogus"), "completed-without-commit").is_err());
    }

    #[test]
    fn parse_trace_id_token_extracts_spec_id() {
        assert_eq!(
            parse_trace_id_token("trace:TASK-673"),
            Some("TASK-673".into())
        );
        // git grep on a ref may keep a `ref:file:` prefix — take the last trace:.
        assert_eq!(
            parse_trace_id_token("main:src/a.rs:trace:STORY-86"),
            Some("STORY-86".into())
        );
        // Hierarchical ids (FR-1-042) survive.
        assert_eq!(
            parse_trace_id_token("trace:FR-1-042"),
            Some("FR-1-042".into())
        );
        // Lower-case input normalizes up.
        assert_eq!(parse_trace_id_token("trace:task-9"), Some("TASK-9".into()));
        // Malformed (no hyphen / leading digit) → None.
        assert_eq!(parse_trace_id_token("trace:nope"), None);
        assert_eq!(parse_trace_id_token("no trace here"), None);
    }

    /// Helper: a code repo on `main` plus an attached `.aida-store` whose specs
    /// are saved through `Storage`. Returns (project_root tempdir, storage).
    fn integrity_fixture() -> (tempfile::TempDir, Storage) {
        fn g(root: &std::path::Path, args: &[&str]) {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .expect("git");
            assert!(out.status.success(), "git {:?} failed", args);
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        g(root, &["init", "-q", "-b", "main"]);
        g(root, &["config", "user.email", "t@t.t"]);
        g(root, &["config", "user.name", "t"]);
        std::fs::create_dir_all(root.join(".aida-store")).unwrap();
        let storage = Storage::new(root.join(".aida-store"));
        (tmp, storage)
    }

    fn completed_spec(spec_id: &str) -> Requirement {
        let mut req = Requirement::new(format!("work for {spec_id}"), String::new());
        req.spec_id = Some(spec_id.to_string());
        req.status = RequirementStatus::Completed;
        req
    }

    #[test]
    fn integrity_flags_completed_spec_without_any_reference() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        let g = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
        };
        // A real commit corroborates TASK-100; TASK-200 has no commit at all.
        std::fs::write(root.join("a.txt"), "x").unwrap();
        g(&["add", "."]);
        g(&["commit", "-qm", "feat: do the thing (TASK-100)"]);

        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![completed_spec("TASK-100"), completed_spec("TASK-200")];
        storage.save(&store).unwrap();

        let findings = scan_completed_without_commit(root, &store, None);
        let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["TASK-200"],
            "only the uncorroborated spec flagged"
        );
        assert_eq!(findings[0].category, "completed-without-commit");
        assert!(!findings[0].safe_heal, "re-open is never a safe auto-fix");
    }

    #[test]
    fn integrity_trace_comment_corroborates() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        let g = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
        };
        // No (SPEC-ID) in the subject, but a tracked file carries the trace.
        std::fs::write(
            root.join("lib.rs"),
            "// trace:TASK-300 | ai:claude\nfn f() {}\n",
        )
        .unwrap();
        g(&["add", "."]);
        g(&["commit", "-qm", "chore: scaffold"]);

        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![completed_spec("TASK-300")];
        storage.save(&store).unwrap();

        let findings = scan_completed_without_commit(root, &store, None);
        assert!(
            findings.is_empty(),
            "a tracked // trace:SPEC-ID is corroboration, expected no findings, got {findings:?}"
        );
    }

    #[test]
    fn integrity_ignores_aida_store_bookkeeping_commits() {
        // The orphan-store-style bare "update TASK-NNN" subject must NOT count
        // as a reference — it always exists and would mask every violation.
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        let g = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
        };
        std::fs::write(root.join("a.txt"), "x").unwrap();
        g(&["add", "."]);
        // Bare "update TASK-400" — no parenthesized trailer.
        g(&["commit", "-qm", "update TASK-400"]);

        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![completed_spec("TASK-400")];
        storage.save(&store).unwrap();

        let findings = scan_completed_without_commit(root, &store, None);
        assert_eq!(
            findings.len(),
            1,
            "bare 'update TASK-NNN' must not corroborate"
        );
        assert_eq!(findings[0].id, "TASK-400");
    }

    #[test]
    fn integrity_done_specs_are_not_flagged() {
        // Done is the queue's "awaiting commit" transient, surfaced elsewhere —
        // this check targets only Completed-without-corroboration.
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "--allow-empty", "-qm", "init"])
            .output()
            .unwrap();

        let mut done = completed_spec("TASK-500");
        done.status = RequirementStatus::Done;
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![done];
        storage.save(&store).unwrap();

        assert!(scan_completed_without_commit(root, &store, None).is_empty());
    }

    // trace:TASK-1089 — the default cutoff is the fixed git-canonical migration
    // date, so pre-migration import-cohort completions are exempt by default.
    #[test]
    fn default_cutoff_is_the_fixed_migration_date() {
        assert_eq!(
            default_completed_without_commit_recent_cutoff().as_deref(),
            Some("2026-06-01"),
            "the default completed-without-commit cutoff must be the migration date"
        );
    }

    // trace:TASK-1089 — an ops/cleanup task opts out of the scan via the
    // explicit `doctor:no-code` tag; other tags / no tags do not.
    #[test]
    fn doctor_no_code_tag_opts_a_spec_out_of_the_scan() {
        use std::collections::HashSet;
        let with_optout: HashSet<String> = ["orchestrator", "doctor:no-code"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let without: HashSet<String> = ["orchestrator"].iter().map(|s| s.to_string()).collect();
        assert!(completed_without_commit_opted_out(&with_optout));
        assert!(!completed_without_commit_opted_out(&without));
        assert!(!completed_without_commit_opted_out(&HashSet::new()));
    }

    #[test]
    fn integrity_since_exempts_legacy_specs() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "--allow-empty", "-qm", "init"])
            .output()
            .unwrap();

        // TASK-1089: the legacy-exemption cutoff compares created_at, not
        // modified_at (the git-canonical migration bulk-reset modified_at to a
        // recent timestamp, defeating a modified_at cutoff). Set created_at.
        let mut legacy = completed_spec("TASK-600");
        legacy.created_at = "2020-01-01T00:00:00Z".parse().unwrap();
        let mut recent = completed_spec("TASK-601");
        recent.created_at = chrono::Utc::now();
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![legacy, recent];
        storage.save(&store).unwrap();

        // No cutoff → both flagged.
        assert_eq!(scan_completed_without_commit(root, &store, None).len(), 2);
        // Cutoff after the legacy spec → only the recent one flagged.
        let findings = scan_completed_without_commit(root, &store, Some("2023-01-01"));
        let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["TASK-601"]);
    }

    // trace:TASK-755 | ai:codex
    #[test]
    fn integrity_excludes_non_code_spec_types() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "--allow-empty", "-qm", "init"])
            .output()
            .unwrap();

        let mut task = completed_spec("TASK-700");
        task.req_type = RequirementType::Task;
        let mut decision = completed_spec("ADR-1");
        decision.req_type = RequirementType::Decision;
        let mut principle = completed_spec("PRIN-1");
        principle.req_type = RequirementType::Principle;
        let mut term = completed_spec("TERM-1");
        term.req_type = RequirementType::Term;
        let mut constraint = completed_spec("CON-1");
        constraint.req_type = RequirementType::Constraint;
        let mut vision = completed_spec("VIS-1");
        vision.req_type = RequirementType::Vision;
        let mut doc = completed_spec("DOC-1");
        doc.req_type = RequirementType::Doc;
        let mut meta = completed_spec("META-1");
        meta.req_type = RequirementType::Meta;
        let mut folder = completed_spec("FOLDER-1");
        folder.req_type = RequirementType::Folder;
        let mut epic = completed_spec("EPIC-1");
        epic.req_type = RequirementType::Epic;
        let mut spike = completed_spec("SPIKE-1");
        spike.req_type = RequirementType::Spike;

        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![
            task, decision, principle, term, constraint, vision, doc, meta, folder, epic, spike,
        ];
        storage.save(&store).unwrap();

        let findings = scan_completed_without_commit(root, &store, None);
        let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["TASK-700"]);
    }

    // trace:TASK-755 | ai:codex
    #[test]
    fn integrity_collapses_historical_tail_by_default() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "--allow-empty", "-qm", "init"])
            .output()
            .unwrap();

        // TASK-1089: the historical-tail cutoff compares created_at, not
        // modified_at (the migration bulk-reset modified_at to a recent value).
        let mut old = completed_spec("TASK-710");
        old.created_at = "2020-01-01T00:00:00Z".parse().unwrap();
        let mut recent = completed_spec("TASK-711");
        recent.created_at = chrono::Utc::now();
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![old, recent];
        storage.save(&store).unwrap();

        let scan =
            scan_completed_without_commit_with_options(root, &store, Some("2023-01-01"), false);
        let ids: Vec<&str> = scan.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["TASK-711"]);
        assert_eq!(scan.hidden_older, 1);
    }

    // trace:TASK-755 | ai:codex
    #[test]
    fn integrity_all_expands_historical_tail() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "--allow-empty", "-qm", "init"])
            .output()
            .unwrap();

        let mut old = completed_spec("TASK-720");
        old.modified_at = "2020-01-01T00:00:00Z".parse().unwrap();
        let mut recent = completed_spec("TASK-721");
        recent.modified_at = chrono::Utc::now();
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![old, recent];
        storage.save(&store).unwrap();

        let scan =
            scan_completed_without_commit_with_options(root, &store, Some("2023-01-01"), true);
        let ids: Vec<&str> = scan.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["TASK-720", "TASK-721"]);
        assert_eq!(scan.hidden_older, 0);
    }

    #[test]
    fn integrity_heal_reopens_completed_to_done() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![completed_spec("TASK-700")];
        storage.save(&store).unwrap();

        let finding = DoctorFinding {
            category: "completed-without-commit".to_string(),
            id: "TASK-700".to_string(),
            summary: "Completed spec TASK-700 has no commit ...".to_string(),
            action: "re-open".to_string(),
            safe_heal: false,
        };
        let result = heal_doctor_completed_without_commit(root, &finding).unwrap();
        assert_eq!(result.status, "healed");
        let reloaded = storage.load().unwrap();
        assert_eq!(reloaded.requirements[0].status, RequirementStatus::Done);

        // Idempotent: a second heal is a no-op (spec already left Completed).
        let again = heal_doctor_completed_without_commit(root, &finding).unwrap();
        assert_eq!(again.status, "skipped");
    }

    /// BUG-407: `confirm_doctor_category` must NOT block on stdin in a
    /// non-interactive shell (the `aida doctor --heal` hang). Under `cargo
    /// test` stdin is non-interactive, so this returns Ok(false) immediately
    /// and the test completing at all proves it doesn't block. Also locks the
    /// contract: non-interactive declines (never silently auto-confirms).
    /// The open-socket hang condition itself is verified empirically (a
    // background `--heal` run with a finding present). trace:BUG-407 | ai:claude
    #[test]
    fn confirm_doctor_category_declines_in_non_interactive_shell() {
        assert!(!confirm_doctor_category("stale-leases", 3).unwrap());
    }

    // ------------------------------------------------------------------
    // STORY-666: destructive `--heal` requires sign-off in autonomous
    // contexts; safe fixes proceed everywhere. trace:STORY-666 | ai:claude
    // ------------------------------------------------------------------

    /// The pure disposition matrix — the single place the fail-closed invariant
    /// lives. Locks every cell.
    #[test]
    fn doctor_heal_disposition_matrix() {
        use HealDisposition::*;
        // Safe fixes ALWAYS proceed — every context, regardless of flags.
        for &force in &[false, true] {
            for &yes in &[false, true] {
                for &auto in &[false, true] {
                    assert_eq!(
                        doctor_heal_disposition(true, force, yes, auto),
                        Proceed,
                        "safe heal must proceed (force={force} yes={yes} auto={auto})"
                    );
                }
            }
        }
        // Destructive without the --force --yes opt-in → the pre-existing
        // needs-a-decision skip, in EVERY context (unchanged).
        for &auto in &[false, true] {
            assert_eq!(
                doctor_heal_disposition(false, false, false, auto),
                SkipNeedsForce
            );
            assert_eq!(
                doctor_heal_disposition(false, true, false, auto),
                SkipNeedsForce
            );
            assert_eq!(
                doctor_heal_disposition(false, false, true, auto),
                SkipNeedsForce
            );
        }
        // Destructive + --force --yes + INTERACTIVE (TTY) = explicit human
        // sign-off → proceeds (req #3: interactive path unchanged).
        assert_eq!(doctor_heal_disposition(false, true, true, false), Proceed);
        // Destructive + --force --yes + AUTONOMOUS = fail-closed gate (the
        // invariant): the heal must NOT execute.
        assert_eq!(
            doctor_heal_disposition(false, true, true, true),
            GateAutonomous
        );
    }

    /// The autonomous-context detector: no TTY OR a live orchestrator.
    #[test]
    fn doctor_context_is_autonomous_signals() {
        // Interactive TTY, no orchestrator → attended.
        assert!(!doctor_context_is_autonomous(true, false));
        // No TTY (piped / CI) → autonomous.
        assert!(doctor_context_is_autonomous(false, false));
        // Live orchestrator drain even at a "TTY" → autonomous.
        assert!(doctor_context_is_autonomous(true, true));
        assert!(doctor_context_is_autonomous(false, true));
    }

    /// End-to-end: in a simulated non-interactive context (under `cargo test`
    /// stdin is not a TTY, and the tempdir has no live drain → autonomous),
    /// a DESTRUCTIVE finding requested with `--force --yes` is GATED — the heal
    /// never executes and the spec is left untouched. This is the heart of the
    /// safety rail: it FAILS against the old code (which would re-open the spec)
    // and PASSES with the gate. trace:STORY-666 | ai:claude
    #[test]
    fn destructive_heal_is_gated_in_autonomous_context() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![completed_spec("TASK-666")];
        storage.save(&store).unwrap();

        // `completed-without-commit` is a destructive (safe_heal=false) category:
        // its heal re-opens a Completed spec back to Done.
        let finding = DoctorFinding {
            category: "completed-without-commit".to_string(),
            id: "TASK-666".to_string(),
            summary: "Completed spec TASK-666 has no commit".to_string(),
            action: "re-open".to_string(),
            safe_heal: false,
        };

        // --force --yes requested, but we are unattended (no TTY under test).
        let results = heal_doctor_findings(
            root,
            &[finding],
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: Some("completed-without-commit".to_string()),
                json: false,
                force: true,
                all: false,
                since: None,
            },
        )
        .unwrap();

        // The category was gated (skipped), not healed.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, "skipped");
        assert!(
            results[0].action.contains("gated"),
            "expected a gate result, got: {:?}",
            results[0]
        );
        assert!(
            results[0]
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("--force --yes --category"),
            "gate detail must name the interactive resume command"
        );

        // The destructive action did NOT run: the spec is still Completed.
        let reloaded = storage.load().unwrap();
        assert_eq!(
            reloaded.requirements[0].status,
            RequirementStatus::Completed,
            "destructive heal must NOT have executed in the autonomous context"
        );
    }

    /// End-to-end companion: in the SAME autonomous context, a SAFE
    /// (reversible) fix DOES proceed — the gate is scoped to the destructive
    /// subset only and must never over-gate routine reversible fixes.
    // trace:STORY-666 | ai:claude
    #[test]
    fn safe_heal_proceeds_in_autonomous_context() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
        let storage = Storage::new(project_root.join(".aida-store"));
        let mut req = Requirement::new("orphaned in-progress".into(), String::new());
        req.spec_id = Some("TASK-667".into());
        req.status = RequirementStatus::InProgress;
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![req];
        storage.save(&store).unwrap();

        // `spec-status-drift` (in-progress with no lease → revert to Approved)
        // is a SAFE (safe_heal=true) reversible fix.
        let finding = DoctorFinding {
            category: "spec-status-drift".to_string(),
            id: "TASK-667".to_string(),
            summary: "TASK-667 is In Progress but no active lease holds it".to_string(),
            action: "revert spec to Approved (no active lease found)".to_string(),
            safe_heal: true,
        };

        // Note: no --force, --yes set (so the safe-category TTY confirm is
        // skipped) — and we are non-interactive (autonomous) under test.
        let results = heal_doctor_findings(
            project_root,
            &[finding],
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: Some("spec-status-drift".to_string()),
                json: false,
                force: false,
                all: false,
                since: None,
            },
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].status, "healed",
            "safe reversible fix must proceed in the autonomous context, got: {:?}",
            results[0]
        );
        let reloaded = storage.load().unwrap();
        assert_eq!(reloaded.requirements[0].status, RequirementStatus::Approved);
    }

    #[test]
    fn doctor_detects_and_heals_stale_cache_lock_info() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
        let cache_path =
            aida_core::CachedGitBackend::default_cache_path(&project_root.join(".aida-store"));
        let lock_info_path = aida_core::cache_lock_info_path(&cache_path);
        let info = aida_core::CacheLockInfo {
            pid: 999_999,
            command: "aida list".to_string(),
            started_at: (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339(),
            user: "tester".to_string(),
            session_id: None,
        };
        std::fs::write(&lock_info_path, serde_json::to_string(&info).unwrap()).unwrap();

        let findings = collect_doctor_findings(
            project_root,
            &aida_core::models::RequirementsStore::new(),
            Some("stale-locks"),
        )
        .unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "stale-locks");
        assert!(findings[0].summary.contains("dead pid"));

        let result = heal_doctor_finding(
            project_root,
            &findings[0],
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: Some("stale-locks".to_string()),
                json: false,
                force: false,
                all: false,
                since: None,
            },
        )
        .unwrap();
        assert_eq!(result.status, "healed");
        assert!(!lock_info_path.exists());
    }

    #[test]
    fn doctor_heals_in_progress_without_lease_back_to_approved() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
        let storage = Storage::new(project_root.join(".aida-store"));
        let mut req = Requirement::new("orphaned in-progress".into(), String::new());
        req.spec_id = Some("TASK-561".into());
        req.status = RequirementStatus::InProgress;
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![req];
        storage.save(&store).unwrap();

        let finding = DoctorFinding {
            category: "spec-status-drift".to_string(),
            id: "TASK-561".to_string(),
            summary: "TASK-561 is In Progress but no active lease holds it".to_string(),
            action: "revert spec to Approved (no active lease found)".to_string(),
            safe_heal: true,
        };
        let result = heal_doctor_finding(
            project_root,
            &finding,
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: Some("spec-status-drift".to_string()),
                json: false,
                force: false,
                all: false,
                since: None,
            },
        )
        .unwrap();

        assert_eq!(result.status, "healed");
        assert_eq!(
            result.action,
            "reverted TASK-561 from In Progress to Approved (no active lease found)"
        );
        let reloaded = storage.load().unwrap();
        assert_eq!(reloaded.requirements[0].status, RequirementStatus::Approved);
    }

    #[test]
    fn doctor_skips_stale_in_progress_orphan_finding_after_manual_fix() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
        let storage = Storage::new(project_root.join(".aida-store"));
        let mut req = Requirement::new("already fixed".into(), String::new());
        req.spec_id = Some("TASK-561".into());
        req.status = RequirementStatus::Approved;
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![req];
        storage.save(&store).unwrap();

        let finding = DoctorFinding {
            category: "spec-status-drift".to_string(),
            id: "TASK-561".to_string(),
            summary: "TASK-561 is In Progress but no active lease holds it".to_string(),
            action: "revert spec to Approved (no active lease found)".to_string(),
            safe_heal: true,
        };
        let result = heal_doctor_finding(
            project_root,
            &finding,
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: Some("spec-status-drift".to_string()),
                json: false,
                force: false,
                all: false,
                since: None,
            },
        )
        .unwrap();

        assert_eq!(result.status, "skipped");
        let reloaded = storage.load().unwrap();
        assert_eq!(reloaded.requirements[0].status, RequirementStatus::Approved);
    }

    // TASK-570: doctor detects orphan queue entries — rows whose backing
    // spec is no longer in the store. trace:TASK-570 | ai:claude
    #[test]
    fn doctor_detects_orphan_queue_entries() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();

        let storage = Storage::new(project_root.join(".aida-store"));
        let store = aida_core::models::RequirementsStore::new();
        storage.save(&store).unwrap();

        let user_id = current_user_id(None);
        let orphan_id = uuid::Uuid::new_v4();
        let entry = aida_core::models::QueueEntry {
            user_id: user_id.clone(),
            requirement_id: orphan_id,
            position: 1,
            added_by: user_id.clone(),
            note: Some("auto-queued by aida pr".into()),
            added_at: chrono::Utc::now(),
            for_role: Some("reviewer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        };
        storage.queue_add(entry).unwrap();

        let findings =
            collect_doctor_findings(project_root, &store, Some("orphan-queue-entries")).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "orphan-queue-entries");
        assert_eq!(findings[0].id, orphan_id.to_string());
        assert!(findings[0].summary.contains("position 1"));
        assert!(findings[0].summary.contains("[for:reviewer]"));
        assert!(findings[0].summary.contains("auto-queued by aida pr"));
        assert!(findings[0].safe_heal);
    }

    // TASK-570: --heal removes the orphan queue entry, mirroring the
    // behaviour of `aida queue prune --orphaned`. Re-running on a clean
    // queue is a no-op (skipped). trace:TASK-570 | ai:claude
    #[test]
    fn doctor_heals_orphan_queue_entries() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();

        let storage = Storage::new(project_root.join(".aida-store"));
        let store = aida_core::models::RequirementsStore::new();
        storage.save(&store).unwrap();

        let user_id = current_user_id(None);
        let orphan_id = uuid::Uuid::new_v4();
        let entry = aida_core::models::QueueEntry {
            user_id: user_id.clone(),
            requirement_id: orphan_id,
            position: 1,
            added_by: user_id.clone(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: Some("reviewer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        };
        storage.queue_add(entry).unwrap();

        let finding = DoctorFinding {
            category: "orphan-queue-entries".to_string(),
            id: orphan_id.to_string(),
            summary: "queue position 1 [for:reviewer] points at deleted spec".to_string(),
            action: "remove orphan queue entry (aida queue prune --orphaned)".to_string(),
            safe_heal: true,
        };
        let opts = DoctorRunOptions {
            heal: true,
            yes: true,
            category: Some("orphan-queue-entries".to_string()),
            json: false,
            force: false,
            all: false,
            since: None,
        };
        let result = heal_doctor_finding(project_root, &finding, &opts).unwrap();
        assert_eq!(result.status, "healed");
        assert_eq!(result.action, "removed orphan queue entry");

        let entries = storage.queue_list(&user_id, false).unwrap();
        assert!(
            entries.iter().all(|e| e.requirement_id != orphan_id),
            "orphan entry should be removed from the queue"
        );

        // Idempotent: a second heal on an already-clean queue reports skipped.
        let result2 = heal_doctor_finding(project_root, &finding, &opts).unwrap();
        assert_eq!(result2.status, "skipped");
    }

    // TASK-570: the orphan-queue-entries alias set normalizes the way the
    // other doctor categories do. trace:TASK-570 | ai:claude
    #[test]
    fn doctor_orphan_queue_aliases_normalize() {
        assert_eq!(
            normalize_doctor_category("orphan-queue").unwrap(),
            "orphan-queue-entries"
        );
        assert_eq!(
            normalize_doctor_category("queue-orphans").unwrap(),
            "orphan-queue-entries"
        );
        assert_eq!(
            normalize_doctor_category("orphan_queue_entry").unwrap(),
            "orphan-queue-entries"
        );
    }

    #[test]
    fn salvage_component_sanitizes_path_like_values() {
        assert_eq!(sanitize_salvage_component("TASK-515/../x"), "TASK-515-x");
        assert_eq!(sanitize_salvage_component("advisor role"), "advisor-role");
    }

    #[test]
    fn salvage_worktree_patch_writes_diff_before_cleanup() {
        let project = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .arg("init")
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["config", "user.email", "t@example.com"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["config", "user.name", "Test"])
            .output()
            .unwrap();
        let file = worktree.path().join("notes.txt");
        std::fs::write(&file, "before\n").unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["add", "notes.txt"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["commit", "-m", "init"])
            .output()
            .unwrap();
        std::fs::write(&file, "after\n").unwrap();

        let patch =
            salvage_worktree_patch(project.path(), "TASK-515", Some("codex"), worktree.path())
                .unwrap()
                .expect("dirty worktree should produce salvage patch");
        let body = std::fs::read_to_string(&patch).unwrap();
        assert!(patch
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("TASK-515-codex-attempt-"));
        assert!(body.contains("# AIDA salvage patch"));
        assert!(body.contains("-before"));
        assert!(body.contains("+after"));
    }

    // BUG-696: the salvage patch must capture untracked files' CONTENT, not
    // just their names — otherwise `aida doctor --heal` loses them when it
    // tears down the orphan-worktree after salvage.
    #[test]
    fn salvage_worktree_patch_captures_untracked_file_content() {
        let project = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        for args in [
            vec!["init"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "Test"],
        ] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(worktree.path())
                .args(&args)
                .output()
                .unwrap();
        }
        // Commit a file, then dirty it (guarantees salvage runs) AND drop an
        // untracked file whose content must survive.
        let tracked = worktree.path().join("tracked.txt");
        std::fs::write(&tracked, "before\n").unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["add", "tracked.txt"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["commit", "-m", "init"])
            .output()
            .unwrap();
        std::fs::write(&tracked, "after\n").unwrap();
        std::fs::write(
            worktree.path().join("new_idea.txt"),
            "SALVAGE_ME_UNIQUE_MARKER untracked body\n",
        )
        .unwrap();

        let patch = salvage_worktree_patch(project.path(), "TASK-1", None, worktree.path())
            .unwrap()
            .expect("dirty worktree should produce salvage patch");
        let body = std::fs::read_to_string(&patch).unwrap();
        assert!(body.contains("new_idea.txt"), "untracked filename indexed");
        assert!(
            body.contains("SALVAGE_ME_UNIQUE_MARKER"),
            "untracked file CONTENT must be captured, not just the name:\n{body}"
        );
    }

    // ── TASK-752: legacy-store-cruft detection + guard ──

    #[test]
    fn normalize_doctor_category_accepts_legacy_store_cruft() {
        // TASK-752
        for alias in [
            "legacy-store-cruft",
            "legacy-store",
            "store-cruft",
            "legacy-cruft",
            "requirements-yaml",
        ] {
            assert_eq!(
                normalize_doctor_category(alias).unwrap(),
                "legacy-store-cruft"
            );
        }
    }

    #[test]
    fn is_legacy_store_cruft_path_matches_top_level_only() {
        // TASK-752: top-level legacy artifacts match.
        assert!(is_legacy_store_cruft_path("requirements.yaml"));
        assert!(is_legacy_store_cruft_path(
            "requirements_20251206_205840.yaml"
        ));
        assert!(is_legacy_store_cruft_path("scaffold-report.html"));
        // Nested paths and unrelated files never match.
        assert!(!is_legacy_store_cruft_path(
            "tests/fixtures/requirements.yaml"
        ));
        assert!(!is_legacy_store_cruft_path("src/requirements.yaml"));
        assert!(!is_legacy_store_cruft_path("my-requirements.yaml"));
        assert!(!is_legacy_store_cruft_path("requirements.json"));
        assert!(!is_legacy_store_cruft_path("docs/scaffold-report.html"));
    }

    fn git752(root: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {:?}: {:?}", args, out);
    }

    /// TASK-752: a distributed/git-canonical project (config declares
    /// `mode = "distributed"` AND the orphan `aida-store` branch exists) with a
    /// tracked `requirements.yaml` + `scaffold-report.html` flags both, and the
    /// heal git-rm's them + appends the gitignore guard.
    #[test]
    fn detects_and_heals_legacy_store_cruft_on_git_canonical_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git752(root, &["init", "-q", "-b", "main"]);
        git752(root, &["config", "user.email", "t@t.t"]);
        git752(root, &["config", "user.name", "t"]);
        // git-canonical: config declares distributed mode...
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[store]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        // ...and the orphan aida-store branch exists.
        std::fs::write(root.join("README.md"), "x\n").unwrap();
        git752(root, &["add", "."]);
        git752(root, &["commit", "-qm", "init"]);
        git752(root, &["branch", "aida-store"]);
        // Tracked legacy-store artifacts.
        std::fs::write(root.join("requirements.yaml"), "legacy: store\n").unwrap();
        std::fs::write(root.join("scaffold-report.html"), "<html/>\n").unwrap();
        git752(root, &["add", "requirements.yaml", "scaffold-report.html"]);
        git752(root, &["commit", "-qm", "cruft"]);

        // Detect.
        let store = aida_core::models::RequirementsStore::new();
        let findings = collect_doctor_findings(root, &store, Some("legacy-store-cruft")).unwrap();
        assert_eq!(findings.len(), 2, "both cruft files flagged: {findings:?}");
        assert!(findings.iter().all(|f| f.category == "legacy-store-cruft"));
        assert!(findings.iter().all(|f| f.safe_heal));
        let ids: std::collections::HashSet<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains("requirements.yaml"));
        assert!(ids.contains("scaffold-report.html"));

        // Heal both.
        for finding in &findings {
            let result = heal_doctor_legacy_store_cruft(root, finding).unwrap();
            assert_eq!(result.status, "healed", "{result:?}");
        }
        // git-rm'd → no longer tracked → no longer flagged.
        let after = collect_doctor_findings(root, &store, Some("legacy-store-cruft")).unwrap();
        assert!(
            after.is_empty(),
            "healed cruft must not reappear: {after:?}"
        );
        // gitignore guard appended exactly PR-651's block.
        let gi = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gi.contains("requirements*.yaml"));
        assert!(gi.contains("scaffold-report.html"));
    }

    /// TASK-752 GUARD: a legacy `--centralized` project that legitimately USES
    /// `requirements.yaml` as its active store (config does NOT declare
    /// distributed mode, no orphan aida-store branch) is a NO-OP — we must never
    /// flag/nuke an active centralized backend.
    #[test]
    fn legacy_store_cruft_is_noop_on_centralized_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git752(root, &["init", "-q", "-b", "main"]);
        git752(root, &["config", "user.email", "t@t.t"]);
        git752(root, &["config", "user.name", "t"]);
        // Legacy centralized: requirements.yaml is the ACTIVE store, no
        // distributed-mode config, no orphan aida-store branch.
        std::fs::write(
            root.join("requirements.yaml"),
            "active: centralized store\n",
        )
        .unwrap();
        git752(root, &["add", "."]);
        git752(root, &["commit", "-qm", "centralized"]);

        // No distributed-mode declaration → no orphan branch → no-op.
        assert!(
            detect_legacy_store_cruft(root).is_empty(),
            "must not flag an active centralized requirements.yaml"
        );
        let store = aida_core::models::RequirementsStore::new();
        let findings = collect_doctor_findings(root, &store, Some("legacy-store-cruft")).unwrap();
        assert!(findings.is_empty(), "centralized project: no findings");
    }

    /// TASK-752: even with distributed-mode config, if the orphan aida-store
    /// branch is MISSING (half-migrated config), stay silent — Gate 2.
    #[test]
    fn legacy_store_cruft_is_noop_without_orphan_branch() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git752(root, &["init", "-q", "-b", "main"]);
        git752(root, &["config", "user.email", "t@t.t"]);
        git752(root, &["config", "user.name", "t"]);
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[store]\nmode = \"distributed\"\n",
        )
        .unwrap();
        std::fs::write(root.join("requirements.yaml"), "x\n").unwrap();
        git752(root, &["add", "."]);
        git752(root, &["commit", "-qm", "init"]);
        // No `aida-store` branch created.
        assert!(
            detect_legacy_store_cruft(root).is_empty(),
            "missing orphan branch → no-op"
        );
    }

    // ── BUG-563: store-tracked per-clone runtime detection + heal ──

    #[test]
    fn normalize_doctor_category_accepts_store_tracked_runtime() {
        // BUG-563
        for alias in [
            "store-tracked-runtime",
            "store-runtime",
            "tracked-runtime",
            "store-node-toml",
            "store-runtime-cruft",
        ] {
            assert_eq!(
                normalize_doctor_category(alias).unwrap(),
                "store-tracked-runtime"
            );
        }
    }

    #[test]
    fn is_store_tracked_runtime_path_matches_per_clone_runtime() {
        // BUG-563: the per-clone runtime set under top-level `.aida/`.
        assert!(is_store_tracked_runtime_path(".aida/node.toml"));
        assert!(is_store_tracked_runtime_path(".aida/dispenser.toml"));
        assert!(is_store_tracked_runtime_path(".aida/dispenser.lock"));
        assert!(is_store_tracked_runtime_path(".aida/sync.lock"));
        assert!(is_store_tracked_runtime_path(".aida/cache.db"));
        assert!(is_store_tracked_runtime_path(".aida/cache.db-journal"));
        assert!(is_store_tracked_runtime_path(".aida/cache.db-wal"));
        assert!(is_store_tracked_runtime_path(".aida/cache.db-shm"));
        // Shared, legitimately-tracked store files never match.
        assert!(!is_store_tracked_runtime_path(".aida/config.toml"));
        assert!(!is_store_tracked_runtime_path("metadata.yaml"));
        assert!(!is_store_tracked_runtime_path(
            "objects/TASK/000/TASK-1.yaml"
        ));
        assert!(!is_store_tracked_runtime_path(
            "registry/agreed_counters.toml"
        ));
        // Not under .aida/, or nested deeper than top-level — never match.
        assert!(!is_store_tracked_runtime_path("node.toml"));
        assert!(!is_store_tracked_runtime_path(".aida/sub/node.toml"));
        assert!(!is_store_tracked_runtime_path("src/.aida/node.toml"));
    }

    /// BUG-563: build a project that declares distributed mode with an attached
    /// `.aida-store` worktree (orphan `aida-store` branch) whose tree TRACKS
    /// `.aida/node.toml` + `.aida/cache.db` + a `.aida/*.lock`. The detector
    /// flags all three; the heal `git rm --cached`'s + gitignores + commits on
    /// the store worktree, after which they no longer reappear and the working
    /// copies survive (per-clone state stays on disk).
    #[test]
    fn detects_and_heals_store_tracked_runtime_on_git_canonical_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git752(root, &["init", "-q", "-b", "main"]);
        git752(root, &["config", "user.email", "t@t.t"]);
        git752(root, &["config", "user.name", "t"]);
        // git-canonical: config declares distributed mode.
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[store]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        std::fs::write(root.join("README.md"), "x\n").unwrap();
        git752(root, &["add", "."]);
        git752(root, &["commit", "-qm", "init"]);

        // Attach the orphan `aida-store` worktree at `.aida-store/`.
        git752(
            root,
            &["worktree", "add", "-q", "-b", "aida-store", ".aida-store"],
        );
        let store = root.join(".aida-store");
        // Make it look like a real store (the detector gates on objects/).
        std::fs::create_dir_all(store.join("objects")).unwrap();
        std::fs::write(store.join("objects/.keep"), "").unwrap();
        std::fs::create_dir_all(store.join(".aida")).unwrap();
        // Per-clone runtime files WRONGLY tracked on the orphan branch.
        std::fs::write(store.join(".aida/node.toml"), "node_id = 1\n").unwrap();
        std::fs::write(store.join(".aida/dispenser.toml"), "next = 5\n").unwrap();
        std::fs::write(store.join(".aida/sync.lock"), "").unwrap();
        std::fs::write(store.join(".aida/cache.db"), "binary").unwrap();
        git752(&store, &["add", "-A"]);
        git752(&store, &["commit", "-qm", "store w/ tracked runtime"]);

        // Detect: all four per-clone runtime files flagged.
        let req_store = aida_core::models::RequirementsStore::new();
        let findings =
            collect_doctor_findings(root, &req_store, Some("store-tracked-runtime")).unwrap();
        assert_eq!(findings.len(), 4, "all runtime files flagged: {findings:?}");
        assert!(findings
            .iter()
            .all(|f| f.category == "store-tracked-runtime"));
        assert!(findings.iter().all(|f| f.safe_heal));
        let ids: std::collections::HashSet<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains(".aida/node.toml"));
        assert!(ids.contains(".aida/dispenser.toml"));
        assert!(ids.contains(".aida/sync.lock"));
        assert!(ids.contains(".aida/cache.db"));

        // Heal each.
        for finding in &findings {
            let result = heal_doctor_store_tracked_runtime(root, finding).unwrap();
            assert_eq!(result.status, "healed", "{result:?}");
        }
        // git-rm --cached'd → no longer tracked → no longer flagged.
        let after =
            collect_doctor_findings(root, &req_store, Some("store-tracked-runtime")).unwrap();
        assert!(
            after.is_empty(),
            "healed runtime must not reappear: {after:?}"
        );
        // Working copies survive (per-clone state stays on disk).
        assert!(store.join(".aida/node.toml").exists());
        assert!(store.join(".aida/cache.db").exists());
        // gitignore guard appended on the STORE worktree.
        let gi = std::fs::read_to_string(store.join(".gitignore")).unwrap();
        assert!(gi.contains(".aida/node.toml"));
        assert!(gi.contains(".aida/dispenser.toml"));
        assert!(gi.contains(".aida/*.lock"));
        assert!(gi.contains(".aida/cache.db"));
    }

    /// BUG-563 GUARD: with distributed-mode config but NO attached `.aida-store`
    /// worktree, the detector is a NO-OP (Gate 2) — nothing to scan.
    #[test]
    fn store_tracked_runtime_is_noop_without_store_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git752(root, &["init", "-q", "-b", "main"]);
        git752(root, &["config", "user.email", "t@t.t"]);
        git752(root, &["config", "user.name", "t"]);
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[store]\nmode = \"distributed\"\n",
        )
        .unwrap();
        std::fs::write(root.join("README.md"), "x\n").unwrap();
        git752(root, &["add", "."]);
        git752(root, &["commit", "-qm", "init"]);
        // No `.aida-store/` worktree attached.
        assert!(
            detect_store_tracked_runtime(root).is_empty(),
            "no store worktree → no-op"
        );
    }
}

/// STORY-70: walk the orphan store, flag STORY/BUG requirements whose
/// descriptions don't contain a recognized acceptance heading. Output
/// shape mirrors the other doctor commands (per-finding rows + a final
/// summary). Exits non-zero on findings so CI/scripts can gate on it.
// trace:STORY-70 | ai:claude
fn doctor_convention_check(quiet: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let objects_root = project_root.join(".aida-store").join("objects");
    if !objects_root.exists() {
        println!("(no objects/ tree — nothing to check)");
        return Ok(());
    }

    let reqs = aida_core::object_store::load_all_objects(&objects_root)?;
    let mut total_in_scope: usize = 0;
    let mut missing: Vec<(String, String)> = Vec::new();
    for req in &reqs {
        if !matches!(
            req.req_type,
            aida_core::RequirementType::Story | aida_core::RequirementType::Bug
        ) {
            continue;
        }
        total_in_scope += 1;
        if requirement_missing_acceptance(req) {
            let id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
            missing.push((id, req.title.clone()));
        }
    }
    missing.sort_by(|a, b| a.0.cmp(&b.0));

    if missing.is_empty() {
        println!(
            "{} all {} STORY/BUG description(s) carry an acceptance section.",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            total_in_scope
        );
        return Ok(());
    }

    if !quiet {
        for (id, title) in &missing {
            println!(
                "{} {}  no `## Acceptance` / `## Verify` section  {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                id.bold(),
                title.dimmed()
            );
        }
    }
    println!(
        "{} of {} STORY/BUG descriptions missing acceptance criteria",
        format!("{}", missing.len()).bold(),
        total_in_scope
    );
    println!(
        "  ({})",
        "run `aida edit <id>` to add — STORY-67 will pick it up automatically".dimmed()
    );
    std::process::exit(1);
}

/// Walk every YAML in objects/, collect every `relationships[*].target_id`
/// reference, and verify each resolves to an existing req's UUID. Reports
/// dangling references, optionally repairs by stripping the bad entries.
///
/// TASK-58: rewrote the original line-scanner — which exited the
/// `relationships:` block on the first `- rel_type:` array entry
/// (any line starting with `-` matched the "exiting top-level key"
/// heuristic) and so never inspected any `target_id`. Now uses
/// `object_store::load_all_objects()` for proper serde-driven
// deserialization. trace:TASK-58 | ai:claude
fn doctor_verify_relationships(repair: bool, yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    let objects_root = store_path.join("objects");
    if !objects_root.exists() {
        println!("(no objects/ tree — nothing to check)");
        return Ok(());
    }

    let reqs = aida_core::object_store::load_all_objects(&objects_root)?;

    // First pass: collect every uuid present in the store + the spec
    // mapping for nicer dangling-edge reporting.
    let all_uuids: std::collections::HashSet<uuid::Uuid> = reqs.iter().map(|r| r.id).collect();
    let _spec_by_uuid: std::collections::HashMap<uuid::Uuid, String> = reqs
        .iter()
        .filter_map(|r| r.spec_id.as_ref().map(|s| (r.id, s.clone())))
        .collect();

    // Second pass: walk each req's relationships array and check each
    // target_id resolves to an existing uuid.
    #[derive(Debug)]
    struct Dangling {
        source_uuid: uuid::Uuid,
        source_spec: String,
        target_uuid: uuid::Uuid,
        rel_type: String,
    }
    let mut dangling: Vec<Dangling> = Vec::new();
    for req in &reqs {
        let source_spec = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
        for rel in &req.relationships {
            if !all_uuids.contains(&rel.target_id) {
                dangling.push(Dangling {
                    source_uuid: req.id,
                    source_spec: source_spec.clone(),
                    target_uuid: rel.target_id,
                    rel_type: format!("{:?}", rel.rel_type),
                });
            }
        }
    }

    if dangling.is_empty() {
        println!(
            "{} every relationship target resolves to an existing requirement.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    println!(
        "{} {} dangling relationship reference(s):",
        crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
        dangling.len()
    );
    println!();
    for d in &dangling {
        println!(
            "  {} → {}: target uuid {} not found",
            d.source_spec.bold(),
            d.rel_type.dimmed(),
            d.target_uuid.to_string().yellow()
        );
    }
    println!();

    if !repair {
        println!(
            "Run with {} to strip dangling references in-place.",
            "--repair".cyan()
        );
        std::process::exit(1);
    }

    if !yes {
        use std::io::Write;
        print!("Strip {} dangling reference(s)? [y/N] ", dangling.len());
        std::io::stdout().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Repair: filter dangling edges out of each affected req's
    // relationships array and rewrite the YAML. trace:TASK-58 | ai:claude
    let dangling_target_uuids: std::collections::HashSet<uuid::Uuid> =
        dangling.iter().map(|d| d.target_uuid).collect();
    let affected_source_uuids: std::collections::HashSet<uuid::Uuid> =
        dangling.iter().map(|d| d.source_uuid).collect();
    let mut fixed = 0usize;
    for mut req in reqs.into_iter() {
        if !affected_source_uuids.contains(&req.id) {
            continue;
        }
        let before = req.relationships.len();
        req.relationships
            .retain(|r| !dangling_target_uuids.contains(&r.target_id));
        if req.relationships.len() != before {
            aida_core::object_store::write_object(&store_path.join("objects"), &req)?;
            fixed += 1;
        }
    }
    let _ = aida_core::git_ops::add(&store_path, &["objects"]);
    let _ = aida_core::git_ops::commit(
        &store_path,
        &format!(
            "chore(repair): strip {} dangling relationship target(s)",
            dangling.len()
        ),
    );
    println!(
        "{} repaired {} requirement(s).",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        fixed
    );
    println!("  Push with: {}", "aida push".cyan());
    Ok(())
}

// Walk source files under the project root for `trace:<SPEC-ID>`
/// patterns and verify each spec_id resolves to a requirement in the
/// store. With `strip_dangling`, rewrites source files to remove the
// dangling trace markers. trace:EPIC-19 | ai:claude
fn doctor_validate_trace_comments(strip_dangling: bool, dry_run: bool, yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    let objects_root = store_path.join("objects");
    if !objects_root.exists() {
        println!("(no objects/ tree — nothing to check)");
        return Ok(());
    }

    // Collect every spec_id AND agreed_id from the store. A trace
    // comment is considered valid if it matches either form — the
    // spec_id is the original (pre-merge-gate) id and stays in trace
    // comments, while agreed_id is the canonical post-merge form.
    // trace:EPIC-19 | ai:claude
    let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
    walk_yamls(&objects_root, &mut yaml_files);
    let mut known_specs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for path in &yaml_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines() {
            let t = line.trim_start();
            if let Some(v) = t.strip_prefix("spec_id:") {
                let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                if !s.is_empty() {
                    known_specs.insert(s);
                }
            } else if let Some(v) = t.strip_prefix("agreed_id:") {
                let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                // agreed_id can be `null` / empty in YAML; skip those.
                if !s.is_empty() && s != "null" && s != "~" {
                    known_specs.insert(s);
                }
            }
        }
    }

    // Collect every trace comment in the project tree.
    let trace_re = regex::Regex::new(r"trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)").unwrap();
    let mut by_spec: std::collections::HashMap<String, Vec<(std::path::PathBuf, usize)>> =
        std::collections::HashMap::new();

    walk_source_for_traces(&project_root, &trace_re, &mut by_spec);

    let mut orphan_specs: Vec<&String> = by_spec
        .keys()
        .filter(|s| !known_specs.contains(*s))
        .collect();
    orphan_specs.sort();

    if orphan_specs.is_empty() {
        println!(
            "{} every `trace:<SPEC-ID>` in source resolves to a requirement ({} unique spec_ids referenced from {} location(s)).",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            by_spec.len(),
            by_spec.values().map(|v| v.len()).sum::<usize>()
        );
        return Ok(());
    }

    println!(
        "{} {} trace comment(s) reference unknown spec_ids:",
        crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
        orphan_specs.len()
    );
    println!();
    for spec in &orphan_specs {
        let locations = by_spec.get(*spec).unwrap();
        println!("{}: ({} reference(s))", spec.bold(), locations.len());
        for (path, line) in locations.iter().take(5) {
            let rel = path.strip_prefix(&project_root).unwrap_or(path);
            println!("  {}:{}", rel.display(), line);
        }
        if locations.len() > 5 {
            println!("  … and {} more", locations.len() - 5);
        }
        println!();
    }
    if !strip_dangling {
        println!("Likely causes: req was deleted, or a typo. Either delete the");
        println!("trace comment or update it to reference an existing spec_id.");
        println!();
        println!(
            "To strip these in-place: {}",
            "aida doctor validate-trace-comments --strip-dangling".cyan()
        );
        std::process::exit(1);
    }

    // --- strip-dangling path ---
    let dangling_set: std::collections::HashSet<String> =
        orphan_specs.iter().map(|s| (*s).clone()).collect();

    println!(
        "{} {} reference(s) across {} unique spec_ids will be stripped.",
        "Plan:".yellow().bold(),
        by_spec
            .iter()
            .filter(|(s, _)| dangling_set.contains(*s))
            .map(|(_, v)| v.len())
            .sum::<usize>(),
        dangling_set.len()
    );

    if dry_run {
        println!();
        let stats = strip_dangling_traces(&project_root, &dangling_set, true)?;
        println!(
            "→ dry-run: would delete {} whole line(s) and modify {} other line(s) across {} file(s).",
            stats.lines_deleted, stats.lines_modified, stats.files_changed
        );
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        print!("Strip dangling trace annotations from source files? [y/N] ");
        std::io::stdout().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let stats = strip_dangling_traces(&project_root, &dangling_set, false)?;
    println!(
        "{} stripped {} line(s) (deleted {} whole, modified {} mixed) across {} file(s).",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        stats.lines_deleted + stats.lines_modified,
        stats.lines_deleted,
        stats.lines_modified,
        stats.files_changed
    );
    println!("  Review the diff: {}", "git diff".cyan());
    Ok(())
}

#[derive(Default)]
struct StripStats {
    files_changed: usize,
    lines_deleted: usize,
    lines_modified: usize,
}

/// Walk every text source file under `root` and either delete or modify
// any line containing `trace:<DANGLING>` per the dangling_ids set.
// When `dry_run`, returns counts without writing. trace:EPIC-19
fn strip_dangling_traces(
    root: &std::path::Path,
    dangling_ids: &std::collections::HashSet<String>,
    dry_run: bool,
) -> Result<StripStats> {
    let mut stats = StripStats::default();
    strip_dangling_walk(root, dangling_ids, dry_run, &mut stats);
    Ok(stats)
}

fn strip_dangling_walk(
    root: &std::path::Path,
    dangling_ids: &std::collections::HashSet<String>,
    dry_run: bool,
    stats: &mut StripStats,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if path.is_dir() {
            if matches!(
                name,
                ".git"
                    | ".aida-store"
                    | ".aida"
                    | "target"
                    | "node_modules"
                    | "dist"
                    | "build"
                    | ".cache"
                    | ".venv"
                    | "venv"
            ) {
                continue;
            }
            strip_dangling_walk(&path, dangling_ids, dry_run, stats);
            continue;
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let probably_text = matches!(
            ext,
            "rs" | "py"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "go"
                | "java"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "cs"
                | "rb"
                | "sh"
                | "md"
                | "toml"
                | "yaml"
                | "yml"
                | "json"
        );
        if !probably_text {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Quick scan: any dangling id present?
        let mut had_match = false;
        for id in dangling_ids {
            if content.contains(&format!("trace:{}", id)) {
                had_match = true;
                break;
            }
        }
        if !had_match {
            continue;
        }

        let (new_content, deleted, modified) = rewrite_strip_dangling(&content, dangling_ids);
        if new_content == content {
            continue;
        }
        stats.files_changed += 1;
        stats.lines_deleted += deleted;
        stats.lines_modified += modified;
        if !dry_run {
            let _ = std::fs::write(&path, new_content);
        }
    }
}

/// Pure transform: take a file's content and the dangling-id set,
/// return (new_content, lines_deleted, lines_modified). A line is
/// "deleted" when the only meaningful content was the trace marker
/// (post-strip, only a comment marker remains); otherwise the trace
// fragment is excised and the line is "modified". trace:EPIC-19
fn rewrite_strip_dangling(
    content: &str,
    dangling_ids: &std::collections::HashSet<String>,
) -> (String, usize, usize) {
    use regex::Regex;
    // Match `trace:<ID> | ai:<tool>(:<conf>)?` fragments. Capture the
    // id so we can check it against the dangling set.
    let frag_re =
        Regex::new(r"trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)\s*\|\s*ai:[a-zA-Z]+(?::(?:high|med|low))?")
            .unwrap();

    let mut out = String::with_capacity(content.len());
    let mut deleted = 0;
    let mut modified = 0;

    for line in content.lines() {
        // Does this line contain a dangling trace?
        let mut should_strip = false;
        for cap in frag_re.captures_iter(line) {
            if let Some(m) = cap.get(1) {
                if dangling_ids.contains(m.as_str()) {
                    should_strip = true;
                    break;
                }
            }
        }
        if !should_strip {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Strip every dangling trace fragment from this line.
        let stripped = frag_re
            .replace_all(line, |caps: &regex::Captures| {
                let id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                if dangling_ids.contains(id) {
                    String::new()
                } else {
                    caps.get(0)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default()
                }
            })
            .into_owned();

        // Decide: delete the whole line if what remains is just a
        // comment marker (no content), or modify (keep the line with
        // the fragment removed).
        let trimmed = stripped.trim();
        let is_just_marker = matches!(trimmed, "" | "//" | "///" | "//!" | "/*" | "*/" | "*" | "#")
            || trimmed.starts_with("// ") && trimmed.trim_end_matches(' ').len() <= 3;

        if is_just_marker {
            deleted += 1;
            // skip — don't push this line
        } else {
            modified += 1;
            // Clean up double-spaces left behind by the strip.
            let cleaned = stripped.replace("  ", " ").trim_end().to_string();
            out.push_str(&cleaned);
            out.push('\n');
        }
    }

    // Preserve trailing newline behavior (str.lines() drops it; if the
    // original content ended without a newline, drop our trailing one).
    if !content.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }

    (out, deleted, modified)
}

/// Recursively collect every `*.yaml` file under `root` into `out`.
/// Hand-rolled to avoid adding a walkdir dep just for the doctor ops.
/// The orphan store's objects/ tree is shallow (3 levels) so a simple
// recursive read_dir is fine. trace:EPIC-19 | ai:claude
fn walk_yamls(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_yamls(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
            out.push(path);
        }
    }
}

/// Mark blocks whose owner isn't in nodes.toml as exhausted, so the
/// dispenser skips them but their range stays reserved (so other
/// clones don't reallocate the same numbers and create a real
// collision). trace:EPIC-19 | ai:claude
fn doctor_repair_stale_blocks(dry_run: bool, yes: bool) -> Result<()> {
    use aida_core::{BlockRegistry, NodeRegistry};

    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    let blocks_path = store_path.join("registry").join("blocks.yaml");
    let nodes_path = store_path.join("registry").join("nodes.toml");

    if !blocks_path.exists() {
        println!("(no blocks.yaml — nothing to repair)");
        return Ok(());
    }

    let mut blocks = BlockRegistry::load(&blocks_path).unwrap_or_default();
    let nodes = NodeRegistry::load(&nodes_path).unwrap_or_default();
    let registered: std::collections::HashSet<String> =
        nodes.nodes.iter().map(|n| n.id.clone()).collect();

    let stale: Vec<(usize, &aida_core::AgreedIdBlock)> = blocks
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| !registered.contains(&b.node_id) && !b.is_exhausted())
        .collect();

    if stale.is_empty() {
        println!(
            "{} no stale blocks — every active block has a registered node.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    println!("{}", "Stale blocks (owner not in nodes.toml)".bold());
    for (_, b) in &stale {
        println!(
            "  {} node `{}` owns {}-{}..{} (next={})",
            "·".dimmed(),
            b.node_id,
            b.type_prefix,
            b.range_start,
            b.range_end,
            b.next
        );
    }
    println!();
    println!("Plan: bump each block's `next` past `range_end` so the dispenser");
    println!("      skips it. The range stays reserved (preserves cross-clone safety).");
    println!();

    if dry_run {
        println!("{} dry-run — no changes written.", "→".cyan());
        return Ok(());
    }
    if !yes {
        use std::io::Write;
        print!("Tombstone {} stale block(s)? [y/N] ", stale.len());
        std::io::stdout().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let stale_indices: Vec<usize> = stale.iter().map(|(i, _)| *i).collect();
    let count = stale_indices.len();
    for idx in stale_indices {
        let b = &mut blocks.blocks[idx];
        b.next = b.range_end + 1;
    }
    blocks.save(&blocks_path)?;
    let _ = aida_core::git_ops::add(&store_path, &["registry/blocks.yaml"]);
    let _ = aida_core::git_ops::commit(
        &store_path,
        &format!(
            "chore(registry): tombstone {} stale block(s) (no node owner)",
            count
        ),
    );

    println!(
        "{} tombstoned {} block(s).",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        count
    );
    println!("  Push with: {}", "aida push".cyan());
    Ok(())
}

/// Walk the orphan store's objects tree, group requirements by their
/// `spec_id` field, and report any spec_id claimed by more than one
/// requirement. v1 reports only — auto-renumber is dangerous (would
// orphan trace comments + commit refs). trace:EPIC-19 | ai:claude
// why: the one-off `collisions` borrow-tuple is local to this reporter; a named alias used in a single spot would obscure more than the inline type.
#[allow(clippy::type_complexity)]
fn doctor_scrub_collisions() -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    let objects_root = store_path.join("objects");
    if !objects_root.exists() {
        println!("(no objects/ tree — nothing to check)");
        return Ok(());
    }

    let mut by_spec: std::collections::HashMap<String, Vec<(String, String, String)>> =
        std::collections::HashMap::new();

    let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
    walk_yamls(&objects_root, &mut yaml_files);
    for path in &yaml_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut spec_id = String::new();
        let mut uuid = String::new();
        let mut title = String::new();
        for raw in content.lines() {
            let line = raw.trim_start();
            if let Some(v) = line.strip_prefix("spec_id:") {
                spec_id = v.trim().trim_matches('"').trim_matches('\'').to_string();
            } else if let Some(v) = line.strip_prefix("id:") {
                if uuid.is_empty() {
                    uuid = v.trim().trim_matches('"').trim_matches('\'').to_string();
                }
            } else if let Some(v) = line.strip_prefix("title:") {
                if title.is_empty() {
                    title = v.trim().trim_matches('"').trim_matches('\'').to_string();
                }
            }
            if !spec_id.is_empty() && !uuid.is_empty() && !title.is_empty() {
                break;
            }
        }
        if spec_id.is_empty() {
            continue;
        }
        by_spec
            .entry(spec_id)
            .or_default()
            .push((uuid, title, path.display().to_string()));
    }

    let mut collisions: Vec<(&String, &Vec<(String, String, String)>)> = by_spec
        .iter()
        .filter(|(_, entries)| entries.len() > 1)
        .collect();
    collisions.sort_by(|a, b| a.0.cmp(b.0));

    if collisions.is_empty() {
        println!(
            "{} no spec_id collisions — every spec_id maps to exactly one requirement.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    println!(
        "{} {} spec_id collision(s) found:",
        crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
        collisions.len()
    );
    println!();
    for (spec, entries) in &collisions {
        println!("{}:", spec.bold());
        for (uuid, title, path) in entries.iter() {
            let title_disp = if title.is_empty() {
                "(no title)".dimmed().to_string()
            } else {
                title.clone()
            };
            println!("  {} {} — {}", uuid.yellow(), title_disp, path.dimmed());
        }
        println!();
    }
    println!("Resolution (v1 is detect-only — auto-renumber would orphan trace comments):");
    println!("  - Decide which UUID is canonical for each spec_id");
    println!("  - For the others, edit their YAML directly to set a fresh spec_id, or");
    println!("    delete their YAML if duplicates");
    println!();
    std::process::exit(1);
}

/// Compose every diagnostic into a single report. Exits non-zero on any
// problem so it can gate CI. trace:EPIC-19 | ai:claude
fn doctor_fsck() -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");

    println!("{}", "AIDA fsck".bold());
    println!("  project root:  {}", project_root.display());
    println!("  store path:    {}", store_path.display());
    println!();

    let mut had_problem = false;

    // --- Check 1: block registry consistency (FR-281's logic, inline) ---
    println!("{}", "── block registry ──".bold());
    use aida_core::{BlockRegistry, NodeRegistry};
    let blocks_path = store_path.join("registry").join("blocks.yaml");
    let nodes_path = store_path.join("registry").join("nodes.toml");
    if blocks_path.exists() {
        let blocks = BlockRegistry::load(&blocks_path).unwrap_or_default();
        let nodes = NodeRegistry::load(&nodes_path).unwrap_or_default();
        let registered: std::collections::HashSet<String> =
            nodes.nodes.iter().map(|n| n.id.clone()).collect();
        // Only ACTIVE (non-exhausted) blocks count — tombstoned blocks
        // are explicitly retired (next > range_end) and no longer
        // dispense, so an unregistered owner on a tombstoned block is
        // expected (it's the post-repair state).
        let block_owners: std::collections::HashSet<String> = blocks
            .blocks
            .iter()
            .filter(|b| !b.is_exhausted())
            .map(|b| b.node_id.clone())
            .collect();
        let orphan_blocks: Vec<&str> = block_owners
            .iter()
            .filter(|id| !registered.contains(*id))
            .map(|s| s.as_str())
            .collect();
        if orphan_blocks.is_empty() {
            println!(
                "  {} every active block has a registered node owner.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            had_problem = true;
            println!(
                "  {} {} block-owning node(s) not in nodes.toml: {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                orphan_blocks.len(),
                orphan_blocks.join(", ")
            );
            println!("    fix: {}", "aida doctor repair-stale-blocks".cyan());
        }
    } else {
        println!(
            "  {} no blocks.yaml — skipping (project may be node-aware-only).",
            "·".dimmed()
        );
    }
    println!();

    // --- Check 2: spec_id collisions ---
    println!("{}", "── spec_id collisions ──".bold());
    let objects_root = store_path.join("objects");
    if objects_root.exists() {
        let mut by_spec: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
        walk_yamls(&objects_root, &mut yaml_files);
        for path in &yaml_files {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let t = line.trim_start();
                    if let Some(v) = t.strip_prefix("spec_id:") {
                        let spec = v.trim().trim_matches('"').trim_matches('\'').to_string();
                        if !spec.is_empty() {
                            *by_spec.entry(spec).or_default() += 1;
                            break;
                        }
                    }
                }
            }
        }
        let collisions: Vec<&String> = by_spec
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(spec, _)| spec)
            .collect();
        if collisions.is_empty() {
            println!(
                "  {} every spec_id maps to one requirement.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            had_problem = true;
            println!(
                "  {} {} spec_id(s) claimed by multiple requirements: {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                collisions.len(),
                collisions
                    .iter()
                    .take(8)
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!("    fix: {}", "aida doctor scrub-collisions".cyan());
        }
    } else {
        println!("  {} no objects/ — skipping.", "·".dimmed());
    }
    println!();

    // --- Check 3: cache freshness ---
    println!("{}", "── cache freshness ──".bold());
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    if cache_path.exists() {
        // Simple heuristic: cache exists. Detailed staleness check
        // requires reading cache HEAD — defer to `aida cache status`.
        println!(
            "  {} cache exists at {} (run `aida cache status` for HEAD-vs-store check).",
            "·".dimmed(),
            cache_path.display()
        );
    } else {
        println!(
            "  {} cache missing — run `aida cache rebuild` if list/search are slow.",
            "·".dimmed()
        );
    }
    println!();

    // --- Check 4: relationship targets resolve ---
    println!("{}", "── relationships ──".bold());
    if objects_root.exists() {
        let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
        walk_yamls(&objects_root, &mut yaml_files);
        let mut all_uuids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for path in &yaml_files {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let t = line.trim_start();
                    if let Some(v) = t.strip_prefix("id:") {
                        let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                        if !s.is_empty() {
                            all_uuids.insert(s);
                        }
                        break;
                    }
                }
            }
        }
        let mut dangling = 0usize;
        for path in &yaml_files {
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            let mut in_rel = false;
            for raw in content.lines() {
                let trimmed = raw.trim_start();
                if !raw.starts_with(' ') && trimmed.starts_with("relationships:") {
                    in_rel = true;
                    continue;
                }
                if in_rel && !raw.starts_with(' ') && !trimmed.is_empty() && trimmed.contains(':') {
                    in_rel = false;
                    continue;
                }
                if !in_rel {
                    continue;
                }
                if let Some(v) = trimmed.strip_prefix("target_id:") {
                    let target = v.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !target.is_empty() && !all_uuids.contains(&target) {
                        dangling += 1;
                    }
                }
            }
        }
        if dangling == 0 {
            println!(
                "  {} every relationship target resolves.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            had_problem = true;
            println!(
                "  {} {} dangling relationship reference(s).",
                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                dangling
            );
            println!(
                "    fix: {}",
                "aida doctor verify-relationships --repair".cyan()
            );
        }
    } else {
        println!("  {} no objects/ — skipping.", "·".dimmed());
    }
    println!();

    // --- Check 5: trace comments resolve to existing reqs (informational) ---
    // Slow-ish (walks source tree) and the failure mode (dangling traces
    // from renumbered/deleted reqs) is rarely urgent — keep it
    // non-blocking so fsck can serve as a CI gate without a perpetual
    // false-fail. trace:EPIC-19 | ai:claude
    println!("{}", "── trace comments ──".bold());
    if objects_root.exists() {
        let trace_re =
            regex::Regex::new(r"trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)").unwrap();
        let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
        walk_yamls(&objects_root, &mut yaml_files);
        let mut known_specs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for path in &yaml_files {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let t = line.trim_start();
                    if let Some(v) = t.strip_prefix("spec_id:") {
                        let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                        if !s.is_empty() {
                            known_specs.insert(s);
                        }
                    } else if let Some(v) = t.strip_prefix("agreed_id:") {
                        let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                        if !s.is_empty() && s != "null" && s != "~" {
                            known_specs.insert(s);
                        }
                    }
                }
            }
        }
        let mut by_spec: std::collections::HashMap<String, Vec<(std::path::PathBuf, usize)>> =
            std::collections::HashMap::new();
        walk_source_for_traces(&project_root, &trace_re, &mut by_spec);
        let total_refs: usize = by_spec.values().map(|v| v.len()).sum();
        let dangling: usize = by_spec
            .iter()
            .filter(|(s, _)| !known_specs.contains(*s))
            .map(|(_, v)| v.len())
            .sum();
        let dangling_specs: usize = by_spec.keys().filter(|s| !known_specs.contains(*s)).count();
        if dangling == 0 {
            println!(
                "  {} {} unique spec_ids referenced from {} location(s); all resolve.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                by_spec.len(),
                total_refs
            );
        } else {
            // Informational, not a failure — see comment above.
            println!(
                "  {} {} unique spec_ids referenced from {} location(s); {} reference(s) ({} unique spec_ids) dangling.",
                "·".yellow(),
                by_spec.len(),
                total_refs,
                dangling,
                dangling_specs
            );
            println!(
                "    detail: {}",
                "aida doctor validate-trace-comments".cyan()
            );
        }
    } else {
        println!("  {} no objects/ — skipping.", "·".dimmed());
    }
    println!();

    // --- Check 6: counter_scope sanity (warn if config + blocks disagree) ---
    println!("{}", "── counter_scope ──".bold());
    let scope = read_id_counter_scope(&project_root);
    let has_global_block = blocks_path.exists()
        && BlockRegistry::load(&blocks_path)
            .map(|br| {
                br.blocks
                    .iter()
                    .any(|b| b.type_prefix == aida_core::IdCounterScope::GLOBAL_TYPE_PREFIX)
            })
            .unwrap_or(false);
    match (scope, has_global_block) {
        (aida_core::IdCounterScope::Global, true) => {
            println!(
                "  {} config=global, blocks have a `*` block. Consistent.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        }
        (aida_core::IdCounterScope::Global, false) => {
            had_problem = true;
            println!(
                "  {} config says global but no `*` block exists. New `aida add` would fall back to per-type.",
                crate::glyph(crate::glyphs::Glyph::Cross).red()
            );
            println!(
                "    fix: {}",
                "aida doctor migrate-counter-scope --to global".cyan()
            );
        }
        (aida_core::IdCounterScope::PerType, true) => {
            println!(
                "  {} config=per-type, but a `*` block exists (mid-migration?). Consider running `migrate-counter-scope --to global`.",
                "·".yellow()
            );
        }
        (aida_core::IdCounterScope::PerType, false) => {
            println!(
                "  {} config=per-type, no `*` block. Consistent.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        }
    }
    println!();

    // --- Check 7: legacy-store cruft on a git-canonical project ---
    // TASK-752: tracked requirements*.yaml / scaffold-report.html while the live
    // store is the orphan aida-store branch. The detector self-gates on
    // distributed mode + orphan branch, so this is silent on a legacy
    // --centralized project. A finding is a problem (non-zero exit) so fsck can
    // gate CI; `aida doctor --heal --category legacy-store-cruft` resolves it.
    // trace:TASK-752 | ai:claude
    println!("{}", "── legacy-store cruft ──".bold());
    let cruft = detect_legacy_store_cruft(&project_root);
    if cruft.is_empty() {
        println!(
            "  {} no tracked legacy-store artifacts (or not a git-canonical project).",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        had_problem = true;
        println!(
            "  {} {} tracked legacy-store artifact(s) on a git-canonical project: {}",
            crate::glyph(crate::glyphs::Glyph::Cross).red(),
            cruft.len(),
            cruft.join(", ")
        );
        println!(
            "    fix: {}",
            "aida doctor --heal --category legacy-store-cruft".cyan()
        );
    }
    println!();

    // --- Check 8: per-clone runtime files tracked on the orphan aida-store branch ---
    // BUG-563: `.aida/node.toml` / dispenser.toml / *.lock / cache.db* tracked on
    // the orphan branch make every cross-clone store-leg rebase conflict forever.
    // The detector self-gates on distributed mode + an attached store worktree.
    // A finding is a problem (non-zero exit) so fsck can gate CI;
    // `aida doctor --heal --category store-tracked-runtime` resolves it.
    // trace:BUG-563 | ai:claude
    println!("{}", "── store-tracked runtime ──".bold());
    let runtime_cruft = detect_store_tracked_runtime(&project_root);
    if runtime_cruft.is_empty() {
        println!(
            "  {} no per-clone runtime files tracked on the orphan aida-store branch (or not a git-canonical project).",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        had_problem = true;
        println!(
            "  {} {} per-clone runtime file(s) tracked on the orphan aida-store branch: {}",
            crate::glyph(crate::glyphs::Glyph::Cross).red(),
            runtime_cruft.len(),
            runtime_cruft.join(", ")
        );
        println!(
            "    fix: {}",
            "aida doctor --heal --category store-tracked-runtime".cyan()
        );
    }
    println!();

    if had_problem {
        println!("{}", "fsck found problems — see above.".red().bold());
        std::process::exit(1);
    } else {
        println!(
            "{}",
            format!("{} fsck clean.", crate::glyph(crate::glyphs::Glyph::Check))
                .green()
                .bold()
        );
    }
    Ok(())
}

fn doctor_migrate_counter_scope(
    to: &str,
    dry_run: bool,
    yes: bool,
    new_block_size: u32,
) -> Result<()> {
    use aida_core::BlockRegistry;

    if to != "global" {
        anyhow::bail!("only `--to global` is supported today (per-type → global)");
    }

    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    let blocks_path = store_path.join("registry").join("blocks.yaml");
    let config_path = project_root.join(".aida").join("config.toml");

    if !blocks_path.exists() {
        anyhow::bail!(
            "no blocks.yaml at {} — nothing to migrate",
            blocks_path.display()
        );
    }
    if !config_path.exists() {
        anyhow::bail!(
            "no config.toml at {} — is this an AIDA project?",
            config_path.display()
        );
    }

    let current_scope = read_id_counter_scope(&project_root);
    if current_scope == aida_core::IdCounterScope::Global {
        println!(
            "{} already on global counter_scope — nothing to migrate.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    let mut registry = BlockRegistry::load(&blocks_path)?;
    if registry.blocks.is_empty() {
        anyhow::bail!("blocks.yaml is empty — no blocks to migrate from");
    }

    // Identify this clone's node id so the new `*` block belongs to it.
    let node_id = load_node_id(&store_path);
    let our_blocks: Vec<_> = registry
        .blocks
        .iter()
        .filter(|b| b.node_id == node_id && !b.is_exhausted())
        .cloned()
        .collect();
    if our_blocks.is_empty() {
        anyhow::bail!(
            "node {} has no active per-type blocks in blocks.yaml — \
             either already migrated, or this clone hasn't been initialized",
            node_id
        );
    }

    // The new `*` block starts strictly above the highest range_end across
    // ALL blocks (any node, any type) so we never collide with another
    // clone's range. Then size more on top.
    let highest_end: u32 = registry
        .blocks
        .iter()
        .map(|b| b.range_end)
        .max()
        .unwrap_or(0);
    let new_start = highest_end + 1;
    let new_end = new_start + new_block_size - 1;

    println!("{}", "Migration plan: per-type → global".bold());
    println!("  node:                {}", node_id);
    println!("  per-type blocks to retire (mark exhausted):");
    for b in &our_blocks {
        println!(
            "    - {} {}-{}..{} (next was {})",
            "·".dimmed(),
            b.type_prefix,
            b.range_start,
            b.range_end,
            b.next
        );
    }
    println!(
        "  new global block:    *-{}..{} (size {}) for node {}",
        new_start, new_end, new_block_size, node_id
    );
    println!("  config write:        [id_format] counter_scope = \"global\"");
    println!();
    println!("After this migration:");
    println!("  - existing requirement spec_ids stay UNCHANGED");
    println!(
        "  - new requirements use the global counter (FR-{}, BUG-{}, etc.)",
        new_start,
        new_start + 1
    );
    println!("  - the retired per-type blocks remain in blocks.yaml as history");
    println!();

    if dry_run {
        println!("{} dry-run — no changes written.", "→".cyan());
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        print!("Proceed? [y/N] ");
        std::io::stdout().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Apply: mark our per-type blocks exhausted (next = range_end + 1) so
    // the dispenser skips them. Then append the new `*` block.
    for b in registry.blocks.iter_mut() {
        if b.node_id == node_id && !b.is_exhausted() {
            b.next = b.range_end + 1;
        }
    }
    let owner = aida_core::git_ops::git_config_get("user.email")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_string());
    registry.claim_block_with_floor(
        node_id.clone(),
        owner,
        hostname(),
        aida_core::IdCounterScope::GLOBAL_TYPE_PREFIX.to_string(),
        new_block_size,
        highest_end,
    );
    registry.save(&blocks_path)?;

    // Update config.toml — preserve the file by line-rewriting; if
    // counter_scope already exists (it shouldn't given the early check),
    // overwrite. Otherwise append after the [id_format] section.
    update_config_counter_scope(&config_path, "global")?;

    // Stage + commit the registry change. The lease symlink in the
    // session worktree means `git -C <store_path>` operates on the
    // shared orphan branch.
    let _ = aida_core::git_ops::add(&store_path, &["registry/blocks.yaml"]);
    let _ = aida_core::git_ops::commit(
        &store_path,
        &format!(
            "chore(registry): migrate node {} to global counter (*-{}..{})",
            node_id, new_start, new_end
        ),
    );

    println!();
    println!(
        "{} migration complete.",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold()
    );
    println!(
        "  new global block: {}",
        format!("*-{}..{}", new_start, new_end).cyan()
    );
    println!(
        "  next `aida add` will dispense {}",
        format!("<TYPE>-{}", new_start).cyan()
    );
    println!();
    println!("Don't forget to push:");
    println!("  {}", "aida push".cyan());
    Ok(())
}

/// Update the `[id_format] counter_scope` value in config.toml. Adds the
/// line if missing, replaces it in-place if present. Preserves the rest
/// of the file (comments, other keys, formatting).
fn update_config_counter_scope(config_path: &std::path::Path, new_value: &str) -> Result<()> {
    let content = std::fs::read_to_string(config_path)?;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut in_id_format = false;
    let mut last_id_format_line: Option<usize> = None;
    let mut replaced = false;
    for (i, line) in lines.iter_mut().enumerate() {
        let trimmed_owned: String = line.trim().to_string();
        if trimmed_owned.starts_with('[') {
            in_id_format = trimmed_owned == "[id_format]";
            if in_id_format {
                last_id_format_line = Some(i);
            }
            continue;
        }
        if in_id_format && trimmed_owned.starts_with("counter_scope") {
            *line = format!("counter_scope = \"{}\"", new_value);
            replaced = true;
        }
        if in_id_format && !trimmed_owned.is_empty() && !trimmed_owned.starts_with('#') {
            last_id_format_line = Some(i);
        }
    }
    if !replaced {
        // Insert after the last line of the [id_format] section.
        let insert_at = last_id_format_line.map(|i| i + 1);
        let new_line = format!("counter_scope = \"{}\"", new_value);
        match insert_at {
            Some(idx) => lines.insert(idx, new_line),
            None => {
                // No [id_format] section found — append both header and value.
                lines.push(String::new());
                lines.push("[id_format]".to_string());
                lines.push(new_line);
            }
        }
    }
    std::fs::write(config_path, lines.join("\n") + "\n")?;
    Ok(())
}
