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

pub(crate) fn humanize_relative(t: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let delta = now.signed_duration_since(t);
    let secs = delta.num_seconds();
    if secs < 0 {
        return "just now".to_string();
    }
    if secs < 60 {
        return format!("{}s ago", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m ago", mins);
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h ago", hours);
    }
    let days = hours / 24;
    format!("{}d ago", days)
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

// trace:TASK-714
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs_forge::TS)]
pub(crate) struct SessionLease {
    /// 12-char hex id derived from the time-ordered uuid v7, for short refs.
    pub(crate) id: String,
    /// Raw scope string the user passed to `--owns`.
    pub(crate) scope: String,
    /// Resolved scope slug, used for branch + dir naming.
    pub(crate) slug: String,
    /// Owner email/user (best-effort from git config).
    pub(crate) owner: String,
    /// Worktree path (canonicalized).
    pub(crate) worktree_path: std::path::PathBuf,
    /// Branch the worktree is on.
    pub(crate) branch: String,
    /// ISO-8601 UTC.
    pub(crate) started_at: chrono::DateTime<chrono::Utc>,
    /// Hostname when started.
    pub(crate) hostname: String,
    /// Role active in the calling shell at session-start time, if any.
    /// Lets `aida session leases` / `aida session show` display role per
    /// session, and gives subsequent role-aware tooling inside the worktree
    /// the right starting persona. Optional for back-compat with older
    /// leases written before STORY-65. trace:STORY-65 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
    /// PID of the shell that ran `aida session start`. Used by
    /// `aida session end` to resolve the right lease when the user runs
    /// `end` from the same parent shell that ran `start` (cwd is outside
    /// the worktree). Optional for back-compat. trace:STORY-73 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) creator_pid: Option<u32>,
    /// Kernel start identity paired with `creator_pid`. Missing on legacy
    /// leases, which deliberately retain the historical PID-only behavior.
    // trace:TASK-1284 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) creator_pid_start_time: Option<String>,
    /// BUG-741: PID of the currently hosted agent child for process-backed
    /// launches such as headless `codex exec`. Unlike `creator_pid`, this is
    /// the worker process itself: alive => Live, dead => Stale.
    /// BUG-752: harness-worktree (Agent-tool subagent) leases stamp the
    /// parent claude harness pid here — the subagent executes inside that
    /// process, so its lifetime bounds the work and the cwd-based worktree
    /// probe can never see it.
    // trace:BUG-741 | ai:codex
    // trace:BUG-752 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) active_pid: Option<u32>,
    /// Kernel start identity paired with `active_pid`; prevents a recycled PID
    /// from keeping a dead process-backed session live.
    // trace:TASK-1284 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) active_pid_start_time: Option<String>,
    /// Parent project's `target/` dir, captured so the session shell can
    /// share its cargo build cache with the parent worktree (avoids a full
    /// rebuild on first `cargo build` inside the session). `None` when the
    /// project doesn't have a `target/` (e.g., non-Rust project, or fresh
    /// checkout that's never been built). Sourced via `.aida/session-env.sh`.
    /// trace:STORY-52 | ai:claude
    #[serde(default)]
    pub(crate) cargo_target_dir: Option<std::path::PathBuf>,
    /// Parent project root (the worktree where `aida session start` ran).
    /// Recorded so cross-worktree views like `aida session list` can walk
    /// the parent's Claude Code session directory in addition to the
    /// session worktree's own. `None` for leases written before STORY-58.
    /// trace:STORY-58 | ai:claude
    #[serde(default)]
    pub(crate) parent_project_root: Option<std::path::PathBuf>,
    /// STORY-71: for PR/MR review sessions (--owns PR-N / MR-N), the
    /// PR head commit SHA captured via `gh pr view` / `glab mr view` at
    /// session-start time. `None` when the scope isn't a PR/MR or when
    /// the forge CLI wasn't available. Surfaced by `aida session show`.
    /// trace:STORY-71 | ai:claude
    #[serde(default)]
    pub(crate) pr_head_sha: Option<String>,
    /// STORY-71: PR base commit SHA at session-start time (companion to
    /// pr_head_sha). Lets the reviewer recompute the diff range later
    /// without round-tripping to the forge.
    /// trace:STORY-71 | ai:claude
    #[serde(default)]
    pub(crate) pr_base_sha: Option<String>,
    /// STORY-71: PR base ref name (e.g. `main`). Mostly informational —
    /// for reporting in `aida session show`.
    /// trace:STORY-71 | ai:claude
    #[serde(default)]
    pub(crate) pr_base_ref: Option<String>,
    /// BUG-237: zen-intent token for a standalone `aida queue work --zen`
    /// session — the per-invocation UUID the `--zen` dispatch minted into
    /// `AIDA_ZEN_TOKEN`. Its presence is what corroborates a standalone zen
    /// session: `aida zen status` trusts an inherited `AIDA_ZEN=1` only when
    /// the lease covering the worktree carries this token (or a live
    /// orchestrator run owns the session). `None` for non-`--zen` sessions —
    /// the dispatch scrubs `AIDA_ZEN_TOKEN` unless `--zen` was genuinely
    /// passed — so a leaked `AIDA_ZEN=1` cannot silently enable zen mode.
    /// trace:BUG-237 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) zen_intent_token: Option<String>,
    /// TASK-358: when the orchestrator's `--escalate-blocks` path parks a
    /// punted spec for a human, it stamps this timestamp on the implementer's
    /// lease. The marker has two readers: `aida edit --status` (out of
    /// `NeedsAttention`) cleans up any lease carrying it for the triaged spec
    /// — STORY-306 deliberately left the worktree alive for the advisor's
    /// resume path, but a never-resumed escalation otherwise leaks; and
    /// `aida session prune --escalations` lists the same set for explicit
    /// bulk cleanup. Absent for interactive user sessions and for the
    /// advisor-resume path, so neither is touched.
    /// trace:TASK-358 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) escalated_to_human: Option<chrono::DateTime<chrono::Utc>>,
    /// STORY-248: when this session was started via `aida queue work
    /// --stack` or `--base BRANCH`, the branch we forked from. `None` for
    /// the default case (forked from `origin/main`). Paired with
    /// `parent_branch_sha`; both are also reflected into `.aida/stacks.json`
    /// for the auto-rebase cascade.
    /// trace:STORY-248 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_branch: Option<String>,
    /// STORY-248: HEAD SHA of `parent_branch` at fork time. Critical for
    /// the cascade: the project squash-merges + deletes branches, so a
    /// later `git rebase origin/main` would replay the parent's
    /// pre-squash commits. The cascade uses
    /// `git rebase --onto origin/main <parent_branch_sha> <branch>` to
    /// skip them. `None` when `parent_branch` is `None`.
    /// trace:STORY-248 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_branch_sha: Option<String>,
    /// BUG-511: marks a lease minted by the human-review verb
    /// (`aida review <spec>`). A review lease is an advisory lock with NO
    /// worktree (`worktree_path` is empty, the TASK-474 convention) — its
    /// liveness signal is the `aida review` process itself (`creator_pid`):
    /// alive → the spec is in flight (being reviewed); dead → the lease is
    /// stale and always safe to reap (no worktree, no uncommitted work).
    /// trace:BUG-511 | ai:claude
    #[serde(default)]
    pub(crate) review_verb: bool,
    /// TASK-957: marks a lease minted by `aida claim <spec>` — a spec-scoped
    /// ADVISORY claim that makes advisor-fanned work (an Agent-tool subagent,
    /// which otherwise takes only a generic `harness-worktree` lease) visible
    /// to BUG-637's duplicate-dispatch gates. Like a review lease it has no
    /// worktree of its own (`worktree_path` may be empty when no `--worktree`
    /// was given), so its liveness signal is the claiming process recorded in
    /// `creator_pid`: alive → the claim still holds; dead → stale and ignored
    /// by every gate (no crash-deadlock). The scope IS the spec id, so the
    /// claim matches `live_spec_claim_by_other` / `lease_owning_spec` /
    /// `spec_scoped_lease` exactly like an AIDA-launched spec lease.
    // trace:TASK-957 | ai:claude
    #[serde(default)]
    pub(crate) claim_verb: bool,
    /// BUG-778: stamped when this worktree was handed to a HUMAN by
    /// `aida worktree enter|add` — the verbs that take the implementer lease
    /// and then deliberately launch NO agent (the operator starts one
    /// themselves). Its presence is the "was an agent ever expected here?"
    /// provenance `aida ps` needs: without it, the seconds between `enter` and
    /// the operator's launch look exactly like a crashed agent, and the row
    /// read "process dead — resume/rebrief: `aida queue work <spec>`" — a hint
    /// that would start a SECOND session competing with the hand-worked one.
    /// Refreshed on every re-enter (the worktree is being handed over again).
    /// Absent on orchestrator-spawned leases, so their crash detection is
    /// unchanged.
    // trace:BUG-778 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) manual_enter_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TASK-1518: stamped by the drain's SIGTERM handler on every lease the
    /// stopped wave created (`creator_pid` match) as it releases the drain
    /// lock — an INTERRUPTED session, not an abandoned one. `aida ps` reads
    /// it: a stamped lease with no live process and a clean tree classifies
    /// `stopped` (worktree intact, resume as normal) instead of a dead agent.
    /// Written as a generic TOML key by `drain_signal`, so a lease this
    /// binary does not otherwise model keeps its other keys.
    // trace:TASK-1518 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) interrupted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// TASK-1518: why the lease was interrupted (`sigterm`). Informational;
    /// the classifier keys on `interrupted_at`.
    // trace:TASK-1518 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) interrupted_reason: Option<String>,
}

pub(crate) fn leases_dir(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida").join("sessions")
}

/// Max display width for the @SCOPE statusline segment (matches @SPEC budget).
/// trace:STORY-55 | ai:claude
pub(crate) const SCOPE_LABEL_MAX: usize = 12;

/// Max display width for the `sess:` statusline label (scope + batch
/// suffix). Slightly wider than SCOPE_LABEL_MAX so common `epic-N#batchM`
/// shapes don't truncate the scope when the suffix is the new signal.
/// Shared by the standalone `sess:` segment and the `[sess:]` anchor
/// annotation folded into `@SPEC`. trace:TASK-60 trace:TASK-282 | ai:claude
pub(crate) const SESS_LABEL_MAX: usize = 20;

pub(crate) fn lease_path(project_root: &std::path::Path, id: &str) -> std::path::PathBuf {
    leases_dir(project_root).join(format!("{}.toml", id))
}

pub(crate) fn session_harness_branch(
    cwd: &std::path::Path,
    override_branch: Option<&str>,
) -> Result<String> {
    if let Some(branch) = override_branch.filter(|s| !s.trim().is_empty()) {
        return Ok(branch.to_string());
    }
    current_branch_at(cwd).ok_or_else(|| {
        anyhow::anyhow!(
            "could not derive branch for harness worktree `{}`",
            cwd.display()
        )
    })
}

pub(crate) fn session_harness_worktree_register(
    agent_id: &str,
    cwd: &str,
    agent_type: Option<&str>,
    branch: Option<&str>,
    scope: Option<&str>,
) -> Result<()> {
    let cwd = std::path::PathBuf::from(cwd);
    // The lease branch later reaches `git log` / `git diff` / `git cherry`;
    // never store one git would read as an option. trace:BUG-1622 | ai:claude
    if let Some(b) = branch {
        git_arg_guard::reject_option_like("--branch", b)?;
    }
    let branch = session_harness_branch(&cwd, branch)?;
    let payload = worktree_lease::SubagentPayload {
        agent_id: agent_id.to_string(),
        agent_type: agent_type.map(|s| s.to_string()),
        cwd: cwd.clone(),
    };
    let mut spec = worktree_lease::lease_spec_for_start(&payload, &branch);
    if let Some(scope) = scope.filter(|s| !s.trim().is_empty()) {
        spec.scope = scope.to_string();
    }

    let project_root = find_main_worktree_root()?;

    // BUG-1772 AC1: A SubagentStart whose cwd is the project root has no
    // dedicated worktree of its own. A "worktree lease" here would track nothing
    // and would erroneously absorb the liveness of any agent process in the root.
    // So we do not write a lease at all.
    // trace:BUG-1772 | ai:antigravity
    let cwd_canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
    let root_canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.clone());
    if cwd_canonical == root_canonical {
        return Ok(());
    }

    let id = worktree_lease::lease_id_from_agent_id(&spec.agent_id);
    let owner = aida_core::git_ops::git_config_get("user.email")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_string());
    let active_pid = process_probe::nearest_claude_ancestor_pid(std::process::id());
    let lease = SessionLease {
        id: id.clone(),
        scope: spec.scope.clone(),
        slug: slugify(&spec.scope),
        owner,
        worktree_path: spec
            .worktree_path
            .canonicalize()
            .unwrap_or_else(|_| spec.worktree_path.clone()),
        branch: spec.branch.clone(),
        started_at: chrono::Utc::now(),
        hostname: hostname(),
        role: spec.agent_type.clone(),
        creator_pid: None,
        creator_pid_start_time: None,
        // BUG-752: an Agent-tool subagent runs INSIDE the parent claude
        // harness process (cwd = parent project root, never the isolation
        // worktree), so the cwd-based worktree probe can never see it — a
        // pid-less harness lease read dormant/"dead process, salvageable"
        // while the agent was actively working. This register command runs
        // as a hook child of that same harness process: walk our own
        // ancestry and stamp the nearest live claude pid as the lease's
        // liveness signal — the agent-tool analog of BUG-741's headless
        // codex child-pid stamp. `None` when no claude ancestor is found
        // (e.g. a detached hook runner); the classifier then treats
        // liveness as unknown rather than dead. trace:BUG-752 | ai:claude
        active_pid,
        active_pid_start_time: active_pid.and_then(process_probe::process_start_identity),
        cargo_target_dir: None,
        parent_project_root: Some(
            project_root
                .canonicalize()
                .unwrap_or_else(|_| project_root.clone()),
        ),
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
    };

    std::fs::create_dir_all(leases_dir(&project_root))?;
    // STORY-1429: atomic, so a reader never sees a half-written lease.
    // trace:STORY-1429 | ai:claude
    aida_core::write_atomic(
        &lease_path(&project_root, &id),
        toml::to_string_pretty(&lease)?,
    )?;
    println!(
        "registered harness worktree lease {} scope:{} branch:{}",
        id, lease.scope, lease.branch
    );
    // BUG-1656: a subagent dispatched INTO a worktree an existing spec lease
    // owns adopts that lease — its harness pid becomes the spec lease's live
    // worker, so `aida ps` / `aida awaiting` stop calling the spec dead.
    // trace:BUG-1656 | ai:claude
    if let Some(adopted) =
        adopt_spec_lease_for_subagent(&project_root, &lease.worktree_path, active_pid)
    {
        println!(
            "adopted spec lease {adopted} for this subagent (its worktree is this subagent's cwd)"
        );
    }
    // BUG-754: a spec-scoped harness lease means a fanned-out implementer is
    // now working that spec — flip Approved → In Progress at lease-take (the
    // same coherence bump `aida session start` performs per BUG-379) so
    // `aida queue list` / `aida list` never show the same spec as both a
    // pickable Approved row and in-flight/leased.
    // trace:BUG-754 | ai:claude
    if bump_spec_in_progress_at_lease_take(&project_root, &lease.scope) {
        println!(
            "bumped {} Approved → In Progress (spec-scoped lease taken)",
            lease.scope
        );
    }
    Ok(())
}

/// BUG-1656 (2): is `lease_scope`/`lease_worktree` a spec-scoped lease whose
/// worktree is exactly the subagent's `cwd`? Pure so the adoption rule is
/// unit-testable without a lease dir. Generic harness leases and non-spec
/// scopes are never adopted; neither is a lease for a different worktree.
// trace:BUG-1656 | ai:claude
pub(crate) fn subagent_adopts_lease(
    lease_scope: &str,
    lease_worktree: &std::path::Path,
    cwd: &std::path::Path,
) -> bool {
    if lease_scope.eq_ignore_ascii_case(worktree_lease::HARNESS_WORKTREE_SCOPE) {
        return false;
    }
    if worktree_lease::spec_id_from_branch(lease_scope).is_none() {
        return false;
    }
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    !lease_worktree.as_os_str().is_empty() && canon(lease_worktree) == canon(cwd)
}

/// BUG-1656 (2): give an Agent-tool subagent the spec lease of the worktree it
/// was dispatched into. Walks `.aida/sessions/`, finds the spec-scoped lease
/// whose `worktree_path` is `cwd`, and — unless that lease already records a
/// LIVE `active_pid` — stamps `harness_pid` as its `active_pid` (plus the
/// kernel start identity and an `adopted_by_subagent_at` marker). Patched as
/// generic TOML key inserts (the `manual_enter_at` pattern) so keys this
/// binary does not model survive. Returns the adopted lease id. Best-effort:
/// unreadable leases are skipped, never rewritten.
// trace:BUG-1656 | ai:claude
pub(crate) fn adopt_spec_lease_for_subagent(
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    harness_pid: Option<u32>,
) -> Option<String> {
    let pid = harness_pid?;
    let entries = std::fs::read_dir(leases_dir(project_root)).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut value) = toml::from_str::<toml::Value>(&body) else {
            continue;
        };
        let Some(table) = value.as_table_mut() else {
            continue;
        };
        let scope = table
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let worktree = table
            .get("worktree_path")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        if !subagent_adopts_lease(&scope, &worktree, cwd) {
            continue;
        }
        let already_live = table
            .get("active_pid")
            .and_then(|v| v.as_integer())
            .and_then(|v| u32::try_from(v).ok())
            .is_some_and(process_probe::pid_is_alive);
        if already_live {
            continue;
        }
        table.insert(
            "active_pid".to_string(),
            toml::Value::Integer(i64::from(pid)),
        );
        match process_probe::process_start_identity(pid) {
            Some(start) => {
                table.insert(
                    "active_pid_start_time".to_string(),
                    toml::Value::String(start),
                );
            }
            None => {
                table.remove("active_pid_start_time");
            }
        }
        table.insert(
            "adopted_by_subagent_at".to_string(),
            toml::Value::String(chrono::Utc::now().to_rfc3339()),
        );
        let id = table
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
        let Ok(content) = toml::to_string_pretty(&value) else {
            continue;
        };
        if write_atomic(&path, &content).is_ok() {
            return Some(id);
        }
    }
    None
}

// BUG-754: does this harness-lease scope name a spec (vs the generic
// `harness-worktree` scope or a non-spec label)? Returns the canonical
// SPEC-ID when it does. Pure, so the fanout-pickup bump gate is
// unit-testable without a store fixture.
// trace:BUG-754 | ai:claude
pub(crate) fn harness_lease_spec_scope(scope: &str) -> Option<String> {
    if scope.eq_ignore_ascii_case(worktree_lease::HARNESS_WORKTREE_SCOPE) {
        return None;
    }
    worktree_lease::spec_id_from_branch(scope)
}

// BUG-754: at lease-take for a spec-scoped harness lease, bump the spec
// Approved → In Progress — the same coherence flip `aida session start`
// performs after saving its lease (BUG-379) — so a harness-fanout
// implementer's spec never reads as a pickable Approved row while an active
// lease reports it in flight. Targeted single-spec write (the BUG-634 rule:
// resolve the one spec via the cache-backed backend and rewrite only its
// YAML); this runs from the SubagentStart hook's small time budget, so a
// full-store load/save is not acceptable here. Idempotent: any status other
// than Approved is left alone — the lifecycle owns those transitions (drain
// completion still lands Done via `aida queue done`, a shelve still lands
// NeedsAttention, and `aida doctor` restores In Progress-without-lease back
// to Approved). Failure is non-fatal: the doctor's spec-status-drift heal
// ("Approved but lease active → bump to In Progress") is the backstop.
// trace:BUG-754 | ai:claude
pub(crate) fn bump_spec_in_progress_at_lease_take(
    project_root: &std::path::Path,
    scope: &str,
) -> bool {
    let Some(spec_id) = harness_lease_spec_scope(scope) else {
        return false;
    };
    let store_root = project_root.join(".aida-store");
    if !store_root.is_dir() {
        // Legacy/centralized store — skip; the doctor heal covers the drift.
        return false;
    }
    let result = (|| -> Result<bool> {
        let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_root);
        let backend = aida_core::CachedGitBackend::open(&store_root, &cache_path)?;
        // trace:TASK-1468 | ai:claude
        let Some(req) = backend.get_requirement_unambiguous(&spec_id)? else {
            return Ok(false);
        };
        // BUG-1638: the race-test seam between the read and the write.
        // trace:BUG-1638 | ai:claude
        status_write_race_seam(&store_root);
        // BUG-1637: the status check runs on the copy read under the store
        // lock; a refused bump writes nothing. trace:BUG-1637 | ai:claude
        let mut bumped = false;
        backend.update_spec_atomically(&req, |r| {
            bumped = approved_to_in_progress_bump(r, aida_core::conflict::LEASE_TAKE_AUTHOR);
        })?;
        Ok(bumped)
    })();
    match result {
        Ok(bumped) => bumped,
        Err(e) => {
            eprintln!(
                "warning: couldn't bump `{}` to In Progress at lease-take: {} — `aida doctor` will heal",
                scope, e
            );
            false
        }
    }
}

// BUG-754: fanout-pickup lease-take status-bump tests.
// trace:BUG-754 | ai:claude
#[cfg(test)]
#[path = "tests/harness_lease_bump_tests.rs"]
mod harness_lease_bump_tests;

pub(crate) fn session_harness_worktree_release(agent_id: &str) -> Result<()> {
    let project_root = find_main_worktree_root()?;
    let payload = worktree_lease::SubagentPayload {
        agent_id: agent_id.to_string(),
        agent_type: None,
        cwd: std::path::PathBuf::new(),
    };
    let id = worktree_lease::lease_id_for_stop(&payload);
    let path = lease_path(&project_root, &id);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            println!("released harness worktree lease {}", id);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// Lease-occupancy primitive: find a lease that already occupies `worktree` and
/// is still live. Pure over the lease set + an injected liveness predicate: the
/// caller supplies PID-liveness / dormancy (via `creator_pid` + the same logic
/// `classify_for_auto_release` uses), so a dead or auto-released lease's worktree
/// is correctly seen as free. Path comparison is exact — the caller canonicalizes
/// `worktree` to match the canonicalized `lease.worktree_path`.
///
/// NOTE: BUG-416's shipped fix is the attribution-aware `aida add` hint, which
/// gates on *agent* occupancy (`agent_registry::live_agents_covering_cwd`), not
/// lease occupancy — so this primitive is currently unused. Retained as the
/// natural building block for lease-hygiene work that needs "is this worktree
/// still claimed?" (e.g. BUG-362 pr-ship lease cleanup). trace:TASK-607 | ai:claude
#[allow(dead_code)] // unused since BUG-416 chose agent-occupancy; kept for lease-hygiene reuse
pub(crate) fn worktree_occupant<'a>(
    worktree: &std::path::Path,
    leases: &'a [SessionLease],
    is_live: impl Fn(&SessionLease) -> bool,
) -> Option<&'a SessionLease> {
    leases
        .iter()
        .find(|l| l.worktree_path == worktree && is_live(l))
}

pub(crate) fn list_leases(project_root: &std::path::Path) -> Vec<SessionLease> {
    let dir = leases_dir(project_root);
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // trace:BUG-1814 | ai:antigravity
            if !file_name.ends_with(".toml")
                || file_name.ends_with(".manifest.toml")
                || file_name.ends_with(".activity.toml")
            {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&p) {
                if let Ok(lease) = toml::from_str::<SessionLease>(&content) {
                    out.push(lease);
                }
            }
        }
    }
    out.sort_by_key(|l| l.started_at);
    out
}

/// BUG-574: pure decision — does THIS clone already hold a lease for `scope`?
/// Returns the existing lease (the first by start time, matching `list_leases`'
/// sort) so `aida session start` can treat a re-run as benign idempotent
/// re-entry (exit 0, report the live session) rather than a hard error.
/// Case-insensitive on the raw scope string, mirroring the old guard.
/// trace:BUG-574 | ai:claude
pub(crate) fn existing_lease_for_scope<'a>(
    leases: &'a [SessionLease],
    scope: &str,
) -> Option<&'a SessionLease> {
    leases.iter().find(|l| l.scope.eq_ignore_ascii_case(scope))
}

/// BUG-483: pure decision — does any lease OTHER than the one being ended
/// share `worktree_path`? `aida agent new` can land two sessions in one
/// worktree (BUG-416), and `session end` force-removes the worktree
/// unconditionally — which would strand the peer (TASK-0396 cross-worktree
/// fingerprint hazard + BUG-108 `(deleted)` cwd). We filter the ending lease
/// out by id and compare canonicalized paths so symlink chains don't mask a
/// match. Conservative: returns the first peer found so the caller can SKIP
/// the removal and name the peer. trace:BUG-483 | ai:claude
pub(crate) fn peer_lease_sharing_worktree<'a>(
    leases: &'a [SessionLease],
    ending_id: &str,
    worktree_path: &std::path::Path,
) -> Option<&'a SessionLease> {
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let target_canon = canon(worktree_path);
    leases.iter().find(|l| {
        l.id != ending_id
            && !l.worktree_path.as_os_str().is_empty()
            && canon(&l.worktree_path) == target_canon
    })
}

/// TASK-670: the set of requirement UUIDs that sit in *some* role queue
/// (across every user's queue file), for the leading queued work-routing glyph on
/// `aida list`. Cheap by design — one `read_dir` over
/// `.aida-store/registry/queues/` + a YAML parse per file, no Storage::load().
/// Work-routing is a project-global axis (not the current shell's queue), so we
/// union across all queue files. Returns an empty set when the dir is absent.
/// trace:TASK-670 | ai:claude
pub(crate) fn all_queued_requirement_ids(project_root: &std::path::Path) -> HashSet<Uuid> {
    let mut out = HashSet::new();
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
        if let Ok(items) = serde_yaml::from_str::<Vec<aida_core::QueueEntry>>(&content) {
            for item in items {
                out.insert(item.requirement_id);
            }
        }
    }
    out
}

/// The queue-insertion timestamp per requirement — requirement UUID → the
/// EARLIEST `added_at` across every user's queue file. The substrate fact the
/// burndown's ready-set ordering is anchored on: "when did this spec enter the
/// queue", as opposed to the accident of how its spec id happens to spell.
///
/// Same cheap `read_dir` + per-file YAML parse as [`all_queued_requirement_ids`]
/// (no `Storage::load()`), and the same project-global union: a spec routed into
/// two role queues carries the earlier of the two stamps, so re-routing work
/// never silently moves it to the back of the wave. Returns an empty map when
/// the queue dir is absent.
// trace:TASK-1175 | ai:claude
pub(crate) fn all_queued_added_at(
    project_root: &std::path::Path,
) -> std::collections::HashMap<Uuid, chrono::DateTime<chrono::Utc>> {
    let mut out: std::collections::HashMap<Uuid, chrono::DateTime<chrono::Utc>> =
        std::collections::HashMap::new();
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
        if let Ok(items) = serde_yaml::from_str::<Vec<aida_core::QueueEntry>>(&content) {
            for item in items {
                out.entry(item.requirement_id)
                    .and_modify(|at| {
                        if item.added_at < *at {
                            *at = item.added_at;
                        }
                    })
                    .or_insert(item.added_at);
            }
        }
    }
    out
}

/// BUG-527: queue memberships for a single spec, across every user's queue
/// file, for the `Queued:` line on `aida show <ID>` (and the MCP
/// `show_requirement` mirror). Each membership is `(for_role, rank)`, where
/// `for_role` is the canonical routing role (`None` = unrouted / general
/// queue) and `rank` is the 1-based position the operator sees on
/// `aida queue list` — the index within that role's queue ordered by raw
/// on-disk position, NOT the sparse `entry.position` value (STORY-72's
/// `max_position + 1000` scheme means the raw value is not a human rank).
/// Reuses the same cheap `read_dir` + per-file YAML parse as
/// [`all_queued_requirement_ids`] (no `Storage::load()`); queue membership
/// is a project-global axis so we union across all queue files. Returns an
/// empty vec when the dir is absent or the spec sits in no queue (the
/// caller omits the line in that case). trace:BUG-527
pub(crate) fn queue_memberships_for(
    project_root: &std::path::Path,
    requirement_id: &Uuid,
) -> Vec<(Option<String>, usize)> {
    let mut out: Vec<(Option<String>, usize)> = Vec::new();
    let dir = project_root.join(".aida-store/registry/queues");
    let Ok(dir_entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for dirent in dir_entries.flatten() {
        let p = dirent.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(mut items) = serde_yaml::from_str::<Vec<aida_core::QueueEntry>>(&content) else {
            continue;
        };
        // Rank is per-role within this user's queue, ordered by raw position
        // (the same ordering `aida queue list` groups + numbers by).
        items.sort_by_key(|e| e.position);
        let mut rank_by_role: std::collections::HashMap<Option<String>, usize> =
            std::collections::HashMap::new();
        for item in &items {
            let role = item.for_role.as_deref().map(canonical_role_name);
            let rank = rank_by_role.entry(role.clone()).or_insert(0);
            *rank += 1;
            if &item.requirement_id == requirement_id {
                out.push((role, *rank));
            }
        }
    }
    // Stable presentation order: routed roles alphabetically, unrouted last,
    // then by rank.
    out.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => x.cmp(y).then(a.1.cmp(&b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.cmp(&b.1),
    });
    out
}

/// BUG-527: render the `(for_role, rank)` memberships from
/// [`queue_memberships_for`] as the value half of the `Queued:` line, e.g.
/// `implementer (pos 2)` or `implementer (pos 2), reviewer (pos 1)`.
/// Unrouted entries render as `general (pos N)`. Returns `None` when there
/// are no memberships so the caller can omit the line entirely.
/// trace:BUG-527
pub(crate) fn format_queue_membership(memberships: &[(Option<String>, usize)]) -> Option<String> {
    if memberships.is_empty() {
        return None;
    }
    let parts: Vec<String> = memberships
        .iter()
        .map(|(role, rank)| {
            let label = role.as_deref().unwrap_or("general");
            format!("{} (pos {})", label, rank)
        })
        .collect();
    Some(parts.join(", "))
}

/// TASK-670: the set of scope strings held by *live* session leases — the input
/// for the in-flight work-routing glyph. "Live" means a running claude was
/// found inside the lease's worktree (the same probe `aida session leases` uses
/// for its `live` state); dormant / stale / reaped leases are excluded so a
/// dead session never shows the in-flight marker (cf. STORY-496). Scopes are lowercased for a
/// case-insensitive match against a row's spec/agreed id. trace:TASK-670 | ai:claude
pub(crate) fn in_flight_lease_scopes(project_root: &std::path::Path) -> HashSet<String> {
    in_flight_lease_role_map(project_root).into_keys().collect()
}

// TASK-1267: `--express` is a one-release compatibility alias for ordinary
// mode=drain intake. It deliberately emits no batch:express or lifecycle tag.
// The default fasttrack filing convention is unchanged.
// trace:TASK-1267 | ai:codex
pub(crate) fn fasttrack_lane_filing(
    express: bool,
) -> (Option<String>, Option<String>, Option<String>) {
    if express {
        (None, None, Some("drain".to_string()))
    } else {
        (
            Some("fasttrack".to_string()),
            Some("lifecycle:no-review".to_string()),
            None,
        )
    }
}

// The eight lane stages a fasttrack/express item can occupy, in roughly
// request-to-ship order. This is a *projection* — there is no new store; the
// stage is derived from the spec's existing status, queue membership, active
// lease, punt ledger, and merged (Completed) state.
// trace:TASK-905 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FasttrackStage {
    /// Draft — filed, not yet dispositioned.
    Requested,
    /// Approved — blessed but not yet on a queue.
    Accepted,
    /// Approved/Planned and sitting in a role queue, awaiting a drain owner.
    Queued,
    /// InProgress, or a live lease holds the spec (a drain is working it).
    Running,
    /// NeedsAttention with no punt record — parked, but not a recorded punt.
    Blocked,
    /// NeedsAttention with a punt record — left the lane loudly (the punt-out
    /// invariant: non-trivial discovery parks + records a punt).
    Punted,
    /// Done or Completed — work finished (Completed == merged to main).
    Shipped,
    /// Rejected — declined; will not be done.
    Rejected,
}

impl FasttrackStage {
    /// Lowercase slug used in CLI output and the `--json` `stage` field.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            FasttrackStage::Requested => "requested",
            FasttrackStage::Accepted => "accepted",
            FasttrackStage::Queued => "queued",
            FasttrackStage::Running => "running",
            FasttrackStage::Blocked => "blocked",
            FasttrackStage::Punted => "punted",
            FasttrackStage::Shipped => "shipped",
            FasttrackStage::Rejected => "rejected",
        }
    }
}

/// Pure stage projection for a single lane item. Kept side-effect-free (takes
/// already-derived booleans, returns the stage) so it is unit-testable without
/// a store, a queue dir, or a lease probe.
///
/// `status_str` is the cache's status string (the `Debug` form of
/// `RequirementStatus` — `Draft`, `Approved`, `Planned`, `InProgress`, `Done`,
/// `Completed`, `Rejected`, `NeedsAttention`); matching is case-insensitive so a
/// `custom_status` row degrades gracefully (unknown → its closest status, else
/// `requested`). `is_queued` = in some role queue; `is_running` = a live lease
/// holds it; `has_punt` = an (unresolved) punt record names it.
///
/// Mapping (per the fasttrack-lane design's status→stage table):
///   Draft            → requested
///   Approved         → accepted, or queued (in a queue), or running (live lease)
///   Planned          → queued, or running (live lease)
///   InProgress       → running
///   NeedsAttention   → punted (has a punt record) else blocked
///   Done | Completed → shipped
///   Rejected         → rejected
// trace:TASK-905 | ai:claude — see docs/plans/2026-06-26-task-0438-fasttrack-lane.md
pub(crate) fn project_fasttrack_stage(
    status_str: &str,
    is_queued: bool,
    is_running: bool,
    has_punt: bool,
) -> FasttrackStage {
    match status_str.to_ascii_lowercase().as_str() {
        "draft" => FasttrackStage::Requested,
        "approved" => {
            if is_running {
                FasttrackStage::Running
            } else if is_queued {
                FasttrackStage::Queued
            } else {
                FasttrackStage::Accepted
            }
        }
        "planned" => {
            if is_running {
                FasttrackStage::Running
            } else {
                FasttrackStage::Queued
            }
        }
        "inprogress" => FasttrackStage::Running,
        "needsattention" => {
            if has_punt {
                FasttrackStage::Punted
            } else {
                FasttrackStage::Blocked
            }
        }
        "done" | "completed" => FasttrackStage::Shipped,
        "rejected" => FasttrackStage::Rejected,
        // Unknown / custom status: a live lease or queue membership still tells
        // us where it is; otherwise treat it as the entry stage.
        _ => {
            if is_running {
                FasttrackStage::Running
            } else if is_queued {
                FasttrackStage::Queued
            } else {
                FasttrackStage::Requested
            }
        }
    }
}

/// `aida fasttrack status` — the lane stage projection.
///
/// Cache-fast: it reuses `backend.list_summaries` (the same projection
/// `aida list` reads) for the current fasttrack and legacy express buckets, and
/// the cheap queue-dir scan + live-lease probe + punt-ledger read the list view
/// already uses for its routing glyphs — NOT the full-store `aida status` scan.
// trace:TASK-905 | ai:claude
pub(crate) fn handle_fasttrack_status(
    store_path: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    json: bool,
) -> Result<()> {
    // Each lane bucket is one tag filter. The cache's tag filter is AND-only, so
    // we run one query per bucket (with archive/defer set to Both so a
    // parked/archived lane item still surfaces in its own status surface) and
    // tag each row with the bucket it came from. A row carrying BOTH tags is
    // kept once, under fasttrack (the more-trivial tier) — fasttrack is listed
    // first so the de-dup HashSet skips the express copy.
    let mut rows: Vec<(&'static str, aida_core::RequirementSummary)> = Vec::new();
    let mut seen: HashSet<Uuid> = HashSet::new();
    for (bucket, tag) in [
        ("fasttrack", "batch:fasttrack"),
        // trace:TASK-1267 | ai:codex — read-only legacy compatibility.
        ("express", "batch:express"),
    ] {
        let filter = aida_core::ListFilter {
            tags: vec![tag.to_string()],
            archive: aida_core::ArchiveFilter::Both,
            defer: aida_core::DeferFilter::Both,
            ..Default::default()
        };
        for s in backend.list_summaries(&filter)? {
            if seen.insert(s.id) {
                rows.push((bucket, s));
            }
        }
    }

    // Routing signals — the same cheap reads `aida list` uses for its flow
    // glyphs. project root = parent of the `.aida-store` worktree.
    let routing_root = store_path.parent().map(|p| p.to_path_buf());
    let queued_ids: HashSet<Uuid> = routing_root
        .as_deref()
        .map(all_queued_requirement_ids)
        .unwrap_or_default();
    let in_flight_scopes: HashSet<String> = routing_root
        .as_deref()
        .map(in_flight_lease_scopes)
        .unwrap_or_default();
    // Specs named by an unresolved punt record (distinguishes punted vs blocked
    // within NeedsAttention). `read_ledger` is a single JSONL read.
    let punted_specs: HashSet<String> = routing_root
        .as_deref()
        .map(|root| {
            punt::read_ledger(root)
                .into_iter()
                .map(|r| r.spec.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();

    let stage_of = |bucket: &str, r: &aida_core::RequirementSummary| -> FasttrackStage {
        let is_queued = queued_ids.contains(&r.id);
        let is_running = [r.agreed_id.as_deref(), r.spec_id.as_deref()]
            .into_iter()
            .flatten()
            .any(|id| in_flight_scopes.contains(&id.to_ascii_lowercase()));
        let has_punt = [r.agreed_id.as_deref(), r.spec_id.as_deref()]
            .into_iter()
            .flatten()
            .any(|id| punted_specs.contains(&id.to_ascii_lowercase()));
        let _ = bucket;
        project_fasttrack_stage(&r.status, is_queued, is_running, has_punt)
    };

    if json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|(bucket, r)| {
                let stage = stage_of(bucket, r);
                serde_json::json!({
                    "spec_id": r.spec_id.as_deref().or(r.agreed_id.as_deref()),
                    "title": r.title,
                    "tier": bucket,
                    "status": r.status,
                    "stage": stage.slug(),
                })
            })
            .collect();
        println!("{}", crate::cache_output::json_pretty(&items)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("{}", "No fasttrack-lane items.".yellow());
        return Ok(());
    }

    // Order rows by lane progression so the surface reads request → ship.
    let stage_rank = |s: FasttrackStage| -> u8 {
        match s {
            FasttrackStage::Requested => 0,
            FasttrackStage::Accepted => 1,
            FasttrackStage::Queued => 2,
            FasttrackStage::Running => 3,
            FasttrackStage::Blocked => 4,
            FasttrackStage::Punted => 5,
            FasttrackStage::Shipped => 6,
            FasttrackStage::Rejected => 7,
        }
    };
    let mut ordered: Vec<(&'static str, &aida_core::RequirementSummary, FasttrackStage)> = rows
        .iter()
        .map(|(bucket, r)| (*bucket, r, stage_of(bucket, r)))
        .collect();
    ordered.sort_by_key(|(_, _, stage)| stage_rank(*stage));

    println!("{}  {} item(s)", "Fasttrack lane".bold(), ordered.len());
    println!(
        "{}",
        "requested → accepted → queued → running → blocked → punted → shipped → rejected".dimmed()
    );
    println!("{}", "─".repeat(72));
    for (bucket, r, stage) in &ordered {
        let id = r
            .spec_id
            .as_deref()
            .or(r.agreed_id.as_deref())
            .unwrap_or("-");
        let stage_disp = match stage {
            FasttrackStage::Requested => stage.slug().yellow(),
            FasttrackStage::Accepted => stage.slug().blue(),
            FasttrackStage::Queued => stage.slug().cyan(),
            FasttrackStage::Running => stage.slug().magenta(),
            FasttrackStage::Blocked | FasttrackStage::Punted => stage.slug().red().bold(),
            FasttrackStage::Shipped => stage.slug().green(),
            FasttrackStage::Rejected => stage.slug().red(),
        };
        println!("{:<10} {:<11} [{}] {}", id, stage_disp, bucket, r.title);
    }
    Ok(())
}

/// BUG-511: lease-state classification that understands review-verb
/// advisory leases. A review lease has no worktree, so the standard
/// worktree/claude/age matrix would always call it stale; its real
/// liveness signal is the `aida review` process recorded in
/// `creator_pid`. Session leases fall through to [`classify_lease_state`]
/// unchanged. trace:BUG-511 | ai:claude
pub(crate) fn lease_state_for(
    l: &SessionLease,
    live_sessions: &[process_probe::LiveSession],
    now: chrono::DateTime<chrono::Utc>,
) -> LeaseState {
    // BUG-511 review leases AND TASK-957 claim leases are advisory locks when
    // they have no worktree of their own — the standard worktree/claude/age
    // matrix would always call them stale. Their real liveness signal is the
    // process that minted them, recorded in `creator_pid`: alive → Live,
    // dead/absent → Stale (never Dormant; an advisory lock's lifetime is
    // exactly its process's lifetime). BUG-882 lets reviewer worktree sessions
    // carry review_verb too; those must still use normal worktree liveness.
    // trace:BUG-511 trace:TASK-957 | ai:claude
    // trace:BUG-882 | ai:codex
    if (l.review_verb || l.claim_verb) && l.worktree_path.as_os_str().is_empty() {
        let alive = l
            .creator_pid
            .map(|pid| {
                process_probe::process_identity_is_alive(pid, l.creator_pid_start_time.as_deref())
            })
            .unwrap_or(false);
        return if alive {
            LeaseState::Live
        } else {
            LeaseState::Stale
        };
    }
    if let Some(pid) = l.active_pid {
        return if process_probe::process_identity_is_alive(pid, l.active_pid_start_time.as_deref())
        {
            LeaseState::Live
        } else {
            LeaseState::Stale
        };
    }
    let worktree_exists = l.worktree_path.exists();
    let has_live_claude = live_sessions
        .iter()
        .any(|s| !s.stale_cwd && (s.cwd == l.worktree_path || s.cwd.starts_with(&l.worktree_path)));
    let age_hours = now.signed_duration_since(l.started_at).num_hours();
    classify_lease_state(worktree_exists, has_live_claude, age_hours)
}

/// BUG-98: cheap lease count for the `aida session list` footer hint.
/// Users repeatedly reach for `session list` looking for "active
/// sessions" when they really want `session leases` (the scoped-lease
/// view). Exposing the count here lets the historical-view command
/// nudge them at the right moment. Tries the current project root first
/// and silently falls through to 0 if we're not inside a project.
/// trace:BUG-98 | ai:claude
pub(crate) fn active_lease_count_for_cwd() -> usize {
    match find_project_root() {
        Ok(root) => list_leases(&root).len(),
        Err(_) => 0,
    }
}

/// Find the active session lease whose worktree contains `cwd`, if any.
/// Used by statusline + enforcement to identify which session "owns" the
/// shell the user is operating from.
/// trace:STORY-55 | ai:claude
pub(crate) fn active_lease_for_cwd(
    project_root: &std::path::Path,
    cwd: &std::path::Path,
) -> Option<SessionLease> {
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    list_leases(project_root)
        .into_iter()
        .find(|l| lease_covers_cwd(l, &canon))
}

/// Whether `lease` covers `canon_cwd` — true iff the lease records a real
/// worktree path AND `canon_cwd` is exactly that path or a descendant of it.
///
/// TASK-474: leases with an empty `worktree_path` are advisory locks with
/// no session context (e.g. an MCP `claim_task` lease that didn't pass a
/// `worktree_path` argument — see `aida-cli/src/mcp.rs::tool_claim_task`).
/// `Path::starts_with` treats every path as starting with the empty path,
/// so an empty-worktree lease would otherwise match every cwd and misroute
/// "this session owns scope X" hints in `aida add` to unrelated shells.
/// trace:TASK-474 | ai:claude
pub(crate) fn lease_covers_cwd(lease: &SessionLease, canon_cwd: &std::path::Path) -> bool {
    if lease.worktree_path.as_os_str().is_empty() {
        return false;
    }
    #[cfg(windows)]
    {
        let lease_key = windows_path_key(&lease.worktree_path.to_string_lossy());
        let cwd_key = windows_path_key(&canon_cwd.to_string_lossy());
        return cwd_key == lease_key
            || cwd_key
                .strip_prefix(&lease_key)
                .is_some_and(|tail| tail.starts_with('/'));
    }
    #[cfg(not(windows))]
    {
        canon_cwd == lease.worktree_path || canon_cwd.starts_with(&lease.worktree_path)
    }
}

/// Normalize Windows spellings used by `Path::display()` and `canonicalize()`
/// before comparing a recorded lease path with the current worktree path.
/// The casing fold is Unicode-aware (`str::to_lowercase`, not
/// `to_ascii_lowercase`) because NTFS path matching is case-insensitive for
/// non-ASCII names too — a lease recorded under `Åsa` must match a cwd
/// canonicalized as `åsa`.
// trace:BUG-1794 | ai:codex
// trace:BUG-1794 | ai:claude
#[cfg(windows)]
pub(crate) fn windows_path_key(path: &str) -> String {
    let path = path.replace('\\', "/").to_lowercase();
    let path = path
        .strip_prefix("//?/unc/")
        .map(|unc| format!("//{unc}"))
        .or_else(|| path.strip_prefix("//?/").map(str::to_string))
        .unwrap_or(path);
    path.trim_end_matches('/').to_string()
}

#[cfg(test)]
#[path = "tests/lease_covers_cwd_tests.rs"]
mod lease_covers_cwd_tests;

/// STORY-58: from inside a session worktree, return the parent project's
/// root path so cross-worktree views (currently just `aida session list`)
/// can also surface sessions launched from the parent. `None` when:
///   - cwd isn't covered by an active lease (not in a session), OR
///   - the lease was written before STORY-58 (no parent recorded), OR
///   - we can't locate any project root to read leases from.
///     trace:STORY-58 | ai:claude
pub(crate) fn parent_project_root_for_session(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    // The lease dir is `<root>/.aida/sessions/`. Inside a session worktree
    // that dir is a symlink back to the parent project's, so `list_leases`
    // returns the same set either way. Walk up from cwd looking for any
    // ancestor that has `.aida/sessions/`, then ask for its lease set.
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut probe = canon.as_path();
    loop {
        // BUG-1598: never adopt a temp root itself as the project root —
        // see `aida_core::store_locate` for the shared rationale.
        // trace:BUG-1598 | ai:claude
        if aida_core::store_locate::is_system_temp_dir(probe) {
            return None;
        }
        if probe.join(".aida").join("sessions").is_dir() {
            let lease = active_lease_for_cwd(probe, &canon)?;
            return lease.parent_project_root;
        }
        match probe.parent() {
            Some(p) => probe = p,
            None => return None,
        }
    }
}

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

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SessionActivityLog {
    #[serde(default)]
    pub(crate) entries: Vec<SessionActivityEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SessionActivityEntry {
    /// Role active when this entry was recorded — needed at session-end to
    /// know which project-level role file to fold the entry back into.
    pub(crate) role: String,
    pub(crate) spec_id: String,
    pub(crate) action: String,
    pub(crate) at: chrono::DateTime<chrono::Utc>,
}

/// Hard cap on session activity entries — keeps the file small for long
/// sessions. Older-than-cap entries fall off the back (so the newest cap
/// entries are kept, matching the project-level role's stream behavior).
pub(crate) const SESSION_ACTIVITY_MAX: usize = 200;

pub(crate) fn session_activity_path(
    project_root: &std::path::Path,
    id: &str,
) -> std::path::PathBuf {
    leases_dir(project_root).join(format!("{}.activity.toml", id))
}

pub(crate) fn load_session_activity(
    project_root: &std::path::Path,
    id: &str,
) -> SessionActivityLog {
    let path = session_activity_path(project_root, id);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return SessionActivityLog::default();
    };
    toml::from_str(&content).unwrap_or_default()
}

pub(crate) fn save_session_activity(
    project_root: &std::path::Path,
    id: &str,
    log: &SessionActivityLog,
) -> Result<()> {
    let path = session_activity_path(project_root, id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(log)?;
    // BUG-228: atomic write — guards the session activity log against the
    // same torn-concurrent-write corruption as the project-level role file.
    write_atomic(&path, &content).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Append an activity entry to the given session's log. Dedupes against
/// the most recent entry the same way project-level activity does:
/// consecutive (role, spec_id, action) collapse into one entry whose
/// timestamp ticks forward.
/// trace:STORY-56 | ai:claude
pub(crate) fn append_session_activity(
    project_root: &std::path::Path,
    session_id: &str,
    role: &str,
    spec_id: &str,
    action: &str,
) -> Result<()> {
    let mut log = load_session_activity(project_root, session_id);
    let entry = SessionActivityEntry {
        role: role.to_string(),
        spec_id: spec_id.to_string(),
        action: action.to_string(),
        at: chrono::Utc::now(),
    };
    // BUG-65: LRU-by-(role, spec_id, action). Drop any prior entry with
    // the same key, then insert at the front — interleaved actions across
    // specs no longer accumulate stale duplicates.
    // trace:BUG-65 | ai:claude
    log.entries.retain(|prev| {
        !(prev.role == entry.role && prev.spec_id == entry.spec_id && prev.action == entry.action)
    });
    log.entries.insert(0, entry);
    log.entries.truncate(SESSION_ACTIVITY_MAX);
    save_session_activity(project_root, session_id, &log)
}

/// BUG-112: the most-recent spec a session lease touched, read from its
/// session activity log. `entries` are newest-first, so the head entry's
/// `spec_id` is the session's current focus. `None` when the log is empty
/// or absent. Used by `aida session list`'s RECENT FOCUS column.
/// trace:BUG-112 | ai:claude
pub(crate) fn session_log_recent_spec(
    project_root: &std::path::Path,
    lease_id: &str,
) -> Option<String> {
    load_session_activity(project_root, lease_id)
        .entries
        .into_iter()
        .next()
        .map(|e| e.spec_id)
}

/// STORY-57: queue routing filter. Returns true if `entry`'s scope/session
/// routing tags are compatible with the consumer side (the shell calling
/// `queue list` / `queue next`).
///
/// Rules:
///   - `for_scope` set on the entry must match the consumer's lease scope
///     (case-insensitive). No lease + scope-tagged entry → filtered out
///     (the entry is targeted at a session that isn't this shell).
///   - `for_session` set on the entry must match the consumer lease's id
///     (8+ char prefix on either side, since a user might have typed a
///     short prefix).
///   - Either field absent on the entry = unrouted on that axis = visible
///     to all consumers.
///
/// Bypass with `bypass=true` (used for `--all` / explicit `--scope any`).
/// trace:STORY-57 | ai:claude
/// TASK-44: walk `req`'s relationships for a Child edge whose target is
/// an Epic-typed requirement and return that Epic's display id
/// (`agreed_id` when merged-trunk-canonical, else `spec_id`). A `Child`
/// relationship semantically means "this req IS a child of target" —
/// the inverse of `Parent`. Used by `aida queue list` to auto-derive
/// the `[@EPIC-XX*]` cluster chip for entries that weren't explicitly
/// queued with `--scope`. Returns None when:
///   - the req has no Child relationship, OR
///   - the target id doesn't resolve to a requirement in `store`, OR
///   - the target's type isn't Epic.
///     Pure — no I/O — so the decision rules can be unit-tested without
///     fixtures. trace:TASK-44 | ai:claude
///     TASK-91: detect a synthetic review-story title `Review PR-<n>:` filed
///     by STORY-90's auto-queue and return a display tuple where PR-N is the
///     prominent id and the canonical STORY-NNN is the parenthetical
///     secondary. Returns `None` when the title doesn't match the pattern;
///     callers fall back to the unchanged (display_id, title) pair.
///
/// Rendering convention (Option A from TASK-91):
///   STORY-108  Review PR-15: EPIC-23 wrap-up      →  display_id stays STORY-108
///                                                    title stays "Review PR-15: EPIC-23 wrap-up"
///   becomes:
///   PR-15 (STORY-108)  EPIC-23 wrap-up            →  prominent PR-N, dual id, trimmed title
///
/// PR-N is the conceptual handle the user reaches for ("I need to review
/// PR-15"); STORY-NNN stays visible for direct `aida edit` operations.
/// trace:TASK-91 | ai:claude
pub(crate) fn format_review_story_display(
    display_id: &str,
    title: &str,
) -> Option<(String, String)> {
    // BUG-91: match the `Review PR-` prefix case-insensitively so a
    // hand-typed `review pr-15: foo` renders consistently with how
    // `review_title_matches` (also case-insensitive) routes it. Slice the
    // ORIGINAL title (not a lowercased copy) so the after-colon title keeps
    // its case.
    const PREFIX: &str = "Review PR-";
    // BUG-525: `str::get(..n)` returns None when `n` is out of bounds OR not a
    // char boundary, so a non-ASCII title (e.g. an em-dash straddling byte 10)
    // no longer panics the way a raw `title[..PREFIX.len()]` byte-slice did.
    let Some(head) = title.get(..PREFIX.len()) else {
        return None;
    };
    if !head.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let rest = &title[PREFIX.len()..];
    let (pr_n, after) = rest.split_once(':')?;
    if pr_n.is_empty() || !pr_n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let combined = format!("PR-{} ({})", pr_n, display_id);
    let trimmed = after.trim_start().to_string();
    Some((combined, trimmed))
}

pub(crate) fn derive_parent_epic_label(
    req: &aida_core::Requirement,
    store: &RequirementsStore,
) -> Option<String> {
    req.relationships
        .iter()
        .filter(|r| r.rel_type == RelationshipType::Child)
        .find_map(|r| {
            let parent = store
                .requirements
                .iter()
                .find(|p| p.id == r.target_id && p.req_type == RequirementType::Epic)?;
            parent
                .agreed_id
                .as_deref()
                .or(parent.spec_id.as_deref())
                .map(String::from)
        })
}

/// Render the header line for a `--tree` group (`aida list --tree`).
///
/// A real parent-backed group (`parent` resolves to the requirement the label
/// groups on) carries the parent's own title + status badge at the parent
/// level, with the child count trailing as metadata — so a real scoped parent
/// reads as a requirement, not as a synthetic count-only bucket. The synthetic
/// `Unscoped` bucket and an unresolvable label both fall back to the plain
/// `<label> (N items)` form, keeping synthetic buckets distinguishable from
/// real parents.
// trace:TASK-0439 | ai:claude
pub(crate) fn tree_group_header(
    key: &str,
    unscoped_key: &str,
    count: usize,
    parent: Option<&aida_core::Requirement>,
    store: &aida_core::RequirementsStore,
) -> String {
    let plural = if count == 1 { "" } else { "s" };
    if key == unscoped_key {
        format!("{} ({} item{})", "Unscoped".cyan().bold(), count, plural)
    } else if let Some(parent) = parent {
        // BUG-658: an EPIC's status is a read-only rollup of its children
        // (BUG-626) — never the raw stored YAML status, which can drift in both
        // directions (a childless epic reading In Progress, an actively-shipping
        // epic reading Draft). The tree header used `parent.status` directly, so
        // the same epic could read one status as a group HEADER here and another
        // as a row / via `aida show` / `aida why`. Route through
        // `effective_display_status` — the same `derive_epic_status` rollup every
        // other surface uses — so the header agrees with the rest of the view.
        // trace:BUG-658 | ai:claude
        let effective = effective_display_status(store, parent);
        format!(
            "{}  {}  [{}]  ({} item{})",
            key.cyan().bold(),
            parent.title,
            // cache_key() ("InProgress") matches the child rows' status form
            // (summary.status), so both spell the same status identically in
            // one view rather than mixing "InProgress" / "In Progress".
            status_display::status_badge(effective.cache_key()),
            count,
            plural,
        )
    } else {
        format!("{} ({} item{})", key.cyan().bold(), count, plural)
    }
}

pub(crate) fn entry_scope_session_match(
    entry: &aida_core::QueueEntry,
    self_lease: Option<&SessionLease>,
    bypass: bool,
) -> bool {
    if bypass {
        return true;
    }
    // BUG-89: When the viewer has no active lease, do NOT apply the implicit
    // scope/session filter. The implicit "match my scope/session" filter
    // only makes sense WHEN you're in a session — without one, the user is
    // asking from the global vantage point (e.g., the project root), so all
    // routed items should be visible. STORY-57's filter is opt-in by being
    // inside a scoped session, not opt-out by sitting outside one. Users
    // who want strict scope filtering from outside can pass `--scope X`
    // explicitly. trace:BUG-89 | ai:claude
    let Some(lease) = self_lease else {
        return true;
    };
    if let Some(want_scope) = entry.for_scope.as_deref() {
        if !lease.scope.eq_ignore_ascii_case(want_scope) {
            return false;
        }
    }
    if let Some(want_sess) = entry.for_session.as_deref() {
        let n = want_sess.len().min(lease.id.len());
        if n < 4 {
            // Too short to be safely matched — bail out as no-match
            // rather than risk a false positive.
            return false;
        }
        if !lease.id[..n].eq_ignore_ascii_case(&want_sess[..n]) {
            return false;
        }
    }
    true
}

/// Lease-enforcement mode for cross-session writes.
/// Configured via `[session] enforcement = "warn"|"block"|"off"` in
/// `.aida/config.toml`; default is `Warn`.
/// trace:STORY-48 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionEnforcement {
    Off,
    Warn,
    Block,
}

impl SessionEnforcement {
    pub(crate) fn from_config_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "none" => SessionEnforcement::Off,
            "block" | "strict" => SessionEnforcement::Block,
            _ => SessionEnforcement::Warn,
        }
    }
}

/// Read `[session].enforcement` from `<project_root>/.aida/config.toml`.
/// Falls back to `Warn` on any parse/IO failure — enforcement should
/// never break the host command.
/// trace:STORY-48 | ai:claude
pub(crate) fn session_enforcement(project_root: &std::path::Path) -> SessionEnforcement {
    let path = project_root.join(".aida").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return SessionEnforcement::Warn;
    };
    let Ok(parsed) = content.parse::<toml::Value>() else {
        return SessionEnforcement::Warn;
    };
    parsed
        .get("session")
        .and_then(|s| s.get("enforcement"))
        .and_then(|v| v.as_str())
        .map(SessionEnforcement::from_config_str)
        .unwrap_or(SessionEnforcement::Warn)
}

/// Returns the lease that "owns" `target_uuid` — i.e. the spec is itself
/// the scope, OR is a descendant of the scope's spec via Parent
/// relationships. Excludes `self_lease` (the caller's own session) so a
/// session can freely edit specs in its own scope. Returns `None` for
/// path-glob / free-form scopes that we can't resolve to a spec id.
/// trace:STORY-48 | ai:claude
/// Build a minimal store containing the target plus its ancestor chain
/// (following `Child` edges — the climb-toward-root direction), resolved via
/// TARGETED per-uuid lookups rather than a full-store `backend.load()` (a scan
/// of every YAML object on the edit write path). `lease_owning_spec` only walks
/// `Child` edges from the target and reads the spec_id/agreed_id of the
/// reachable ancestors, so a store holding exactly that closure is behaviorally
/// identical to the full store for lease enforcement. The targeted
/// `get_requirement` is cache-backed; a missing ancestor is simply skipped (an
/// absent ancestor can't own a lease scope anyway).
// trace:BUG-634 | ai:claude
pub(crate) fn collect_ancestor_store(
    backend: &aida_core::CachedGitBackend,
    target: &Requirement,
) -> RequirementsStore {
    let mut visited: HashSet<Uuid> = HashSet::new();
    visited.insert(target.id);
    let mut requirements: Vec<Requirement> = vec![target.clone()];
    let mut frontier: Vec<Requirement> = vec![target.clone()];
    while let Some(curr) = frontier.pop() {
        for rel in &curr.relationships {
            if rel.rel_type == RelationshipType::Child && visited.insert(rel.target_id) {
                if let Ok(Some(parent)) = backend.get_requirement(&rel.target_id) {
                    requirements.push(parent.clone());
                    frontier.push(parent);
                }
            }
        }
    }
    RequirementsStore {
        requirements,
        ..Default::default()
    }
}

/// BUG-637: a closure that reports whether a session lease is currently LIVE,
/// reusing STORY-694's [`lease_state_for`] (pid liveness + worktree existence +
/// age, with the review-verb special-case). Built once over a pre-probed live
/// session set so the caller pays for the (relatively expensive) process probe a
/// single time and the per-lease check is cheap. A spec-scoped CLAIM only gates
/// work while its holder is live — a dead/stale claim is ignored by every gate
/// (no crash-deadlock).
// trace:BUG-637 | ai:claude
// BUG-1205: what to do when a PR's source branch is held by another lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeaseConflictDecision {
    /// A human is at the keyboard — offer the four options.
    Prompt,
    /// Headless and the lease's session is gone — end it and continue.
    AutoEnd,
    /// Headless and the lease is LIVE — refuse with a typed message.
    Refuse,
}

// BUG-1205: pure decision for the lease-held pre-flight in `session_start`.
// trace:BUG-1205 | ai:claude
pub(crate) fn lease_conflict_decision(headless: bool, lease_live: bool) -> LeaseConflictDecision {
    match (headless, lease_live) {
        (false, _) => LeaseConflictDecision::Prompt,
        (true, false) => LeaseConflictDecision::AutoEnd,
        (true, true) => LeaseConflictDecision::Refuse,
    }
}

pub(crate) fn lease_is_live<'a>(
    live_sessions: &'a [process_probe::LiveSession],
    now: chrono::DateTime<chrono::Utc>,
) -> impl Fn(&SessionLease) -> bool + 'a {
    move |l: &SessionLease| matches!(lease_state_for(l, live_sessions, now), LeaseState::Live)
}

/// BUG-637: the pre-pickup claim gate. Returns the LIVE spec-scoped lease held by
/// a DIFFERENT session for any of `spec_ids`, if one exists — the signal that
/// another agent is already working this spec and a fresh pickup would duplicate
/// it (the BUG-634 incident). Pure over the lease set + an injected liveness
/// predicate so it is unit-testable without a live process or a lease dir:
///
/// - skips the caller's own lease (`self_lease`) — resuming your own work is fine;
/// - skips non-live (crashed / stale / dormant) claims — no crash-deadlock;
/// - matches a lease whose raw `--owns` scope equals one of `spec_ids`
///   (case-insensitive), i.e. the AIDA-launched spec-scoped happy path. Generic
///   `harness-worktree` advisor fan-out scopes never match, so they don't gate
///   (making THOSE spec-claimable is the documented follow-on).
// trace:BUG-637 | ai:claude
pub(crate) fn live_spec_claim_by_other<'a>(
    leases: &'a [SessionLease],
    self_lease: Option<&SessionLease>,
    spec_ids: &[&str],
    is_live: impl Fn(&SessionLease) -> bool,
) -> Option<&'a SessionLease> {
    leases.iter().find(|l| {
        if let Some(self_l) = self_lease {
            if l.id == self_l.id {
                return false;
            }
        }
        if !is_live(l) {
            return false;
        }
        spec_ids
            .iter()
            .any(|id| !id.is_empty() && l.scope.eq_ignore_ascii_case(id))
    })
}

/// What the requeue lease gate found for a spec.
// trace:STORY-1429 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequeueLeaseCheck {
    /// No other session holds a live lease on the spec.
    Clear,
    /// Another session holds a live lease: requeueing would reset work a
    /// running session owns.
    HeldByOther {
        session: String,
        started: String,
        worktree: String,
    },
    /// The lease state could not be established. Missing evidence is not good
    /// evidence, so this refuses exactly like a live lease.
    Unknown(String),
}

impl RequeueLeaseCheck {
    /// The refusal text for `id`, or `None` when the gate is clear. It names
    /// the holder and the command that releases it. `--force` does not
    /// override it.
    pub(crate) fn refusal(&self, id: &str) -> Option<String> {
        match self {
            RequeueLeaseCheck::Clear => None,
            RequeueLeaseCheck::HeldByOther {
                session,
                started,
                worktree,
            } => Some(format!(
                "{id} is claimed by live session {session} (started {started}, worktree \
                 {worktree}); requeueing it would reset work that session owns. If that \
                 session is abandoned, end it with `aida session end {session}` and requeue \
                 again. --force does not override a live claim."
            )),
            RequeueLeaseCheck::Unknown(why) => Some(format!(
                "cannot tell whether another session is working on {id} ({why}); refusing to \
                 requeue it. Fix the session leases (`aida session leases`) and requeue again."
            )),
        }
    }
}

/// Whether `name` is a lease file the requeue gate reads: a session lease
/// `<id>.toml` (no dot in the stem, the same rule `lease_ids_in` (BUG-114)
/// uses), or an MCP `claim_task` claim `mcp-claim.<spec>.toml`. That excludes
/// the `<id>.activity.toml` / `<id>.manifest.toml` companions and the
/// `write_atomic` staging files. An MCP claim parses as a `SessionLease` and
/// the BUG-637 pickup gate evaluates it, so the requeue gate does too: a claim
/// with a real worktree and a live session is Live and refuses, while a dead or
/// worktree-less claim comes out Stale through the same liveness check.
// trace:STORY-1429 | ai:claude
pub(crate) fn is_session_lease_file(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".toml") else {
        return false;
    };
    let stem = stem.strip_prefix("mcp-claim.").unwrap_or(stem);
    !stem.is_empty() && !stem.contains('.')
}

/// What one lease file told the requeue gate.
// trace:STORY-1429 | ai:claude
#[derive(Debug)]
pub(crate) enum LeaseFileRead {
    Lease(Box<SessionLease>),
    /// Unparseable, but it visibly names a different scope: cannot be a
    /// claim on the spec being requeued.
    OtherScope,
    /// Unreadable or unparseable, and it could name the spec.
    Unknown(String),
}

/// Read one lease file for the requeue gate. A read or parse failure is
/// retried once (a writer may be mid-write); a file still unparseable after
/// the retry is `Unknown` only when it could name one of `spec_ids`: its text
/// mentions one, or no `scope` is visible in it at all.
// trace:STORY-1429 | ai:claude
pub(crate) fn read_lease_file_for_gate(
    label: &str,
    read: impl Fn() -> std::io::Result<String>,
    spec_ids: &[&str],
) -> LeaseFileRead {
    let attempt = || -> Result<SessionLease, (Option<String>, String)> {
        let content =
            read().map_err(|e| (None, format!("the lease {label} is unreadable: {e}")))?;
        toml::from_str::<SessionLease>(&content).map_err(|e| {
            (
                Some(content),
                format!("the lease {label} does not parse: {e}"),
            )
        })
    };
    let (content, why) = match attempt() {
        Ok(lease) => return LeaseFileRead::Lease(Box::new(lease)),
        Err(_) => {
            std::thread::sleep(std::time::Duration::from_millis(50));
            match attempt() {
                Ok(lease) => return LeaseFileRead::Lease(Box::new(lease)),
                Err(e) => e,
            }
        }
    };
    let Some(content) = content else {
        return LeaseFileRead::Unknown(why);
    };
    let lower = content.to_ascii_lowercase();
    let mentions = spec_ids
        .iter()
        .any(|id| !id.is_empty() && lower.contains(&id.to_ascii_lowercase()));
    if mentions {
        return LeaseFileRead::Unknown(why);
    }
    // Only a COMPLETE quoted scope counts: a value cut off mid-write could
    // still be the spec's id.
    let names_a_scope = content.lines().any(|l| {
        let l = l.trim_start();
        l.strip_prefix("scope")
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('='))
            .map(str::trim)
            .is_some_and(|v| v.len() > 2 && v.starts_with('"') && v.ends_with('"'))
    });
    if names_a_scope {
        LeaseFileRead::OtherScope
    } else {
        LeaseFileRead::Unknown(why)
    }
}

/// Lease listing for the requeue gate. Reads only real lease files (see
/// [`is_session_lease_file`]). An unreadable lease directory is an error; a
/// missing one is a definite "no leases". A lease that stays unparseable after
/// one retry is an error only when it could name one of `spec_ids`.
// trace:STORY-1429 | ai:claude
pub(crate) fn list_leases_strict(
    project_root: &std::path::Path,
    spec_ids: &[&str],
) -> Result<Vec<SessionLease>, String> {
    let dir = leases_dir(project_root);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&dir)
        .map_err(|e| format!("the lease directory {} is unreadable: {e}", dir.display()))?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("reading {}: {e}", dir.display()))?;
        let p = entry.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_session_lease_file(name) {
            continue;
        }
        let label = p.display().to_string();
        match read_lease_file_for_gate(&label, || std::fs::read_to_string(&p), spec_ids) {
            LeaseFileRead::Lease(lease) => out.push(*lease),
            LeaseFileRead::OtherScope => {}
            LeaseFileRead::Unknown(why) => {
                // A lease removed between the listing and the read is gone,
                // not unknown.
                if !p.exists() {
                    continue;
                }
                return Err(why);
            }
        }
    }
    out.sort_by_key(|l| l.started_at);
    Ok(out)
}

/// The requeue lease gate, pure over a lease listing and a FALLIBLE liveness
/// probe. The BUG-637 predicate cannot say "unknown", so the fail-closed
/// handling lives here: a listing error, or a probe error on a lease that
/// claims one of `spec_ids`, is [`RequeueLeaseCheck::Unknown`]. Dead or stale
/// claims do not block (pickup reclaims them). The caller's own lease never
/// blocks. Only leases in this clone are visible.
// trace:STORY-1429 | ai:claude
pub(crate) fn requeue_lease_gate_from(
    listed: Result<Vec<SessionLease>, String>,
    self_lease: Option<&SessionLease>,
    spec_ids: &[&str],
    is_live: impl Fn(&SessionLease) -> Result<bool, String>,
) -> RequeueLeaseCheck {
    let leases = match listed {
        Ok(leases) => leases,
        Err(why) => return RequeueLeaseCheck::Unknown(why),
    };
    for l in &leases {
        if self_lease.is_some_and(|s| s.id == l.id) {
            continue;
        }
        if !spec_ids
            .iter()
            .any(|id| !id.is_empty() && l.scope.eq_ignore_ascii_case(id))
        {
            continue;
        }
        match is_live(l) {
            Ok(true) => {
                return RequeueLeaseCheck::HeldByOther {
                    session: l.id.chars().take(8).collect(),
                    started: l
                        .started_at
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string(),
                    worktree: l.worktree_path.display().to_string(),
                }
            }
            Ok(false) => {}
            Err(why) => {
                return RequeueLeaseCheck::Unknown(format!(
                    "the liveness of session {} could not be checked: {why}",
                    l.id.chars().take(8).collect::<String>()
                ))
            }
        }
    }
    RequeueLeaseCheck::Clear
}

/// Production liveness for the requeue gate. On Linux the process table is
/// the evidence; when `/proc` cannot be read, liveness is unknown rather than
/// "dead".
// trace:STORY-1429 | ai:claude
pub(crate) fn requeue_lease_liveness(
    l: &SessionLease,
    live_sessions: &std::cell::OnceCell<Vec<process_probe::LiveSession>>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    if std::fs::read_dir("/proc").is_err() {
        return Err("the process table (/proc) is not readable".to_string());
    }
    let live = live_sessions.get_or_init(process_probe::probe_live_claude_sessions);
    Ok(matches!(lease_state_for(l, live, now), LeaseState::Live))
}

/// Run the requeue lease gate for `spec_ids` against the leases under
/// `project_root`.
// trace:STORY-1429 | ai:claude
pub(crate) fn requeue_lease_gate(
    project_root: &std::path::Path,
    spec_ids: &[&str],
) -> RequeueLeaseCheck {
    let listed = list_leases_strict(project_root, spec_ids);
    let self_lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(project_root, &cwd));
    let now = chrono::Utc::now();
    let live = std::cell::OnceCell::new();
    requeue_lease_gate_from(listed, self_lease.as_ref(), spec_ids, |l| {
        requeue_lease_liveness(l, &live, now)
    })
}

pub(crate) fn lease_owning_spec(
    leases: &[SessionLease],
    self_lease: Option<&SessionLease>,
    target_uuid: Uuid,
    target_spec_id: Option<&str>,
    store: &RequirementsStore,
    // BUG-637: liveness predicate. A spec-scoped claim only blocks an outbound
    // mutation while its holder is genuinely LIVE — a crashed/stale agent's
    // lease must NOT permanently lock its spec (no crash-deadlock). Callers
    // pass `lease_is_live(&live_sessions, now)`; the unit tests (which exercise
    // ancestry/scope matching, not liveness) pass `|_| true`.
    // trace:BUG-637 | ai:claude
    is_live: impl Fn(&SessionLease) -> bool,
) -> Option<SessionLease> {
    if leases.is_empty() {
        return None;
    }

    // Walk to collect ancestors (incl. target itself). AIDA stores
    // hierarchy with `rel_type: Child` on the descendant pointing at the
    // ancestor (display: "X is child of Y"), so the climb-toward-root edge
    // is `Child`, NOT `Parent`. Visit each uuid at most once; pathological
    // cycles are bounded by the visited set.
    let mut ancestors: HashSet<Uuid> = HashSet::new();
    ancestors.insert(target_uuid);
    let mut frontier = vec![target_uuid];
    while let Some(curr) = frontier.pop() {
        if let Some(req) = store.requirements.iter().find(|r| r.id == curr) {
            for rel in &req.relationships {
                if rel.rel_type == RelationshipType::Child && ancestors.insert(rel.target_id) {
                    frontier.push(rel.target_id);
                }
            }
        }
    }

    // Pre-compute the lower-cased spec_ids / agreed_ids of the ancestor
    // set for fast string-match against scope strings.
    let ancestor_ids: HashSet<String> = ancestors
        .iter()
        .filter_map(|id| store.requirements.iter().find(|r| r.id == *id))
        .flat_map(|r| {
            [r.spec_id.as_deref(), r.agreed_id.as_deref()]
                .into_iter()
                .flatten()
                .map(|s| s.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .collect();

    for lease in leases {
        if let Some(self_l) = self_lease {
            if lease.id == self_l.id {
                continue;
            }
        }
        // BUG-637: a dead/stale claim doesn't own the scope anymore — skip it so a
        // crashed agent can't permanently block edits to its spec.
        // trace:BUG-637 | ai:claude
        if !is_live(lease) {
            continue;
        }
        let scope_lc = lease.scope.to_ascii_lowercase();
        // Direct id-form match (handles SPEC-ID + agreed-id forms).
        if ancestor_ids.contains(&scope_lc) {
            return Some(lease.clone());
        }
        // The provided target_spec_id might equal the scope verbatim even
        // when ancestor walking doesn't pick it up (e.g. agreed_id stripped).
        if let Some(tid) = target_spec_id {
            if tid.eq_ignore_ascii_case(&lease.scope) {
                return Some(lease.clone());
            }
        }
        // Path globs / free-form tags: no ancestry to walk → don't enforce.
    }
    None
}

/// Enforce a session lease for an outbound mutation on `target`. Returns
/// `Err` only when the active enforcement mode is `Block` and another
/// session owns the scope; in `Warn` mode it prints a warning and returns
/// `Ok(())` so the operation proceeds. `force_block` (e.g. `--strict` on
/// `aida edit`) escalates Warn → Block for this single call.
///
/// BUG-637: `force_skip` (e.g. `--force` on `aida edit`) is the operator
/// override — it short-circuits the whole gate so a deliberate
/// edit/reject of a live-claimed spec is always possible. The claim check is
/// now LIVENESS-aware ([`lease_is_live`]): a dead/stale claim no longer owns the
/// scope, so a crashed agent can't permanently block edits to its spec (the
/// reject-while-working fix without the crash-deadlock).
// trace:STORY-48 trace:BUG-637 | ai:claude
pub(crate) fn enforce_session_lease(
    project_root: &std::path::Path,
    target: &Requirement,
    store: &RequirementsStore,
    operation: &str,
    force_block: bool,
    force_skip: bool,
) -> Result<()> {
    // BUG-637: operator override — `--force` always wins, no probe needed.
    if force_skip {
        return Ok(());
    }
    let leases = list_leases(project_root);
    if leases.is_empty() {
        return Ok(());
    }
    let self_lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(project_root, &cwd));
    // BUG-637: probe live sessions once, then gate the claim on liveness.
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    let owner = lease_owning_spec(
        &leases,
        self_lease.as_ref(),
        target.id,
        target.spec_id.as_deref(),
        store,
        lease_is_live(&live, now),
    );
    let Some(owner) = owner else { return Ok(()) };

    let mut mode = session_enforcement(project_root);
    if force_block {
        mode = SessionEnforcement::Block;
    }
    let target_label = target.spec_id.as_deref().unwrap_or("?");
    let owner_id_short: String = owner.id.chars().take(8).collect();
    match mode {
        SessionEnforcement::Off => Ok(()),
        SessionEnforcement::Warn => {
            eprintln!(
                "{} {} on {} touches scope owned by session {} ({})",
                "Warning:".yellow().bold(),
                operation,
                target_label.cyan(),
                owner_id_short.yellow(),
                owner.scope.cyan(),
            );
            eprintln!("  worktree: {}", owner.worktree_path.display());
            eprintln!(
                "  ({} or set `[session] enforcement = \"off\"` in .aida/config.toml to silence)",
                "pass --strict to convert into a hard block".dimmed()
            );
            Ok(())
        }
        SessionEnforcement::Block => {
            anyhow::bail!(
                "{} on {} blocked: scope owned by session {} ({}, worktree: {}). \
                 End that session first or set `[session] enforcement = \"warn\"` to downgrade.",
                operation,
                target_label,
                owner_id_short,
                owner.scope,
                owner.worktree_path.display()
            );
        }
    }
}

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

// ─── TASK-965: worktree-tangle gate (substrate-as-bouncer) ─────────────────
//
// AIDA's fan-out incident: a bare agent `git checkout -b`s in the MAIN repo,
// leaving the primary checkout parked on a feature branch, so the next
// `git merge --ff-only origin/main` aborts. We used to defend this with a memory
// RULE only; the substrate-as-bouncer principle says ship a GATE. Two pure
// predicates back the two guards — a spawn-time assertion (`aida agent new`) and
// a stranded-primary alarm (`aida status` / `aida ps`). Kept side-effect-free so
// both are unit-testable without git/leases.

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

/// STORY-106: best-effort emit the "queue empty for this role" workflow
/// hint after a successful `queue done` (or `edit --status completed` on
/// a queue-tracked spec). Detects role+scope from the session env / lease,
/// counts remaining queue entries, and fires the hint only when the
/// active-role queue is now empty. Silent on any detection failure —
/// hints are nice-to-have, not load-bearing.
/// trace:STORY-106 | ai:claude
pub(crate) fn maybe_hint_after_queue_drain(storage: &Storage, user_id: &str) {
    let project_root = match storage.path().parent() {
        Some(p) => p.to_path_buf(),
        None => return,
    };
    let role = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.is_empty());

    // Match the filter `aida queue list` uses for the active session:
    //   1. role: matches AIDA_SESSION_ROLE
    //   2. scope: matches the active session lease's scope (unscoped
    //      entries pass through, same as queue list inside a session)
    //   3. the entry's requirement still exists in the store — dangling
    //      entries pointing to deleted/renamed UUIDs are noise and `aida
    //      queue list` hides them. Without #3 the hint sees stale queue
    //      cruft and never fires.
    // trace:STORY-106 | ai:claude
    let scope = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(&project_root, &cwd))
        .map(|l| l.scope);

    let remaining = storage.queue_list(user_id, false).unwrap_or_default();
    let store_snapshot = storage.load().ok();
    // Entry counts as "open work for this user" only when its
    // requirement (a) still exists in the store and (b) hasn't already
    // reached terminal status. The second check mirrors `aida queue
    // list`'s terminal-hide behavior (TASK-46) — without it, queue
    // entries left behind after `aida edit --status completed` (which
    // doesn't auto-dequeue) read as "still in flight" and suppress the
    // hint forever.
    let entry_open = |e: &aida_core::QueueEntry| -> bool {
        let Some(snap) = &store_snapshot else {
            return true;
        };
        let Some(req) = snap.requirements.iter().find(|r| r.id == e.requirement_id) else {
            return false;
        };
        !is_terminal_status(&req.status)
    };
    let filtered: Vec<&aida_core::QueueEntry> = remaining
        .iter()
        .filter(|e| entry_open(e))
        .filter(|e| match role.as_deref() {
            Some(r) => e.for_role.as_deref() == Some(r),
            None => true,
        })
        .filter(|e| match scope.as_deref() {
            Some(s) => match e.for_scope.as_deref() {
                Some(es) => es == s,
                None => true,
            },
            None => true,
        })
        .collect();
    if !filtered.is_empty() {
        return;
    }

    let branch = current_branch_at(&project_root);
    let commits_ahead = branch
        .as_deref()
        .and_then(|b| branch_commits_ahead_main(&project_root, b));

    // BUG-232: when the branch carries committed-but-unshipped work, probe
    // whether a PR is already open. This lets the hint sharpen "open a PR"
    // into a pointed "no PR open — run `/aida-pr`" warning — the failure
    // mode where a `--zen` session ends with the spec Done but unmergeable.
    // Probe only when there's something to ship; a `gh` miss / failure
    // stays Unknown so the hint never *asserts* there's no PR on guesswork.
    // trace:BUG-232 | ai:claude
    let pr_state = match (branch.as_deref(), commits_ahead) {
        // STORY-516: forge-routed. trace:STORY-516 | ai:claude
        (Some(b), Some(n)) if n > 0 => match change_lookup_for_branch(&project_root, b) {
            crate::forge::ChangeLookup::Found(c) => workflow_hints::PrState::Open(c.id),
            crate::forge::ChangeLookup::NoChange => workflow_hints::PrState::Absent,
            crate::forge::ChangeLookup::CliMissing
            | crate::forge::ChangeLookup::CliFailed(_)
            | crate::forge::ChangeLookup::Unreachable(_) => workflow_hints::PrState::Unknown,
        },
        _ => workflow_hints::PrState::Unknown,
    };

    workflow_hints::after_queue_drained(
        Some(&project_root),
        role.as_deref(),
        scope.as_deref(),
        commits_ahead,
        branch.as_deref(),
        pr_state,
    );
}

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
