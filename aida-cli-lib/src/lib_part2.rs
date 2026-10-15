/// Handle requirement type commands
/// Handle `aida cache {rebuild,status}` against a CachedGitBackend.
/// trace:EPIC-1-001 | ai:claude
/// Handle `aida db block <subcommand>` — pre-allocated agreed ID blocks.
// trace:FR-2-005 | ai:claude
/// One-shot migration: collapse legacy origin spec_ids onto their agreed_ids.
/// For each requirement where spec_id ≠ agreed_id, set spec_id := agreed_id
/// and clear agreed_id (since the canonical id now lives in spec_id alone).
/// The on-disk YAML file moves from `objects/<TYPE>/000/<OLD>.yaml` to
/// `objects/<TYPE>/000/<NEW>.yaml`; that's handled implicitly by save()'s
/// "delete files that fell out of the store" pass.
///
/// Relationships use UUIDs internally so they're unaffected.
/// trace:FR-1-071 | ai:claude
pub(crate) fn handle_retire_legacy_ids(
    backend: &aida_core::CachedGitBackend,
    _store_path: &std::path::Path,
    dry_run: bool,
) -> Result<()> {
    use aida_core::DatabaseBackend;

    let store = backend.load()?;

    // Collect the rename plan first so we can preview before mutating.
    let mut renames: Vec<(String, String, uuid::Uuid)> = Vec::new();
    for req in &store.requirements {
        if let (Some(spec), Some(agreed)) = (req.spec_id.as_deref(), req.agreed_id.as_deref()) {
            if spec != agreed {
                renames.push((spec.to_string(), agreed.to_string(), req.id));
            }
        }
    }

    if renames.is_empty() {
        println!(
            "{} No legacy ids to retire — every requirement's spec_id already matches its agreed_id.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    println!(
        "Found {} requirement{} with diverging spec_id ↔ agreed_id:",
        renames.len(),
        if renames.len() == 1 { "" } else { "s" }
    );
    println!("{}", "─".repeat(60));
    let preview_limit = 20;
    for (spec, agreed, _) in renames.iter().take(preview_limit) {
        println!("  {}  →  {}", spec.dimmed(), agreed.bold());
    }
    if renames.len() > preview_limit {
        println!(
            "  {}",
            format!("… and {} more", renames.len() - preview_limit).dimmed()
        );
    }

    if dry_run {
        println!();
        println!("{} Dry run — no changes made.", "→".cyan());
        println!("    Re-run without --dry-run to apply.");
        return Ok(());
    }

    // Apply: load the mutable store, update each affected req, save once.
    // The save() path (post-BUG-1-040) writes only changed YAMLs and deletes
    // files that fell out of the store, so renames are handled automatically.
    let mut mutable = store;
    for req in mutable.requirements.iter_mut() {
        let needs = match (req.spec_id.as_deref(), req.agreed_id.as_deref()) {
            (Some(s), Some(a)) if s != a => Some(a.to_string()),
            _ => None,
        };
        if let Some(new_spec) = needs {
            req.spec_id = Some(new_spec);
            req.agreed_id = None;
        }
    }
    backend.save(&mutable)?;

    println!();
    println!(
        "{} Retired {} legacy spec_id{}. Canonical id now lives in spec_id alone.",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        renames.len(),
        if renames.len() == 1 { "" } else { "s" }
    );
    Ok(())
}

/// TASK-589: assemble the discipline-pack glossary from the binary's embedded
/// templates — the machinery glossary and/or the lifecycle vocabulary. Reads
/// the embedded copy (not the project's scaffolded files), so it is correct
/// even when a project's `.aida/discipline/` is missing or stale. With
/// neither flag (or both) it returns both sections, machinery first.
/// trace:TASK-589 | ai:claude
pub(crate) fn render_discipline_glossary(machinery: bool, lifecycle: bool) -> Result<String> {
    let templates = aida_core::get_embedded_templates();
    let embedded = |key: &str| -> Result<String> {
        templates
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.content.clone())
            .ok_or_else(|| anyhow::anyhow!("embedded glossary `{key}` not found in this binary"))
    };
    // Neither flag (default) or both flags → show both sections.
    let both = machinery == lifecycle;
    let show_machinery = both || machinery;
    let show_lifecycle = both || lifecycle;

    let mut out = String::new();
    if show_machinery {
        out.push_str(&embedded(".aida/discipline/machinery-glossary.md")?);
    }
    if show_machinery && show_lifecycle {
        out.push_str("\n\n");
    }
    if show_lifecycle {
        out.push_str(&embedded(".aida/discipline/lifecycle-vocabulary.md")?);
    }
    Ok(out)
}

/// Pure selection for the release-time doc-coverage gate (TASK-680).
///
/// Returns the requirements that **reached Completed at or after `cutoff`** and
/// have **no Doc entry referencing them** (no `Doc`-typed requirement carries a
/// `References` edge pointing at them). When `cutoff` is `None`, every spec that
/// ever reached Completed is considered (full-history scan — used when the repo
/// has no tags yet).
///
/// "Reached Completed" is read from the spec's transition `history`: any
/// `HistoryEntry` whose `changes` include a `status` field whose `new_value`
/// is `Completed`. As a fallback for specs that are currently Completed but
/// carry no such history row (e.g. imported/legacy data), the spec's
/// `modified_at` is used as the transition time.
///
/// Doc-typed requirements and archived specs are never themselves reported as
/// gaps (a doc doesn't need a doc about it).
///
/// trace:TASK-680 | ai:claude
pub(crate) fn find_uncovered_completed_specs(
    requirements: &[aida_core::models::Requirement],
    cutoff: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<aida_core::models::Requirement> {
    use aida_core::models::{RelationshipType, RequirementStatus, RequirementType};

    // Set of spec uuids that at least one Doc References.
    let mut documented: std::collections::HashSet<uuid::Uuid> = std::collections::HashSet::new();
    for doc in requirements
        .iter()
        .filter(|r| r.req_type == RequirementType::Doc)
    {
        for rel in &doc.relationships {
            if rel.rel_type == RelationshipType::References {
                documented.insert(rel.target_id);
            }
        }
    }

    let completed_str = RequirementStatus::Completed.to_string();

    let mut gaps: Vec<aida_core::models::Requirement> = Vec::new();
    for req in requirements {
        // Docs and archived specs are out of scope.
        if req.req_type == RequirementType::Doc || req.archived {
            continue;
        }

        // When did this spec reach Completed (if ever)?
        let completed_at: Option<chrono::DateTime<chrono::Utc>> = req
            .history
            .iter()
            .filter(|h| {
                h.changes
                    .iter()
                    .any(|c| c.field_name == "status" && c.new_value == completed_str)
            })
            .map(|h| h.timestamp)
            .max()
            .or_else(|| {
                // Fallback: currently Completed but no history row recording it.
                if req.status == RequirementStatus::Completed {
                    Some(req.modified_at)
                } else {
                    None
                }
            });

        let Some(reached_at) = completed_at else {
            continue;
        };

        // Inside the release window?
        if let Some(c) = cutoff {
            if reached_at < c {
                continue;
            }
        }

        // Already documented?
        if documented.contains(&req.id) {
            continue;
        }

        gaps.push(req.clone());
    }

    gaps.sort_by_key(|a| a.display_id());
    gaps
}

#[cfg(test)]
#[path = "tests/glossary_render_tests.rs"]
mod glossary_render_tests;

// ----------------------------------------------------------------------------
// `aida dev` — developer-only commands: pyenv-style activate of an in-repo
// build, foreground supervisor for aida-server + vite, shell-init helpers.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// `aida help-all` — full command inventory grouped by topic. Includes the
// commands that are #[clap(hide = true)] in the default `aida --help`.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

// ----------------------------------------------------------------------------
// `aida role` — persistent personas / hats. State lives at
// <project>/.aida/roles/<name>.toml. Resume by name to restore working
// directory and surface the role in the statusline.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

pub(crate) const ACTIVITY_MAX: usize = 10;

impl std::fmt::Display for MailboxResolveFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { query } => write!(f, "message not found: {query}"),
            Self::AmbiguousPrefix { query, candidates } => write!(
                f,
                "message id prefix is ambiguous: {query}; lengthen the prefix to select one of: {candidates}"
            ),
        }
    }
}

impl std::error::Error for MailboxResolveFailure {}

/// True if `line` (already trimmed) looks like a TOML `key = value`
/// assignment — a bare-key identifier followed by `=`. The role-file
/// salvage path uses it to tell a real field from injected junk such as
/// the stray `"` a torn write left behind in BUG-228.
/// trace:BUG-228 | ai:claude
pub(crate) fn is_toml_kv_line(line: &str) -> bool {
    match line.split_once('=') {
        Some((key, _)) => {
            let key = key.trim();
            !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        }
        None => false,
    }
}

/// TASK-713: terminal width for laying out the picker. Honors `$COLUMNS`
/// (exported by interactive shells), clamps to a sane floor, and falls back
/// to 80 when unknown. Kept separate so the layout function stays pure.
pub(crate) fn picker_terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.trim().parse::<usize>().ok())
        .filter(|w| *w >= 20)
        .unwrap_or(80)
}

#[cfg(test)]
mod task_1239_picker_tests {
    use super::*;

    #[test]
    fn stakeholder_personas_append_as_a_separate_labeled_section() {
        // Two build/driver roles come first; guest + requester follow under a
        // divider heading, each tagged "not a build seat". trace:TASK-1239
        let role_labels = vec!["  advisor".to_string(), "  implementer".to_string()];
        let role_names = vec!["advisor".to_string(), "implementer".to_string()];
        let (labels, targets) = assemble_role_picker_items(role_labels, &role_names, None);

        // Build roles stay first and map to Role targets.
        assert_eq!(targets[0], RolePickTarget::Role("advisor".to_string()));
        assert_eq!(targets[1], RolePickTarget::Role("implementer".to_string()));

        // Then a non-selectable section heading.
        assert_eq!(targets[2], RolePickTarget::Divider);
        assert!(labels[2].contains("Stakeholder personas"), "{}", labels[2]);
        assert!(labels[2].contains("not a build seat"), "{}", labels[2]);

        // Then the two personas, launchable, visually tagged non-build.
        assert_eq!(targets[3], RolePickTarget::Persona("guest".to_string()));
        assert_eq!(targets[4], RolePickTarget::Persona("requester".to_string()));
        assert!(labels[3].contains("guest") && labels[3].contains("not a build seat"));
        assert!(labels[4].contains("requester") && labels[4].contains("not a build seat"));

        // The personas are appended, never mixed into the driver roles.
        assert_eq!(labels.len(), 5);
        assert_eq!(targets.len(), 5);
    }

    #[test]
    fn active_stakeholder_persona_is_marked() {
        let (labels, _) = assemble_role_picker_items(
            vec!["  advisor".to_string()],
            &["advisor".to_string()],
            Some("guest"),
        );
        // guest is index 2 (role, divider, guest) and carries the active `*`.
        assert!(labels[2].starts_with("*"), "{}", labels[2]);
    }
}

#[cfg(test)]
#[path = "tests/sh_single_quote_tests.rs"]
mod sh_single_quote_tests;

/// The canonical first-class route target for "a human is required" — the
/// escalation-cascade terminus (implementer → advisor → human). `aida queue add
/// --for human` files a spec explicitly into the human-attention set, and
/// `aida list human` unions those explicitly-routed specs with the
/// status/tag-derived membership. trace:TASK-747 | ai:claude
pub(crate) const HUMAN_ROUTE: &str = "human";

/// Is this `--for <role>` value the `human` route target? Case-insensitive so
/// `--for Human` and `--for HUMAN` are accepted as the same first-class route.
/// trace:TASK-747 | ai:claude
pub(crate) fn is_human_route(raw: &str) -> bool {
    raw.eq_ignore_ascii_case(HUMAN_ROUTE)
}

/// Collect the spec IDs (uppercased, stable order) of every OPEN requirement
/// explicitly routed `--for human` across all per-user queues. Work-routing is
/// a project-global axis, so we union across all queue YAML files directly
/// (mirrors [`all_queued_requirement_ids`]). Terminal (Completed/Rejected) and
/// archived specs are skipped — a routed item that already shipped is no longer
/// a human bottleneck. Returns an empty set when the queue dir is absent.
/// trace:TASK-747 | ai:claude
pub(crate) fn human_routed_spec_ids(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let dir = project_root.join(".aida-store/registry/queues");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(items) = serde_yaml::from_str::<Vec<aida_core::QueueEntry>>(&content) else {
            continue;
        };
        for item in items {
            // Route match is case-insensitive (canonical_role_name lowercases on
            // add, but legacy/hand-edited entries may carry mixed casing).
            if !item
                .for_role
                .as_deref()
                .map(is_human_route)
                .unwrap_or(false)
            {
                continue;
            }
            let Some(req) = store
                .requirements
                .iter()
                .find(|r| r.id == item.requirement_id)
            else {
                continue;
            };
            if !human_route_is_open(req.archived, &req.status) {
                continue;
            }
            let id = req
                .agreed_id
                .clone()
                .or_else(|| req.spec_id.clone())
                .unwrap_or_else(|| req.id.to_string());
            out.insert(id.to_ascii_uppercase());
        }
    }
    out
}

/// A `--for human` routed spec counts toward the human-attention view only
/// while it is OPEN — an archived or terminal (Completed/Rejected) spec that
/// once carried the route is no longer a bottleneck. Pure over its inputs so
/// the open-test is directly unit-testable. trace:TASK-747 | ai:claude
pub(crate) fn human_route_is_open(archived: bool, status: &aida_core::RequirementStatus) -> bool {
    !archived
        && !matches!(
            status,
            aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
        )
}

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
// paths) live in `doctor_cmd.rs`; the collection helpers below stay here because
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

// ----------------------------------------------------------------------------
// EPIC-31 Phase 2 — supervised agent launchers.
// trace:STORY-432 | ai:codex
// ----------------------------------------------------------------------------

/// BUG-2144: refuse an `aida agent new <tool>` launch under `[agents]
/// contained` when `<tool>` cannot run an AIDA-contained session (only Claude
/// can) — the same rule the drain applies via
/// [`session::require_vendor_containment`].
// trace:BUG-2144 | ai:claude
pub(crate) fn refuse_uncontainable_agent_tool(agent_type: &str) -> Result<()> {
    let containable = session::HeadlessVendor::parse(agent_type)
        .is_some_and(|v| session::containment_applied(v, true));
    if !containable {
        anyhow::bail!(
            "[contained] containment is on ([agents] contained / [contained] enable), but \
             `{agent_type}` cannot run an AIDA-contained session — refusing to launch it \
             uncontained. Launch `aida agent new claude`, or turn containment off for this \
             project."
        );
    }
    Ok(())
}

pub(crate) fn apply_agent_default_flags(
    config: &mut AgentLaunchConfig,
    project_root: &std::path::Path,
    flag_options: AgentDefaultFlagOptions,
    explicit_permission: bool,
) -> Result<()> {
    if flag_options.use_config_defaults {
        let per_tool = load_agent_default_flags(project_root, config.agent_type)?;
        // STORY-495: the uniform `[agents] bypass` knob injects this tool's
        // bypass flag UNLESS (a) the launcher already set an explicit posture
        // (`--permission-mode` / `--bypass-sandbox` / forced `--bg`), or
        // (b) the tool carries its own per-tool `default_flags`, which take
        // precedence over the uniform knob (TASK-557 raw passthrough). A
        // `--no-default-flags` launch skips this block entirely (agents.toml
        // is never read), so it lands on the faithful native default.
        // BUG-2144 (advisor ruling, #2665 review): containment is monotonic on
        // the interactive `agent new` path too. `[agents] contained` on a
        // vendor that cannot run an AIDA-contained session (codex: its own
        // `--sandbox workspace-write` is not AIDA containment; agy; any other
        // tool) is REFUSED here — before any side effect, and before the
        // `--no-exec` preview renders — never downgraded to a native launch.
        // Per-tool `default_flags` do not bypass this; only an explicit
        // launch-posture flag on the command line does (operator override).
        // trace:BUG-2144 | ai:claude
        if !explicit_permission && load_agents_contained(project_root)? {
            refuse_uncontainable_agent_tool(config.agent_type)?;
        }
        if !explicit_permission && per_tool.is_empty() {
            let bypass = load_agents_bypass(project_root)?;
            let contained = load_agents_contained(project_root)?;
            if bypass && contained {
                anyhow::bail!(
                    "[agents] bypass and [agents] contained are mutually exclusive launch postures"
                );
            }
            if contained {
                // trace:BUG-2131 | ai:claude
                aida_core::git_ops::ensure_git_supports_contained()?;
                config
                    .default_args
                    .extend(tool_contained_flags(config.agent_type));
                config.contained = config.agent_type == "claude";
            } else if bypass {
                config
                    .default_args
                    .extend(tool_bypass_flags(config.agent_type));
                config.contained = false;
            }
        }
        // BUG-1699: per-tool `default_flags` used to be appended AFTER an explicit posture
        // the launcher had already set, producing one command line carrying both — e.g.
        // `codex --dangerously-bypass-approvals-and-sandbox --sandbox workspace-write
        // --ask-for-approval never` — and leaving the real posture to an undocumented
        // precedence inside the vendor CLI while the preview asserted the opposite. When the
        // launcher set the posture, the conflicting per-tool flags lose and we say so.
        // trace:BUG-1699 | ai:claude
        let per_tool = if explicit_permission {
            let (kept, dropped) = split_posture_flags(config.agent_type, per_tool);
            if !dropped.is_empty() {
                eprintln!(
                    "  {} explicit launch posture wins — dropped from agents.toml: {}",
                    "note:".yellow(),
                    dropped.join(" ")
                );
            }
            kept
        } else {
            per_tool
        };
        config.default_args.extend(per_tool);

        // BUG-1698 / TASK-1558: give a claude seat the configured AIDA surface. The default
        // (`off`) loads no MCP servers at all — SPIKE-73 measured MCP at ~1.8-2x the CLI's cost
        // for identical or worse success — and it also closes the project-MCP trust modal that
        // used to block every claude launch. Inside `use_config_defaults` so
        // `--no-default-flags` remains the escape hatch to the untouched native launch.
        // trace:BUG-1698 | ai:claude
        // trace:TASK-1558 | ai:claude
        if config.agent_type == "claude" {
            let surface = load_agents_mcp(project_root)?;
            config
                .default_args
                .extend(session::claude_mcp_flags(surface));
        }
    }
    let resolved_model = flag_options
        .model_override
        .as_deref()
        .filter(|m| !m.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            flag_options
                .use_config_defaults
                .then(|| {
                    aida_core::agents_config::resolve_vendor_model(project_root, config.agent_type)
                })
                .flatten()
        });
    // trace:STORY-1003 | ai:codex
    if let Some(model) = resolved_model {
        config.default_args.extend(["--model".to_string(), model]);
    }
    config.default_args.extend(flag_options.extra_flags);
    // BUG-2007: the codex launcher injects `--no-daemon`; an operator who also
    // passes it (extra flag or config default) must not get a doubled flag.
    // trace:BUG-2007 | ai:claude
    crate::agent_launch::dedupe_codex_no_daemon(config.agent_type, &mut config.default_args);
    Ok(())
}

// Apply the consent decision to the final argv, after every configured and
// explicit flag has been assembled. Preview callers only report this result.
// trace:TASK-1500 | ai:codex
pub(crate) fn gate_agent_bypass(
    root: &std::path::Path,
    agent: &str,
    args: &mut Vec<String>,
    background: bool,
    explicit: bool,
) -> Result<()> {
    use bypass_confirm::{BypassSource, Decision};
    let has_bypass = bypass_confirm::args_have_bypass(args);
    let source = if background {
        BypassSource::Background
    } else if explicit {
        BypassSource::Explicit
    } else {
        BypassSource::Configured
    };
    let setting = bypass_confirm::load(root);
    let tty = authority_stdin_is_terminal() && authority_stdout_is_terminal();
    match bypass_confirm::decide(has_bypass, source, setting.on, tty) {
        Decision::Proceed => Ok(()),
        Decision::Prompt if bypass_confirm::prompt(agent) => {
            eprintln!("  bypass confirmed at prompt");
            Ok(())
        }
        Decision::Prompt if matches!(source, BypassSource::Background) => {
            anyhow::bail!("background bypass needs affirmative terminal confirmation; pass --permission-mode <mode> for an explicit foreground launch")
        }
        Decision::Prompt => { bypass_confirm::strip_bypass_flags(args); eprintln!("  bypass declined; continuing with native permissions"); Ok(()) }
        Decision::DowngradeNative => { bypass_confirm::strip_bypass_flags(args); eprintln!("  bypass confirmation needs a terminal; continuing with native permissions"); Ok(()) }
        Decision::Refuse => anyhow::bail!("bypass needs confirmation at a terminal; run from an interactive shell or set [agents] confirm_bypass = false in your user config"),
    }
}

/// Write the global `~/.aida/agents.toml` recording the chosen permission
/// posture. Creates `~/.aida/` if needed. trace:TASK-698 | ai:claude
#[cfg(test)]
pub(crate) fn write_global_agents_posture(path: &std::path::Path, bypass: bool) -> Result<()> {
    write_global_agents_permission_posture(
        path,
        if bypass {
            AgentPermissionPosture::Bypass
        } else {
            AgentPermissionPosture::Native
        },
    )
}

/// STORY-495: resolve the effective `--permission-mode` for the
/// `session new` / `session start --launch` interactive launchers. `None`
/// means honor Claude's native posture (the faithful default). Precedence:
/// explicit flag > `[agents] bypass` knob > native. (Unlike `queue work`,
/// these launchers do not layer the `AIDA_PERMISSION_MODE` env or
/// `[behavior] permission_mode` config — that resolution is queue-work
/// specific and predates this change.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedClaudeLaunch {
    pub(crate) mode: Option<String>,
    pub(crate) contained: bool,
}

// ----------------------------------------------------------------------------
// EPIC-20 v1 — scoped session leases.
// trace:EPIC-20 | ai:claude
// ----------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Per-session role activity log (STORY-56)
//
// Project-level role activity (`.aida/roles/<name>.toml`'s `activity` field)
// is shared by every shell that has the role active. When two sessions for
// the same project both run `role:implementer`, they fight over the same
// "current spec" — last writer wins, and `aida statusline`'s @SPEC segment
// flips between specs depending on which session most recently touched
// something. STORY-56 splits that activity stream: while a shell is inside
// a session lease's worktree, role activity goes to a session-local log
// (`.aida/sessions/<id>.activity.toml`) instead. Statusline reads the
// session log when in-session, the project log otherwise. `session end`
// flattens unique (spec_id) entries from the session log back into each
// participating role's project-level activity (newest first, dedupe,
// truncate to ACTIVITY_MAX) so long-term recent-activity views still show
// what was worked on under the closed session.
// trace:STORY-56 | ai:claude
// ---------------------------------------------------------------------------

pub(crate) fn print_merge_hold_no_forge(json: bool) {
    if json {
        println!("{}", serde_json::json!({"status":"no_forge", "labels":{}}));
    } else {
        println!("No forge is configured; there are no merge-hold labels to provision.");
    }
}

/// STORY-1436: `aida history --kind <KIND>` — one event kind from the local
/// feed (rotated archive included), newest first, with the gate/floor tally
/// for `gate-held`.
// trace:STORY-1436 | ai:claude
pub(crate) fn history_kind_report(
    kind: &str,
    since: Option<&str>,
    until: Option<&str>,
    author: Option<&str>,
    limit: usize,
) -> Result<()> {
    let want = events::normalize_kind_name(kind);
    if !events::EventKind::known_names()
        .iter()
        .any(|n| events::normalize_kind_name(n) == want)
    {
        anyhow::bail!(
            "unknown event kind `{kind}`; known kinds: {}",
            events::EventKind::known_names().join(", ")
        );
    }
    // `--kind` shares `aida history`'s --since/--until flags, so it gets the
    // same time-bound grammar and ordering check as the digest/events views,
    // instead of only the bare-date/RFC3339 forms `events::parse_time_bound`
    // covers.
    // trace:TASK-1502 | ai:claude
    let now = chrono::Utc::now();
    let bound = |v: Option<&str>, flag: &str| -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        v.map(|raw| history::parse_history_bound(raw, flag, now, &chrono::Local))
            .transpose()
    };
    let since_at = bound(since, "--since")?;
    let until_at = bound(until, "--until")?;
    history::validate_window_order(since_at, until_at, &chrono::Local)?;
    let query = events::KindQuery {
        kind: kind.to_string(),
        since: since_at,
        until: until_at,
        who: author.map(str::to_string),
        limit,
    };
    let root = find_main_worktree_root()?;
    let all = events::read_all_with_archive(&root);
    events::kind_report(&all, &query, std::io::stdout().lock())?;
    Ok(())
}

/// STORY-1436: BUG-1535's ambiguous-id refusal fires from dozens of lookup
/// sites; it is recorded ONCE, here, where every refusal surfaces. Only when
/// the error chain carries an `AmbiguousIdError` (no cost otherwise), and
/// only into a project that already has `.aida/` (never creates one).
// trace:STORY-1436 | ai:claude
pub(crate) fn record_ambiguous_id_refusal(err: &anyhow::Error) {
    let Some(ambiguous) = ambiguous_id_in_chain(err) else {
        return;
    };
    let Ok(root) = find_main_worktree_root() else {
        return;
    };
    if !root.join(".aida").is_dir() {
        return;
    }
    events::record_gate_held(
        &root,
        events::GATE_AMBIGUOUS_ID,
        Some(ambiguous.id.to_ascii_uppercase()),
        None,
        &format!(
            "`{}` resolves to {} requirements; refused to pick one",
            ambiguous.id.to_ascii_uppercase(),
            ambiguous.candidates.len()
        ),
    );
}

// trace:STORY-1436 | ai:claude
pub(crate) fn ambiguous_id_in_chain(
    err: &anyhow::Error,
) -> Option<&aida_core::id_collisions::AmbiguousIdError> {
    err.chain()
        .find_map(|e| e.downcast_ref::<aida_core::id_collisions::AmbiguousIdError>())
}

// ─── TASK-965: worktree-tangle gate (substrate-as-bouncer) ─────────────────
//
// AIDA's fan-out incident: a bare agent `git checkout -b`s in the MAIN repo,
// leaving the primary checkout parked on a feature branch, so the next
// `git merge --ff-only origin/main` aborts. We used to defend this with a memory
// RULE only; the substrate-as-bouncer principle says ship a GATE. Two pure
// predicates back the two guards — a spawn-time assertion (`aida agent new`) and
// a stranded-primary alarm (`aida status` / `aida ps`). Kept side-effect-free so
// both are unit-testable without git/leases.
