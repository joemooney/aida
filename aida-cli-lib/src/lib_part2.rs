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
    std::process::Command::new("git")
        .arg("-C")
        .arg(store_path)
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
    std::process::Command::new("git")
        .arg("-C")
        .arg(&store_path)
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
        if !explicit_permission && per_tool.is_empty() {
            let bypass = load_agents_bypass(project_root)?;
            let contained = load_agents_contained(project_root)?;
            if bypass && contained {
                anyhow::bail!(
                    "[agents] bypass and [agents] contained are mutually exclusive launch postures"
                );
            }
            if contained {
                config
                    .default_args
                    .extend(tool_contained_flags(config.agent_type));
            } else if bypass {
                config
                    .default_args
                    .extend(tool_bypass_flags(config.agent_type));
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

/// TASK-54: return Some((behind, sample)) when `branch` lags `main`
/// (or origin/main) by 1+ commits. `sample` is a list of one-line
/// `<short-sha> <subject>` strings for the commits the user would
/// rebase onto. Returns None when:
///   - we're already on main (no point comparing to ourselves)
///   - main / origin/main don't exist locally
///   - branch is equal-to or strictly-ahead-of main (nothing to pull)
///   - any git invocation fails (treat as "no signal" rather than
///     pretending we know the answer).
///     Prefers `origin/main` when present so users with stale local
///     `main` still see the right delta. trace:TASK-54 | ai:claude
///     STORY-106: count commits this branch is ahead of `origin/main` (falls
///     back to `main` when origin isn't available). Best-effort — returns
///     `None` on any git failure so callers can degrade silently.
///     trace:STORY-106 | ai:claude
pub(crate) fn branch_commits_ahead_main(repo: &std::path::Path, branch: &str) -> Option<u32> {
    match branch_commits_ahead_default(repo, branch) {
        workflow_hints::CommitsAhead::Ahead(n) => Some(n),
        _ => None,
    }
}

pub(crate) fn branch_unshipped_patch_count_default(
    repo: &std::path::Path,
    branch: &str,
) -> Option<u32> {
    let Some(default_ref) = resolve_default_branch_ref(repo) else {
        return None;
    };
    branch_unshipped_patch_count_vs(repo, &default_ref, branch)
}

/// Body of [`branch_unshipped_patch_count_default`] with the default ref
/// already resolved. BUG-1756: the unshipped-work detector probes dozens of
/// candidate branches under a wall-clock budget; resolving the default ref
/// per branch (1–2 git subprocesses each) was pure overhead against that
/// budget, so the detector resolves once and passes it here.
// trace:BUG-1756 | ai:claude
pub(crate) fn branch_unshipped_patch_count_vs(
    repo: &std::path::Path,
    default_ref: &str,
    branch: &str,
) -> Option<u32> {
    match branch_unshipped_patch_count_probe(repo, default_ref, branch, None) {
        PatchCountProbe::Counted(n) => Some(n),
        PatchCountProbe::NoSignal | PatchCountProbe::TimedOut => None,
    }
}

/// Outcome of one candidate branch's bounded patch-count probe.
// trace:BUG-1756 | ai:claude
pub(crate) enum PatchCountProbe {
    Counted(u32),
    /// git failed — the pre-existing "no signal" shape: the candidate is
    /// dispositioned (skipped) and counts as scanned, exactly as
    /// [`branch_unshipped_patch_count_default`] returning `None` always has.
    NoSignal,
    /// The probe exceeded its per-candidate slice before answering. The
    /// candidate was NOT classified, so the caller must count it as
    /// unscanned and the scan as incomplete — never as "nothing to
    /// report" (PRIN-5).
    TimedOut,
}

/// [`branch_unshipped_patch_count_vs`] with an optional per-invocation time
/// slice. BUG-1756: `git cherry` patch-ids every default-branch commit since
/// the fork point, so ONE anciently-forked candidate (an old review
/// snapshot, a long-dead ref) can cost 20–30s — several times the entire
/// scan budget — and starve every candidate sorted after it. The slice caps
/// what a single candidate may spend; `None` (every unbounded caller, and
/// the exhaustive test paths) behaves exactly as before.
// trace:BUG-1756 | ai:claude
pub(crate) fn branch_unshipped_patch_count_probe(
    repo: &std::path::Path,
    default_ref: &str,
    branch: &str,
    probe_slice: Option<std::time::Duration>,
) -> PatchCountProbe {
    let run = |args: &[&str]| -> Result<std::process::Output, PatchCountProbe> {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(repo).args(args);
        match probe_slice {
            Some(slice) => match command_output_with_timeout_detail(cmd, slice) {
                BoundedCommandOutput::Completed(out) => Ok(out),
                BoundedCommandOutput::SpawnFailed => Err(PatchCountProbe::NoSignal),
                BoundedCommandOutput::TimedOut => Err(PatchCountProbe::TimedOut),
            },
            None => cmd.output().map_err(|_| PatchCountProbe::NoSignal),
        }
    };

    // trace:BUG-853 | ai:codex
    // A squash-merged branch can still be ahead by commit id while its tree is
    // identical to the default branch. Treat that as shipped content.
    // The branch can come from a registered lease, so keep it from
    // reading as an option. trace:BUG-1622 | ai:claude
    let tree_diff = match run(&[
        "diff",
        "--quiet",
        git_arg_guard::END_OF_OPTIONS,
        default_ref,
        branch,
        "--",
    ]) {
        Ok(out) => out,
        Err(probe) => return probe,
    };
    match tree_diff.status.code() {
        Some(0) => return PatchCountProbe::Counted(0),
        Some(1) => {}
        _ => return PatchCountProbe::NoSignal,
    }

    // trace:BUG-1622 | ai:claude
    let out = match run(&["cherry", git_arg_guard::END_OF_OPTIONS, default_ref, branch]) {
        Ok(out) => out,
        Err(probe) => return probe,
    };
    if !out.status.success() {
        return PatchCountProbe::NoSignal;
    }
    PatchCountProbe::Counted(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|line| line.starts_with("+ "))
            .count() as u32,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReworkHeadChange {
    Unchanged,
    RebaseOnly,
    ContentChanged,
}

/// Classify the branch's contribution between two rework heads.
///
/// A changed SHA alone is not evidence of rework: amend and rebase rewrite
/// commit identities. Excluding the current default branch keeps a pure rebase
/// onto a newer main from looking like newly-authored work.
// trace:TASK-1265 trace:BUG-1445 | ai:codex
pub(crate) fn classify_rework_head_change(
    repo: &std::path::Path,
    before: &str,
    after: &str,
) -> Option<ReworkHeadChange> {
    let before = before.trim();
    let after = after.trim();
    // `before` is a stored verdict's reviewed sha: only a commit ID may
    // reach git. A non-ID is unclassifiable (the caller refuses).
    // trace:BUG-1622 | ai:claude
    if !git_arg_guard::is_hex_sha(before) || !git_arg_guard::is_hex_sha(after) {
        return None;
    }
    if before.eq_ignore_ascii_case(after) {
        return Some(ReworkHeadChange::Unchanged);
    }

    let tree_diff = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "diff",
            "--quiet",
            git_arg_guard::END_OF_OPTIONS,
            before,
            after,
            "--",
        ])
        .status()
        .ok()?;
    match tree_diff.code() {
        Some(0) => return Some(ReworkHeadChange::Unchanged),
        Some(1) => {}
        _ => return None,
    }

    let default_ref = resolve_default_branch_ref(repo)?;
    let exclude_default = format!("^{default_ref}");
    let range = format!("{before}...{after}");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "rev-list",
            "--cherry-pick",
            "--right-only",
            git_arg_guard::END_OF_OPTIONS,
            &range,
            &exclude_default,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(if String::from_utf8_lossy(&out.stdout).trim().is_empty() {
        ReworkHeadChange::RebaseOnly
    } else {
        ReworkHeadChange::ContentChanged
    })
}

// trace:TASK-1265 trace:BUG-1445 | ai:codex
pub(crate) fn rework_no_op_message(
    pr: u32,
    before: &str,
    after: &str,
    reason: &str,
    round: usize,
    change: ReworkHeadChange,
) -> String {
    let classification = match change {
        ReworkHeadChange::Unchanged => "the PR head did not move",
        ReworkHeadChange::RebaseOnly => {
            "the PR head moved, but its patch-ids are unchanged (rebase-only)"
        }
        ReworkHeadChange::ContentChanged => "the branch contribution changed",
    };
    format!(
        "ROUND {round} rework added no patch-unique work to PR-{pr}: {classification} (head `{}` → `{}`). The previous round's commits do not count. Authoritative open items:\n{reason}",
        before.trim(),
        after.trim()
    )
}

// Reviewers normally persist the verdict under PR-N; older/manual handoffs
// may use the requirement id. Keep both readable so the no-op guard is armed
// by the same artifact that the reviewer actually wrote.
// trace:BUG-1445 | ai:codex
pub(crate) fn blocking_rework_verdict(
    project_root: &std::path::Path,
    spec: &str,
    pr: u32,
) -> Option<review_verdict::RecordedVerdict> {
    if let Some(verdict) = review_verdict::read_recorded_verdict(project_root, spec)
        .filter(|verdict| verdict.kind.blocks_done())
    {
        return Some(verdict);
    }
    let pr_id = format!("PR-{pr}");
    review_verdict::read_recorded_verdict(project_root, &pr_id)
        .filter(|verdict| verdict.kind.blocks_done())
}

/// Count how many commits `branch` is ahead of the repo's DEFAULT branch, and
/// say WHY when the answer can't be produced.
///
/// The `Option<u32>` wrapper above collapses "on the default branch",
/// "no default branch ref anywhere", and "rev-list failed" into one `None`,
/// which is fine for a hint but fatal for a gate: a gate that can't tell
/// "nothing to compare" from "comparison impossible" ends up waving work
/// through. `aida queue done` uses this richer form.
///
/// Two fixes over the old `origin/main`-or-`main` probe:
///   - the default branch is resolved with the same order the rest of the CLI
///     uses (`origin/HEAD` → `init.defaultBranch` → `main`/`master`, remote
///     ref preferred, LOCAL ref accepted) — so a repo with NO origin remote,
///     or one whose default branch is `master`, still gets a real answer.
///     Local-merge workflows live in exactly those repos.
///   - being *on* the default branch is a distinct, named outcome rather than
///     an unresolvable one.
// trace:BUG-775 | ai:claude
pub(crate) fn branch_commits_ahead_default(
    repo: &std::path::Path,
    branch: &str,
) -> workflow_hints::CommitsAhead {
    let Some(default_ref) = resolve_default_branch_ref(repo) else {
        return workflow_hints::CommitsAhead::Unknown(
            "no default branch could be resolved in this repository — \
             neither origin/HEAD, init.defaultBranch, main, nor master exists"
                .to_string(),
        );
    };
    // `origin/main` → `main`: compare NAMES so a branch checked out locally
    // is recognised as the default branch even when the resolved ref is the
    // remote-tracking form.
    let default_name = default_ref
        .strip_prefix("origin/")
        .unwrap_or(&default_ref)
        .to_string();
    if branch == "HEAD" || branch.eq_ignore_ascii_case(&default_name) || branch == default_ref {
        return workflow_hints::CommitsAhead::OnDefaultBranch;
    }
    let range = format!("{}..{}", default_ref, branch);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-list", "--count", &range])
        .output();
    let out = match out {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            return workflow_hints::CommitsAhead::Unknown(format!(
                "`git rev-list --count {range}` failed{}",
                if err.is_empty() {
                    String::new()
                } else {
                    format!(": {err}")
                }
            ));
        }
        Err(e) => {
            return workflow_hints::CommitsAhead::Unknown(format!("could not run git: {e}"));
        }
    };
    match String::from_utf8_lossy(&out.stdout).trim().parse::<u32>() {
        Ok(n) => workflow_hints::CommitsAhead::Ahead(n),
        Err(_) => workflow_hints::CommitsAhead::Unknown(format!(
            "`git rev-list --count {range}` returned no count"
        )),
    }
}

/// Full object name for `rev` in `repo`, or `None` when it doesn't resolve to
/// a commit here (unknown sha, sha from another clone, garbage input).
/// Expanding both sides is what lets a short sha recorded in a review verdict
/// be compared with a branch tip.
// trace:BUG-775 | ai:claude
pub(crate) fn resolve_commit_sha(repo: &std::path::Path, rev: &str) -> Option<String> {
    let rev = rev.trim();
    if rev.is_empty() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            git_arg_guard::END_OF_OPTIONS, // trace:BUG-1622 | ai:claude
            &format!("{rev}^{{commit}}"),
        ])
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

/// TASK-1459: the head `aida review <SPEC>` marks its review-in-progress
/// marker at. Prefers the forge's live PR head — the local `origin/<branch>`
/// tracking ref used before this can be stale (nothing here fetches it), so a
/// marker written against it could under-cover a head that already moved.
/// Falls back to that local ref when the forge can't be reached (pure-git,
/// offline, forge fault) rather than leaving the marker head unrecorded,
/// which is the same best-effort shape as [`pr_head_sha_best_effort`].
// trace:TASK-1459 | ai:claude
pub(crate) fn review_marker_head_best_effort(
    project_root: &std::path::Path,
    pr: u64,
    branch: &str,
) -> Option<String> {
    pr_cmd::fetch_change_info_via_resolved_forge(
        project_root,
        pr,
        crate::forge::resolve_open_change_forge_kind(project_root),
    )
    .ok()
    .map(|info| info.head_oid)
    .filter(|s| !s.trim().is_empty())
    .or_else(|| resolve_commit_sha(project_root, &format!("origin/{branch}")))
}

/// `git merge-base --is-ancestor <ancestor> <descendant>` as a tri-state:
/// `Some(true)` / `Some(false)` for the two clean answers, `None` when the
/// probe itself could not run.
// trace:BUG-775 | ai:claude
pub(crate) fn is_ancestor_commit(
    repo: &std::path::Path,
    ancestor: &str,
    descendant: &str,
) -> Option<bool> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "merge-base",
            "--is-ancestor",
            git_arg_guard::END_OF_OPTIONS,
            ancestor,
            descendant,
        ]) // trace:BUG-1622 | ai:claude
        .output()
        .ok()?;
    match out.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Where the tip of `branch` (or HEAD when `branch` is `None`) sits relative
/// to the commit a review verdict named. All the git I/O for the review gate
/// lives here; the decision itself is the pure
/// `review_verdict::classify_tip_relation`.
// trace:BUG-775 | ai:claude
pub(crate) fn verdict_tip_relation(
    repo: &std::path::Path,
    branch: Option<&str>,
    reviewed_sha: Option<&str>,
) -> review_verdict::TipRelation {
    let Some(reviewed_raw) = reviewed_sha else {
        return review_verdict::TipRelation::Unknown;
    };
    let reviewed = resolve_commit_sha(repo, reviewed_raw);
    let tip = resolve_commit_sha(repo, branch.unwrap_or("HEAD"))
        .or_else(|| resolve_commit_sha(repo, "HEAD"));
    let ancestry = match (reviewed.as_deref(), tip.as_deref()) {
        (Some(a), Some(b)) => is_ancestor_commit(repo, a, b),
        _ => None,
    };
    review_verdict::classify_tip_relation(reviewed.as_deref(), tip.as_deref(), ancestry)
}

/// Resolve a branch's head LOCALLY, no forge call. `refs/remotes/origin/`
/// first (the common case: the branch was pushed but this checkout never
/// took it as a local branch), falling back to `refs/heads/` (a local-only
/// branch, e.g. the checkout this process is running in). Neither resolving
/// is "unknown", not an error -- the caller folds that into
/// `TipRelation::Unknown`, which `review_actionability` already treats as
/// indeterminate-therefore-absent (PRIN-5).
// trace:BUG-1508 | ai:claude
pub(crate) fn resolve_local_branch_head(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    resolve_commit_sha(project_root, &format!("refs/remotes/origin/{branch}"))
        .or_else(|| resolve_commit_sha(project_root, &format!("refs/heads/{branch}")))
}

/// The spec id(s) a routed reviewer-queue entry's verdict must be read
/// against. A `Review PR-N: ...` auto-queue story (BUG-102/BUG-776) doesn't
/// carry a verdict itself -- it `implements` the real spec(s) the PR covers
/// (the same relationship `aida_subcmd_add_review_story` writes), so walk
/// that edge. A direct routing (the row's own spec_id, no review-story
/// wrapper) covers only itself.
///
/// `resolve` looks a relationship target's uuid up to its display id.
/// Generic over the lookup so a caller holding a full `RequirementsStore`
/// (an index) and a caller holding only a cache/backend (one targeted read
/// per uuid) share this walk.
// trace:BUG-1508 | ai:claude
pub(crate) fn covered_spec_ids_for_reviewer_row(
    req: &aida_core::Requirement,
    mut resolve: impl FnMut(uuid::Uuid) -> Option<String>,
) -> Vec<String> {
    if parse_review_story_pr_number(&req.title).is_some() {
        let ids: Vec<String> = req
            .relationships
            .iter()
            .filter(|rel| {
                matches!(&rel.rel_type, aida_core::RelationshipType::Custom(n) if n.eq_ignore_ascii_case("implements"))
            })
            .filter_map(|rel| resolve(rel.target_id))
            .collect();
        if !ids.is_empty() {
            return ids;
        }
    }
    req.agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .into_iter()
        .collect()
}

/// A routed reviewer-queue row's actionability (BUG-1508 AC1/AC2/AC3/AC8),
/// resolved entirely from local state: the verdict file(s) for the spec(s)
/// the row covers, and each covered spec's branch head resolved via
/// `resolve_local_branch_head` -- no forge/network call, so this is safe on
/// the fast `aida queue list` / `aida awaiting` paths.
///
/// The branch a covered spec's current head is read from, in order: the
/// verdict's own `reviewed_branch` (the reviewer recorded it, so it's the
/// most specific signal), then a live lease scoped to that spec, then a live
/// lease scoped to the ROW's own id (the review-story's lease, when a
/// reviewer has taken it via `aida worktree enter`). No resolvable branch
/// means an unknown head, which folds into `TipRelation::Unknown` ->
/// `NeedsReview` -- indeterminate is never read as covered (AC8).
///
/// A row covering several specs (one PR, several `(REQ-ID)` trailers) is
/// `NeedsReview` if ANY covered spec still needs one, else `AwaitingRework`
/// if any blocks, else `Resolved`.
///
/// `resolve` is the same uuid -> display-id lookup `covered_spec_ids_for_reviewer_row`
/// takes -- see that function for why it's generic.
// trace:BUG-1508 | ai:claude
pub(crate) fn reviewer_row_actionability(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
    leases: &[SessionLease],
    resolve: impl FnMut(uuid::Uuid) -> Option<String>,
) -> review_verdict::ReviewActionability {
    let story_id = req
        .agreed_id
        .as_deref()
        .or(req.spec_id.as_deref())
        .unwrap_or("");
    let covered = covered_spec_ids_for_reviewer_row(req, resolve);
    if covered.is_empty() {
        return review_verdict::ReviewActionability::NeedsReview;
    }
    let mut saw_rework = false;
    for spec_id in &covered {
        let verdict = review_verdict::read_recorded_verdict_any(project_root, &[spec_id.as_str()]);
        let branch = verdict
            .as_ref()
            .and_then(|v| v.reviewed_branch.clone())
            .or_else(|| {
                leases
                    .iter()
                    .find(|l| l.scope.eq_ignore_ascii_case(spec_id))
                    .map(|l| l.branch.clone())
            })
            .or_else(|| {
                leases
                    .iter()
                    .find(|l| l.scope.eq_ignore_ascii_case(story_id))
                    .map(|l| l.branch.clone())
            });
        let head = branch.and_then(|b| resolve_local_branch_head(project_root, &b));
        let reviewed_sha = verdict.as_ref().and_then(|v| v.reviewed_sha.as_deref());
        let ancestry = match (reviewed_sha, head.as_deref()) {
            (Some(a), Some(b)) => is_ancestor_commit(project_root, a, b),
            _ => None,
        };
        let relation =
            review_verdict::classify_tip_relation(reviewed_sha, head.as_deref(), ancestry);
        match review_verdict::review_actionability(verdict.as_ref(), relation) {
            review_verdict::ReviewActionability::NeedsReview => {
                return review_verdict::ReviewActionability::NeedsReview;
            }
            review_verdict::ReviewActionability::AwaitingRework => saw_rework = true,
            review_verdict::ReviewActionability::Resolved => {}
        }
    }
    if saw_rework {
        review_verdict::ReviewActionability::AwaitingRework
    } else {
        review_verdict::ReviewActionability::Resolved
    }
}

#[cfg(test)]
#[path = "tests/bug_1186_reviewer_seat_tests.rs"]
mod bug_1186_reviewer_seat_tests;
#[cfg(test)]
#[path = "tests/bug_1508_reviewer_row_tests.rs"]
mod bug_1508_reviewer_row_tests;
#[cfg(test)]
#[path = "tests/bug_775_commits_ahead_tests.rs"]
mod bug_775_commits_ahead_tests;

/// TASK-99: count how many commits the local `base_ref` is BEHIND
/// `origin/main` — i.e. commits on origin that the base hasn't yet
/// picked up. Returns `None` when `origin/main` (or the base ref)
/// doesn't resolve — fresh clone, offline, detached, etc. — so the
/// caller stays silent rather than warn on missing data. Best-effort:
/// any git failure → `None`. trace:TASK-99 | ai:claude
pub(crate) fn commits_behind_origin_main(repo: &std::path::Path, base_ref: &str) -> Option<u32> {
    let rev_parse = |refname: &str| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                git_arg_guard::END_OF_OPTIONS,
                refname,
            ]) // trace:BUG-1622 | ai:claude
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    // Only meaningful against the remote-tracking ref. If `origin/main`
    // isn't present we have nothing authoritative to compare against.
    rev_parse("origin/main")?;
    rev_parse(base_ref)?;
    let range = format!("{}..origin/main", base_ref);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-list", "--count", git_arg_guard::END_OF_OPTIONS, &range]) // trace:BUG-1622 | ai:claude
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// TASK-99: pure decision — given how many commits the new worktree's
/// base is behind `origin/<branch>`, produce the warning line (or
/// `None` when the base is current). Side-effect-free so the
/// behind-count → message mapping is unit-testable in isolation, no
/// git/CWD required. trace:TASK-99 | ai:claude
pub(crate) fn behind_origin_warning(behind: u32, branch: &str) -> Option<String> {
    if behind == 0 {
        return None;
    }
    Some(format!(
        "worktree base is {} commit{} behind origin/{} — run `aida rebase` to refresh, or your PR may land stale",
        behind,
        if behind == 1 { "" } else { "s" },
        branch
    ))
}

/// Default warn threshold (commits behind `origin/main`) at or above which
/// the statusline surfaces the `base behind by N` staleness indicator for an
/// active lease. Kept a touch loud so a one-commit drift doesn't nag every
/// prompt; the queue-list surface uses a lower (any-non-zero) floor.
// trace:TASK-101 | ai:claude
pub(crate) const BASE_BEHIND_STATUSLINE_THRESHOLD: u32 = 5;

/// Pure formatting for the "base behind by N" staleness indicator.
/// Given how many commits an active lease's branch is BEHIND `origin/main`
/// (from [`commits_behind_origin_main`]) and a noise-floor `threshold`,
/// produce the compact `base behind by N` label — or `None` when the base is
/// current (`behind == 0`) or the drift is below the caller's threshold. The
/// statusline passes a higher warn threshold; `aida queue list` passes `1` so
/// any non-zero drift surfaces. Side-effect-free so the behind-count → label
/// mapping is unit-testable without git/CWD.
// trace:TASK-101 | ai:claude
pub(crate) fn base_behind_indicator(behind: u32, threshold: u32) -> Option<String> {
    if behind == 0 || behind < threshold {
        return None;
    }
    Some(format!("base behind by {}", behind))
}

#[cfg(test)]
#[path = "tests/task_101_base_behind_tests.rs"]
mod task_101_base_behind_tests;

#[cfg(test)]
#[path = "tests/task_99_behind_origin_tests.rs"]
mod task_99_behind_origin_tests;

pub(crate) fn branch_behind_main(
    repo: &std::path::Path,
    branch: &str,
) -> Option<(u64, Vec<String>)> {
    if branch == "main" || branch == "HEAD" {
        return None;
    }
    let rev_parse = |refname: &str| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                git_arg_guard::END_OF_OPTIONS,
                refname,
            ]) // trace:BUG-1622 | ai:claude
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    // Prefer origin/main when it exists — local main can be days behind.
    let main_ref = rev_parse("origin/main")
        .map(|_| "origin/main")
        .or_else(|| rev_parse("main").map(|_| "main"))?;
    let range = format!("{}..{}", branch, main_ref);
    let count_out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        // trace:BUG-1622 | ai:claude
        .args(["rev-list", "--count", git_arg_guard::END_OF_OPTIONS, &range])
        .output()
        .ok()?;
    if !count_out.status.success() {
        return None;
    }
    let count: u64 = String::from_utf8_lossy(&count_out.stdout)
        .trim()
        .parse()
        .ok()?;
    if count == 0 {
        return None;
    }
    let log_out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", &range, "--pretty=format:%h %s", "--no-merges"])
        .output()
        .ok()?;
    let sample: Vec<String> = String::from_utf8_lossy(&log_out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    Some((count, sample))
}
