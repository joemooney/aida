//! requirement add/list/show/edit/delete handlers, status/priority/type parsing and `aida grep` (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// True for a scaffold artifact that must be executable: a git or Claude
/// Code hook, or a shell script.
// trace:BUG-21 trace:BUG-2009 | ai:claude
pub(crate) fn is_executable_artifact(rel_path: &std::path::Path) -> bool {
    let s = rel_path.to_string_lossy();
    s.starts_with(".git/hooks/") || s.starts_with(".claude/hooks/") || s.ends_with(".sh")
}

/// Ensure git/claude hook files are executable. Called from scaffolder
/// write paths so freshly-scaffolded hooks don't trigger git's "hook was
/// ignored because not executable" warning. Sets 0755 explicitly (never
/// the mode an overwritten file had, nor what the umask leaves for a new
/// one), and a failure is returned: a hook git skips is silent.
// trace:BUG-21 trace:BUG-2009 | ai:claude
pub(crate) fn ensure_executable_if_hook(
    rel_path: &std::path::Path,
    full_path: &std::path::Path,
) -> std::io::Result<()> {
    if !is_executable_artifact(rel_path) {
        return Ok(());
    }
    aida_core::git_ops::set_hook_mode(full_path)
}

/// True when the scaffold artifact at `full_path` must be executable and is
/// not: its content may match, but git skips such a hook.
// trace:BUG-2009 | ai:claude
pub(crate) fn hook_artifact_needs_mode(
    rel_path: &std::path::Path,
    full_path: &std::path::Path,
) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        is_executable_artifact(rel_path)
            && std::fs::metadata(full_path).is_ok_and(|m| m.permissions().mode() & 0o111 == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = (rel_path, full_path);
        false
    }
}

/// Resolve the description from one of three sources: inline `--description`,
/// `--description-from-file PATH`, or `--description-stdin`. The CLI struct
/// already enforces mutual exclusion via `conflicts_with_all`; here we just
/// fetch the content from the right source. Returns `Ok(None)` when no
/// source is set (caller falls back to empty / interactive prompt).
/// trace:BUG-17 | ai:claude
pub(crate) fn resolve_description(
    description: &Option<String>,
    description_from_file: &Option<std::path::PathBuf>,
    description_stdin: bool,
) -> Result<Option<String>> {
    if let Some(d) = description {
        return Ok(Some(d.clone()));
    }
    if let Some(path) = description_from_file {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read description from {}", path.display()))?;
        return Ok(Some(body));
    }
    if description_stdin {
        use std::io::Read;
        let mut body = String::new();
        std::io::stdin()
            .read_to_string(&mut body)
            .context("failed to read description from stdin")?;
        return Ok(Some(body));
    }
    Ok(None)
}

// trace:BUG-1234 | ai:codex
/// Resolve an edit description and refuse an empty file/stdin body so a
/// failed producer cannot erase an existing requirement description.
/// Inline `--description ""` remains the explicit way to clear the field.
pub(crate) fn resolve_edit_description(
    description: &Option<String>,
    description_from_file: &Option<std::path::PathBuf>,
    description_stdin: bool,
) -> Result<Option<String>> {
    let resolved = resolve_description(description, description_from_file, description_stdin)?;
    if (description_from_file.is_some() || description_stdin)
        && resolved.as_deref().is_some_and(str::is_empty)
    {
        anyhow::bail!("description input is empty; existing description left unchanged");
    }
    Ok(resolved)
}

pub(crate) fn parse_requirement_type(s: &str) -> Result<RequirementType> {
    match s.to_lowercase().as_str() {
        "functional" | "fr" => Ok(RequirementType::Functional),
        "non-functional" | "nonfunctional" | "nfr" => Ok(RequirementType::NonFunctional),
        "system" | "sr" => Ok(RequirementType::System),
        "user" | "ur" => Ok(RequirementType::User),
        // Workflow type: a proposed change. trace:TASK-716 | ai:claude
        "change-request" | "changerequest" | "change" | "cr" => Ok(RequirementType::ChangeRequest),
        "bug" => Ok(RequirementType::Bug),
        "epic" => Ok(RequirementType::Epic),
        "story" => Ok(RequirementType::Story),
        "task" => Ok(RequirementType::Task),
        "spike" => Ok(RequirementType::Spike),
        "sprint" => Ok(RequirementType::Sprint),
        "folder" => Ok(RequirementType::Folder),
        "meta" => Ok(RequirementType::Meta),
        // Docs-layer types (FR-1-074). Aliases match the type prefix used
        // in agreed-id format (`PRIN`, `VIS`, `CON`, `ADR`, `TERM`).
        // trace:FR-1-074 | ai:claude
        "principle" | "prin" => Ok(RequirementType::Principle),
        "vision" | "vis" => Ok(RequirementType::Vision),
        "constraint" | "con" => Ok(RequirementType::Constraint),
        "decision" | "adr" => Ok(RequirementType::Decision),
        "term" | "glossary" => Ok(RequirementType::Term),
        // trace:STORY-104 | ai:claude
        "doc" | "documentation" => Ok(RequirementType::Doc),
        "faq" => Ok(RequirementType::Faq),
        _ => anyhow::bail!("Unknown requirement type: {}", s),
    }
}

#[cfg(test)]
#[path = "tests/parse_requirement_type_tests.rs"]
mod parse_requirement_type_tests;

/// Initialize distributed mode using an orphan branch + worktree.
/// This is the default for single-repo projects.
/// Store lives at .aida-store/ (worktree of orphan branch 'aida-store').
/// BUG-446 / TASK-686: immediate-child directories of `cwd` that are their OWN
/// project — a nested git repo (a `.git` directory, or a `.git` file =
/// gitlink/submodule worktree) OR an AIDA project (a child `.aida/` dir) — and
/// are NOT registered submodules. A non-empty result means `cwd` is a
/// workspace-/parent-of-projects, not a single project.
///
/// We deliberately test for a child's own `.git`/`.aida` entry rather than
/// calling `git_ops::is_git_repo` (which shells out to `git rev-parse`): from
/// inside `cwd` — itself a git repo by the time this runs — `rev-parse`
/// resolves to `cwd`'s git-dir for EVERY subdirectory, so it would flag plain
/// subdirs too. Dotted children (`.git`, `.aida-store`, …) and
/// `.gitmodules`-declared submodules are intentional and excluded.
///
/// TASK-686 added the child-`.aida/` arm: a parent of AIDA projects that aren't
/// all plain git repos (the `~/ai/` case — ~80 children) would otherwise slip
/// the guard and leave a scaffold every child inherits via ancestor CLAUDE.md.
/// trace:BUG-446 trace:TASK-686 | ai:claude
pub(crate) fn unmanaged_nested_projects(cwd: &std::path::Path) -> Vec<String> {
    let submodule_paths = gitmodule_child_paths(cwd);
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(cwd) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || submodule_paths.contains(&name) {
            continue;
        }
        if path.join(".git").exists() || path.join(".aida").is_dir() {
            found.push(name);
        }
    }
    found.sort();
    found
}

/// First path component of each `path = …` entry in the top-level `.gitmodules`
/// — enough to exclude an immediate-child submodule directory from the
/// workspace-of-projects guard. trace:BUG-446 | ai:claude
pub(crate) fn gitmodule_child_paths(cwd: &std::path::Path) -> std::collections::HashSet<String> {
    let mut paths = std::collections::HashSet::new();
    let Ok(content) = std::fs::read_to_string(cwd.join(".gitmodules")) else {
        return paths;
    };
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("path") {
            if let Some(value) = rest.split('=').nth(1) {
                if let Some(first) = value.trim().split('/').next() {
                    if !first.is_empty() {
                        paths.insert(first.to_string());
                    }
                }
            }
        }
    }
    paths
}

/// What to do when `aida init` runs in a directory that isn't a git repo yet.
/// trace:STORY-552 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GitInitDecision {
    /// Run `git init` without asking (explicit `--git-init`).
    Yes,
    /// Offer interactively (TTY, no flag).
    Prompt,
    /// Keep the safe bail+recipe (non-interactive, no flag — don't
    /// silently git-init in scripts).
    Bail,
}

/// Decide how to handle a non-git folder at the front of init. `--git-init`
/// always wins; otherwise prompt at a TTY and bail elsewhere. Pure so it can be
/// unit-tested without a terminal. trace:STORY-552 | ai:claude
pub(crate) fn git_init_decision(git_init_flag: bool, at_tty: bool) -> GitInitDecision {
    if git_init_flag {
        GitInitDecision::Yes
    } else if at_tty {
        GitInitDecision::Prompt
    } else {
        GitInitDecision::Bail
    }
}

/// Get a git config value from the global config.
pub(crate) fn _git_config_get_global(key: &str) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["config", "--global", key])
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        anyhow::bail!("git config {} not set", key)
    }
}

pub(crate) fn add_requirement_interactive(storage: &Storage) -> Result<()> {
    // Load existing requirements
    let mut store = storage.load()?;

    // Prompt user for requirement details
    let requirement = crate::prompts::prompt_new_requirement(&mut store)?;
    let id = requirement.id;

    // Get prefixes for ID generation
    let feature_prefix = store
        .get_feature_by_name(&requirement.feature)
        .map(|f| f.prefix.clone());
    let type_prefix = store.get_type_prefix(&requirement.req_type);

    // Add the requirement with auto-assigned ID based on configuration
    store.add_requirement_with_id(
        requirement,
        feature_prefix.as_deref(),
        type_prefix.as_deref(),
    );
    storage.save(&store)?;

    // Get the added requirement to show its ID
    let added_req = store
        .get_requirement_by_id(&id)
        .expect("Just added requirement");

    println!("{}", "Requirement added successfully!".green());
    println!("UUID: {}", id);
    if let Some(spec_id) = &added_req.spec_id {
        println!("ID: {}", spec_id.green());
    }

    Ok(())
}

// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_requirement_cli(
    storage: &Storage,
    title: &Option<String>,
    description: &Option<String>,
    status_str: &Option<String>,
    priority_str: &Option<String>,
    type_str: &Option<String>,
    owner: &Option<String>,
    feature: &Option<String>,
    tags_str: &Option<String>,
    prefix: &Option<String>,
    parent: &Option<String>,
    force_parent: bool,
) -> Result<()> {
    // Load existing requirements
    let mut store = storage.load()?;

    // Check required fields
    let title = match title {
        Some(t) => t.clone(),
        None => anyhow::bail!("Title is required. Use --title to specify a title."),
    };

    let description = match description {
        Some(d) => d.clone(),
        None => String::new(),
    };

    // Validate parent exists if specified
    let parent_uuid = if let Some(parent_id) = parent {
        let uuid = parse_requirement_id(parent_id, &store)?;
        // BUG-64: terminal-status guard. Refuse to file a new child
        // under a Completed/Rejected parent unless --force-parent.
        // trace:BUG-64 | ai:claude
        if !force_parent {
            let pr = store
                .get_requirement_by_id(&uuid)
                .ok_or_else(|| anyhow::anyhow!("parent {} not found", parent_id))?;
            if is_terminal_status(&pr.status) {
                anyhow::bail!(
                    "parent {} is {} — adding new children to a closed parent is usually a mistake. \
                     Pass `--force-parent` to override.",
                    pr.spec_id.as_deref().unwrap_or(parent_id),
                    pr.status,
                );
            }
        }
        Some(uuid)
    } else {
        None
    };

    // Create a requirement with basic data
    let mut requirement = Requirement::new(title, description);

    // Set optional fields
    if let Some(status) = status_str {
        requirement.status = parse_status(status)?;
    }

    // TASK-647 (ADR-3): advisor-gate the production of approved+ specs. A
    // non-advisor, non-TTY caller (headless agent, drain/auto capture) can
    // only file `draft`; a requested approved+ status is downgraded with a
    // one-line triage notice. Advisor role or an interactive human is
    // unaffected. trace:TASK-647 | ai:claude
    let intake_downgraded_from =
        if status_requires_advisor_authority(&requirement.status) && !has_advisor_authority() {
            let from = requirement.status;
            requirement.status = RequirementStatus::Draft;
            Some(from)
        } else {
            // BUG-498: an approved+ status that was NOT downgraded means the
            // caller exercised advisor authority — if that authority came from
            // an `AIDA_SESSION_ROLE=advisor` prefix rather than a seated role,
            // nudge them to seat it. trace:BUG-498 | ai:claude
            if status_requires_advisor_authority(&requirement.status) {
                maybe_hint_advisor_seat();
            }
            None
        };

    if let Some(priority) = priority_str {
        requirement.priority = parse_priority(priority)?;
    }

    if let Some(req_type) = type_str {
        requirement.req_type = parse_type(req_type)?;
    }

    // Set owner: use explicit value, AIDA_AUTHOR env var, or system username
    requirement.owner = owner.clone().unwrap_or_else(get_default_author);

    if let Some(feature_val) = feature {
        requirement.feature = feature_val.clone();
    }

    if let Some(tags) = tags_str {
        let tag_set: HashSet<String> = tags
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        requirement.tags = tag_set;
    }

    // Set prefix override if specified
    if let Some(prefix_val) = prefix {
        requirement
            .set_prefix_override(prefix_val)
            .map_err(|e| anyhow::anyhow!(e))?;
    }

    let id = requirement.id;

    // Get prefixes for ID generation
    let feature_prefix = store
        .get_feature_by_name(&requirement.feature)
        .map(|f| f.prefix.clone());
    let type_prefix = store.get_type_prefix(&requirement.req_type);

    // Add the requirement with auto-assigned ID based on configuration
    store.add_requirement_with_id(
        requirement,
        feature_prefix.as_deref(),
        type_prefix.as_deref(),
    );

    // Add parent relationship if specified.
    //
    // BUG-58: previously stored `(child, Parent, parent)` with
    // bidirectional=false, which is doubly broken:
    //   1. Relationship type is "I am X to target", so Parent on the
    //      child says "I AM the parent of <parent>" — backwards.
    //   2. Without bidirectional=true, the parent never gets the inverse
    //      Parent edge pointing at the child, so `rel list <parent>`
    //      didn't show its new child.
    // Fix: store `Child` on the source (child) pointing at the parent,
    // bidirectional so the parent gets the matching `Parent` edge.
    // Matches the convention already used by the git-canonical add
    // path (FR-215). trace:BUG-58 | ai:claude
    if let Some(parent_id) = parent_uuid {
        store
            .add_relationship(&id, RelationshipType::Child, &parent_id, true)
            .map_err(|e| anyhow::anyhow!("Failed to add parent relationship: {}", e))?;
    }

    storage.save(&store)?;

    // Get the added requirement to show its ID
    let added_req = store
        .get_requirement_by_id(&id)
        .expect("Just added requirement");

    println!("{}", "Requirement added successfully!".green());
    println!("UUID: {}", id);
    if let Some(spec_id) = &added_req.spec_id {
        println!("ID: {}", spec_id.green());
    }

    // TASK-647 (ADR-3): tell the caller their requested status was held for
    // triage. Quiet and non-fatal — the spec is filed, just as draft.
    if let Some(from) = intake_downgraded_from {
        eprintln!(
            "{} filed as {} (requested {} needs advisor authority) — queued for advisor triage.",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            "draft".yellow(),
            from.to_string().to_lowercase().dimmed()
        );
    }

    // Show parent relationship if created
    if let Some(parent_id_str) = parent {
        println!("Parent: {}", parent_id_str.cyan());
    }

    Ok(())
}

/// TASK-527: match one `--tags` filter token against a spec's tag set, with
/// prefix-glob support. A trailing `*` (`aida:queue:*`) matches any tag starting
/// with the literal prefix, plus the bare prefix without its trailing `:` (so
/// `aida:queue:*` also matches an exact `aida:queue` tag). Without `*` it's the
/// existing exact membership test. Comma-OR composition happens at the caller.
/// trace:TASK-527 | ai:claude
pub(crate) fn tag_filter_matches(filter: &str, tags: &std::collections::HashSet<String>) -> bool {
    if let Some(prefix) = filter.strip_suffix('*') {
        let bare = prefix.strip_suffix(':').unwrap_or(prefix);
        tags.iter().any(|t| t.starts_with(prefix) || t == bare)
    } else {
        tags.contains(filter)
    }
}

#[cfg(test)]
#[path = "tests/task_527_tag_glob_tests.rs"]
mod task_527_tag_glob_tests;

pub(crate) fn list_requirements(
    storage: &Storage,
    status: &Option<String>,
    priority: &Option<String>,
    req_type: &Option<String>,
    feature: &Option<String>,
    tags: &Option<String>,
) -> Result<()> {
    // Load requirements
    let store = storage.load_for_read()?;
    let mut requirements = store.requirements.clone();

    // Apply filters if provided
    if let Some(status_str) = status {
        let status_filter = parse_list_status_filter(status_str)?;
        requirements.retain(|r| requirement_matches_status_filter(&store, r, &status_filter));
    }

    if let Some(priority_str) = priority {
        let priority_filter = parse_priority(priority_str)?;
        requirements.retain(|r| r.priority == priority_filter);
    }

    if let Some(type_str) = req_type {
        let type_filter = parse_type(type_str)?;
        requirements.retain(|r| r.req_type == type_filter);
    }

    if let Some(feature_str) = feature {
        requirements.retain(|r| r.feature == *feature_str);
    }

    if let Some(tags_str) = tags {
        let tag_filters: Vec<String> = tags_str.split(',').map(|s| s.trim().to_string()).collect();
        requirements.retain(|r| tag_filters.iter().any(|f| tag_filter_matches(f, &r.tags)));
    }

    // Display the requirements
    if requirements.is_empty() {
        println!("{}", "No requirements found.".yellow());
        return Ok(());
    }

    const STATUS_COLUMN_WIDTH: usize = 22;
    println!(
        "{:<10} | {:<36} | {:<30} | {:<STATUS_COLUMN_WIDTH$} | {:<10} | {:<15}",
        "SPEC-ID", "UUID", "Title", "Status", "Priority", "Feature"
    );
    println!("{}", "-".repeat(132));

    for req in requirements {
        let display_status = effective_display_status(&store, &req);
        let status_str =
            list_requirement_status_cell(&store, &req, &display_status, STATUS_COLUMN_WIDTH);
        let priority_str = match req.priority {
            RequirementPriority::High => "High".red(),
            RequirementPriority::Medium => "Medium".yellow(),
            RequirementPriority::Low => "Low".green(),
        };

        let spec_id_display = req.spec_id.as_deref().unwrap_or("-");

        println!(
            "{:<10} | {:<36} | {:<30} | {} | {:<10} | {:<15}",
            spec_id_display,
            req.id.to_string(),
            req.title,
            status_str,
            priority_str,
            req.feature
        );
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ListStatusFilter {
    Stored(RequirementStatus),
    Shelved,
    NeedsDecision,
    // trace:BUG-1687 | ai:claude — the deferred VIEW axis as a `--status`
    // token. Kept in step with the git-backend path on purpose: BUG-1771's
    // lesson was that a token accepted on one listing path and refused on the
    // other is worse than one refused everywhere.
    Deferred,
}

// trace:STORY-1023 | ai:codex
// trace:BUG-1771 | ai:claude — the token set now comes from the shared
// `status_display::LENS_FILTER_TOKENS` table the git-backend list path and its
// "Unknown status filter" refusal also read, instead of a second hand-written
// `match`. The two paths had already drifted once: these tokens worked here and
// errored on the shipped (distributed-mode) CLI.
pub(crate) fn parse_list_status_filter(status_str: &str) -> Result<ListStatusFilter> {
    if let Some(key) = status_display::lens_filter_key(status_str) {
        return match key {
            "Shelved" => Ok(ListStatusFilter::Shelved),
            "NeedsDecision" => Ok(ListStatusFilter::NeedsDecision),
            // A token in the shared table with no arm here is a wiring gap, not
            // a stored status — say so rather than letting `parse_status` reject
            // it with a message that denies the token exists.
            key => anyhow::bail!(
                "status lens '{status_str}' (key '{key}') is accepted by \
                 `aida list --status` but not wired into this listing path"
            ),
        };
    }
    // trace:BUG-1687 | ai:claude — same single-table discipline for the view
    // axes: the token set lives in `VIEW_FILTER_TOKENS`, not in a second
    // hand-written match that can drift from it.
    if let Some(axis) = status_display::view_filter_axis(status_str) {
        return match axis {
            status_display::ViewFilterAxis::Deferred => Ok(ListStatusFilter::Deferred),
        };
    }
    parse_status(status_str).map(ListStatusFilter::Stored)
}

pub(crate) fn requirement_matches_status_filter(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
    status_filter: &ListStatusFilter,
) -> bool {
    // trace:STORY-1023 | ai:codex
    let display_status = effective_display_status(store, req);
    match status_filter {
        ListStatusFilter::Stored(status) => display_status == *status,
        ListStatusFilter::Shelved => matches!(
            effective_needs_attention_lens(store, req, &display_status),
            Some(status_display::NeedsAttentionLens::Shelved { .. })
        ),
        ListStatusFilter::NeedsDecision => matches!(
            effective_needs_attention_lens(store, req, &display_status),
            Some(status_display::NeedsAttentionLens::NeedsDecision { .. })
        ),
        // trace:BUG-1687 | ai:claude — the defer axis, read through the same
        // flag-OR-legacy-tag predicate the cache query uses, so the two listing
        // paths return the same set.
        ListStatusFilter::Deferred => status_display::is_deferred(req),
    }
}

pub(crate) fn list_requirement_status_cell(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
    display_status: &RequirementStatus,
    width: usize,
) -> String {
    // trace:STORY-1023 | ai:codex
    let (label, palette_key) = if matches!(display_status, RequirementStatus::NeedsAttention) {
        effective_needs_attention_lens(store, req, display_status)
            .map(|lens| {
                let label = lens.label();
                let key = lens.palette_key();
                (label, key)
            })
            .unwrap_or_else(|| ("Needs Decision".to_string(), "NeedsDecision"))
    } else {
        let label = match display_status {
            RequirementStatus::Draft => "Draft",
            RequirementStatus::Approved => "Approved",
            RequirementStatus::Planned => "Planned",
            RequirementStatus::InProgress => "In Progress",
            RequirementStatus::Done => "Done",
            RequirementStatus::Completed => "Completed",
            RequirementStatus::Rejected => "Rejected",
            RequirementStatus::Superseded => "Superseded",
            RequirementStatus::NeedsAttention => unreachable!("handled above"),
        };
        (label.to_string(), label)
    };
    let padded = format!("{label:<width$}");
    status_display::paint_status(&padded, palette_key).to_string()
}

pub(crate) fn needs_attention_badge_for_lens(lens: &status_display::NeedsAttentionLens) -> String {
    let label = lens.label();
    let key = lens.palette_key();
    format!(
        "{} {}",
        status_display::status_glyph(key),
        status_display::paint_status(&label, key)
    )
}

pub(crate) fn effective_needs_attention_lens(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
    display_status: &RequirementStatus,
) -> Option<status_display::NeedsAttentionLens> {
    effective_needs_attention_lens_with_source(store, req, display_status).map(|(_, lens)| lens)
}

pub(crate) fn effective_needs_attention_lens_with_source<'a>(
    store: &'a aida_core::RequirementsStore,
    req: &'a aida_core::models::Requirement,
    display_status: &RequirementStatus,
) -> Option<(
    &'a aida_core::models::Requirement,
    status_display::NeedsAttentionLens,
)> {
    if !matches!(display_status, RequirementStatus::NeedsAttention) {
        return None;
    }
    if let Some(lens) = status_display::needs_attention_lens(req) {
        return Some((req, lens));
    }
    if req.req_type == RequirementType::Epic {
        let mut decision_lens = None;
        // trace:STORY-1023 | ai:codex
        for child_id in aida_core::graph_walk::subtree_ids(store, req.id, None).nodes {
            let Some(child) = store.get_requirement_by_id(&child_id) else {
                continue;
            };
            let child_display_status = effective_display_status(store, child);
            match effective_needs_attention_lens_with_source(store, child, &child_display_status) {
                Some((source, status_display::NeedsAttentionLens::Shelved { cause })) => {
                    return Some((
                        source,
                        status_display::NeedsAttentionLens::Shelved { cause },
                    ));
                }
                Some((source, status_display::NeedsAttentionLens::NeedsDecision { reason })) => {
                    decision_lens.get_or_insert((
                        source,
                        status_display::NeedsAttentionLens::NeedsDecision { reason },
                    ));
                }
                None => {}
            }
        }
        if decision_lens.is_some() {
            return decision_lens;
        }
    }
    Some((
        req,
        status_display::NeedsAttentionLens::NeedsDecision { reason: None },
    ))
}

#[cfg(test)]
mod story_1023_list_render_tests {
    use super::*;

    #[test]
    fn list_status_cell_splits_shelved_from_needs_decision() {
        let mut shelved =
            aida_core::models::Requirement::new("stale base".to_string(), String::new());
        shelved.status = RequirementStatus::NeedsAttention;
        shelved.failure_reason = Some(aida_core::FailureReason {
            phase: "review".to_string(),
            phase_index: 3,
            kind: "stale-base".to_string(),
            detail: "base moved under the branch".to_string(),
            recovery_hint: Some("rebase and retry".to_string()),
            shelved_by: Some("codex".to_string()),
            shelved_at: chrono::Utc::now(),
        });

        let mut decision =
            aida_core::models::Requirement::new("design fork".to_string(), String::new());
        decision.status = RequirementStatus::NeedsAttention;
        decision.attention_reason = Some(aida_core::AttentionReason {
            category: aida_core::PuntCategory::DesignFork,
            detail: "choose the public API shape".to_string(),
            lean: None,
            raised_by: Some("codex".to_string()),
            raised_at: chrono::Utc::now(),
        });

        let store = aida_core::RequirementsStore::new();
        colored::control::set_override(false);
        let shelved_cell = list_requirement_status_cell(&store, &shelved, &shelved.status, 22);
        let decision_cell = list_requirement_status_cell(&store, &decision, &decision.status, 22);
        colored::control::unset_override();

        assert!(
            shelved_cell.contains("Shelved (stale-base)"),
            "cell: {shelved_cell:?}"
        );
        assert!(
            !shelved_cell.contains("Needs Attention"),
            "cell: {shelved_cell:?}"
        );
        assert!(
            decision_cell.contains("Needs Decision (design-fork)"),
            "cell: {decision_cell:?}"
        );
    }

    #[test]
    fn status_filter_uses_effective_display_status_and_parked_lens() {
        let mut store = aida_core::RequirementsStore::new();

        let mut epic = aida_core::models::Requirement::new("Epic".to_string(), String::new());
        epic.req_type = RequirementType::Epic;
        epic.status = RequirementStatus::Approved;
        let mut child =
            aida_core::models::Requirement::new("stale child".to_string(), String::new());
        child.status = RequirementStatus::NeedsAttention;
        child.failure_reason = Some(aida_core::FailureReason {
            phase: "review".to_string(),
            phase_index: 3,
            kind: "stale-base".to_string(),
            detail: "base moved under the branch".to_string(),
            recovery_hint: Some("rebase and retry".to_string()),
            shelved_by: Some("codex".to_string()),
            shelved_at: chrono::Utc::now(),
        });
        child.relationships.push(aida_core::models::Relationship {
            rel_type: aida_core::models::RelationshipType::Parent,
            target_id: epic.id,
            created_at: None,
            created_by: None,
        });

        let epic_id = epic.id;
        let child_id = child.id;
        store.requirements.push(epic);
        store.requirements.push(child);

        let epic = store.get_requirement_by_id(&epic_id).unwrap();
        assert_eq!(
            effective_display_status(&store, epic),
            RequirementStatus::NeedsAttention
        );
        assert_ne!(epic.status, RequirementStatus::NeedsAttention);

        colored::control::set_override(false);
        let epic_cell =
            list_requirement_status_cell(&store, epic, &RequirementStatus::NeedsAttention, 22);
        colored::control::unset_override();
        assert!(
            epic_cell.contains("Shelved (stale-base)"),
            "cell: {epic_cell:?}"
        );

        assert!(requirement_matches_status_filter(
            &store,
            epic,
            &ListStatusFilter::Stored(RequirementStatus::NeedsAttention)
        ));
        assert!(requirement_matches_status_filter(
            &store,
            epic,
            &ListStatusFilter::Shelved
        ));
        assert!(!requirement_matches_status_filter(
            &store,
            epic,
            &ListStatusFilter::NeedsDecision
        ));

        let (source, lens) = effective_needs_attention_lens_with_source(
            &store,
            epic,
            &RequirementStatus::NeedsAttention,
        )
        .expect("effective lens");
        assert_eq!(source.id, child_id);
        assert_eq!(lens.label(), "Shelved (stale-base)");
    }
}

/// Read the `[external_refs]` provider → base-URL map from `.aida/config.toml`.
///
/// Each line under the section is `provider = "https://..."` (e.g.
/// `linear = "https://linear.app/acme/issue/"`). The project root is the
/// parent of the orphan store path; fall back to `find_project_root()` and
/// finally CWD. Missing file or section yields an empty map (callers then use
/// the built-in defaults — see `aida_core::external_refs::render_ref_url`).
/// Mirrors the line-by-line `read_config_workflow_hints` pattern.
/// trace:STORY-476 | ai:claude
pub(crate) fn read_external_ref_base_urls(
    store_path: &std::path::Path,
) -> std::collections::HashMap<String, String> {
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .or_else(|| find_project_root().ok())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let config_path = project_root.join(".aida").join("config.toml");
    let mut map = std::collections::HashMap::new();
    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return map,
    };
    let mut in_section = false;
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_section = stripped.trim_end_matches(']').trim() == "external_refs";
            continue;
        }
        if in_section {
            if let Some((key, val)) = line.split_once('=') {
                let provider = key.trim().to_ascii_lowercase();
                // Strip a trailing inline `# comment`, then surrounding quotes.
                let v = val
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
                if !provider.is_empty() && !v.is_empty() {
                    map.insert(provider, v);
                }
            }
        }
    }
    map
}

/// The status to DISPLAY for a requirement. For an EPIC this is the read-only
/// rollup of its children (`derive_epic_status`), so the full-store display
/// surfaces (`aida show`, `aida why`, `aida status`) agree with the cache-backed
/// `aida list`, whose `status` column already holds the derived value. For every
/// non-epic, and for an epic whose only children are Rejected (the derivation
/// declines to auto-reject), this is the stored status.
// trace:BUG-626 | ai:claude
pub(crate) fn effective_display_status(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
) -> RequirementStatus {
    if req.req_type == RequirementType::Epic {
        if let Some(derived) = aida_core::rollup::derive_epic_status(store, req.id) {
            return derived;
        }
    }
    req.status.clone()
}

pub(crate) fn show_requirement(storage: &Storage, id_str: &str) -> Result<()> {
    // Load requirements first (needed for SPEC-ID lookup)
    let store = storage.load_for_read()?;

    // Parse UUID or SPEC-ID
    let id = parse_requirement_id(id_str, &store)?;

    // Find the specified requirement
    let req = store
        .get_requirement_by_id(&id)
        .context("Requirement not found")?;

    // Display the requirement details
    println!("{}: {}", "ID".blue(), req.id);
    if let Some(spec_id) = &req.spec_id {
        println!("{}: {}", "SPEC-ID".blue(), spec_id);
    }
    println!("{}: {}", "Title".blue(), req.title);
    println!("{}: {}", "Description".blue(), req.description);

    // BUG-626: an epic's displayed status is the read-only rollup of its
    // children, not the stored field. trace:BUG-626 | ai:claude
    let display_status = effective_display_status(&store, req);
    // BUG-1687: deferral is a display override that outranks both the parked
    // lens and the stored status — a deferred spec must not read as "act now".
    // trace:BUG-1687 | ai:claude
    let status_str = if let Some(badge) = status_display::deferred_badge(req) {
        badge
    } else if matches!(display_status, RequirementStatus::NeedsAttention) {
        effective_needs_attention_lens(&store, req, &display_status)
            .map(|lens| needs_attention_badge_for_lens(&lens))
            .unwrap_or_else(|| status_display::parked_status_badge(req))
    } else {
        match display_status {
            RequirementStatus::Draft => "Draft".yellow().to_string(),
            RequirementStatus::Approved => "Approved".blue().to_string(),
            RequirementStatus::Planned => "Planned".cyan().to_string(),
            RequirementStatus::InProgress => "In Progress".magenta().to_string(),
            RequirementStatus::Done => "Done".bright_green().bold().to_string(),
            RequirementStatus::Completed => "Completed".green().to_string(),
            RequirementStatus::Rejected => "Rejected".red().to_string(),
            // trace:TASK-1176 | ai:claude
            RequirementStatus::Superseded => "Superseded".green().dimmed().to_string(),
            RequirementStatus::NeedsAttention => unreachable!("handled above"),
        }
    };
    println!("{}: {}", "Status".blue(), status_str);

    let priority_str = match req.priority {
        RequirementPriority::High => "High".red(),
        RequirementPriority::Medium => "Medium".yellow(),
        RequirementPriority::Low => "Low".green(),
    };
    println!("{}: {}", "Priority".blue(), priority_str);

    let type_str = match req.req_type {
        RequirementType::Functional => "Functional",
        RequirementType::NonFunctional => "Non-Functional",
        RequirementType::System => "System",
        RequirementType::User => "User",
        RequirementType::ChangeRequest => "Change Request",
        RequirementType::Bug => "Bug",
        RequirementType::Epic => "Epic",
        RequirementType::Story => "Story",
        RequirementType::Task => "Task",
        RequirementType::Spike => "Spike",
        RequirementType::Sprint => "Sprint",
        RequirementType::Folder => "Folder",
        RequirementType::Meta => "Meta",
        RequirementType::Principle => "Principle",
        RequirementType::Vision => "Vision",
        RequirementType::Constraint => "Constraint",
        RequirementType::Decision => "Decision",
        RequirementType::Term => "Term",
        RequirementType::Doc => "Doc",
        RequirementType::Faq => "Faq",
    };
    println!("{}: {}", "Type".blue(), type_str);

    println!("{}: {}", "Owner".blue(), req.owner);
    // STORY-639: render assignee only when set. trace:STORY-639 | ai:claude
    if let Some(assignee) = req.assignee.as_deref() {
        println!("{}: {}", "Assignee".blue(), assignee);
    }
    println!("{}: {}", "Feature".blue(), req.feature);
    println!("{}: {}", "Created".blue(), req.created_at);
    println!("{}: {}", "Modified".blue(), req.modified_at);

    if !req.tags.is_empty() {
        let tags_str = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        println!("{}: {}", "Tags".blue(), tags_str);
    }

    if !req.dependencies.is_empty() {
        let deps_str = req
            .dependencies
            .iter()
            .map(|uuid| uuid.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("{}: {}", "Dependencies".blue(), deps_str);
    }

    // STORY-542: user-facing interface changes captured at close — the
    // deterministic source the operator digest reads. trace:STORY-542
    if let Some(ic) = req.interface_changes.as_ref() {
        if !ic.is_empty() {
            println!("\n{}:", "Interface changes".green());
            let surfaces: [(&str, &Vec<String>); 4] = [
                ("CLI", &ic.cli),
                ("MCP", &ic.mcp),
                ("TUI", &ic.tui),
                ("Other", &ic.other),
            ];
            for (label, lines) in surfaces {
                for line in lines {
                    println!("  {} {}", format!("[{label}]").cyan(), line);
                }
            }
        }
    }

    // STORY-698: the verification steps the builder ran, captured at
    // `aida queue done` — the implementation audit trail the PR body surfaces.
    // trace:STORY-698 | ai:claude
    if let Some(steps) = req
        .implementation_info
        .as_ref()
        .and_then(|info| info.test_coverage_notes.as_ref())
        .filter(|s| !s.trim().is_empty())
    {
        println!("\n{}:", "Verification steps".green());
        for line in steps.lines().filter(|l| !l.trim().is_empty()) {
            println!("  {} {}", "-".dimmed(), line);
        }
    }

    if !req.relationships.is_empty() {
        println!("\n{}:", "Relationships".green());
        for relationship in &req.relationships {
            let target = store.get_requirement_by_id(&relationship.target_id);
            if let Some(target_req) = target {
                let target_spec = target_req.spec_id.as_deref().unwrap_or("N/A");

                // TASK-102: shared phrase mapping (STORY-333 added typed
                // blocked-by/blocks prose; now via `relationship_phrase`).
                let description = relationship_phrase(&relationship.rel_type);

                println!(
                    "  {} {} - {}",
                    description.cyan(),
                    target_spec.yellow(),
                    target_req.title
                );
            } else {
                println!(
                    "  {} {} {}",
                    relationship.rel_type.to_string().cyan(),
                    relationship.target_id.to_string().yellow(),
                    "(not found)".red()
                );
            }
        }
    }

    if !req.comments.is_empty() {
        println!("\n{}:", "Comments".green());
        for comment in &req.comments {
            print_comment(comment, 0);
        }
    }

    if !req.history.is_empty() {
        println!("\n{}:", "History".green());
        for entry in &req.history {
            println!(
                "\n{}:",
                entry
                    .timestamp
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
                    .yellow()
            );
            println!("  {} {}", "By:".dimmed(), entry.author.cyan());
            for change in &entry.changes {
                println!(
                    "  {} {} → {}",
                    change.field_name.magenta(),
                    change.old_value.red(),
                    change.new_value.green()
                );
            }
        }
    }

    Ok(())
}

/// STORY-582: render the durable processing-record audit trail for `aida
/// show`. One block per record: timestamp + agent header, the linkage
/// (PR/commit/brief), the summary, and the decisions / punted / verdict
/// tails when present. trace:STORY-582 | ai:claude
pub(crate) fn print_processing_records(records: &[aida_core::ProcessingRecord]) {
    println!("\n{}:", "Processing record".green().bold());
    for rec in records {
        println!(
            "\n{}  {}",
            rec.timestamp
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
                .yellow(),
            format!("by {}", rec.agent).cyan(),
        );
        // Linkage line — PR / commit / brief, whichever are present.
        let mut linkage: Vec<String> = Vec::new();
        if let Some(pr) = rec.pr {
            linkage.push(format!("PR #{pr}"));
        }
        if let Some(sha) = rec.commit_sha.as_deref() {
            linkage.push(format!("commit {}", &sha[..sha.len().min(8)]));
        }
        if let Some(brief) = rec.brief_ref.as_deref() {
            linkage.push(format!("brief {brief}"));
        }
        if let Some(v) = rec.review_verdict.as_deref() {
            linkage.push(format!("verdict {v}"));
        }
        if !linkage.is_empty() {
            println!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                linkage.join(" · ").dimmed()
            );
        }
        println!("  {}", rec.summary);
        for d in &rec.decisions {
            println!("  {} {}", "decision:".magenta(), d);
        }
        for p in &rec.punted {
            println!("  {} {}", "punted:".yellow(), p);
        }
    }
}

/// STORY-582: `aida record list|prune` — inspect or trim the durable
/// processing-record audit trail. List is read-only (backend); prune writes
/// through `Storage::update_atomically`, propose-by-default. trace:STORY-582
// trace:REQ-0232 | ai:claude:high
/// What a `--add-tag` / `--remove-tag` pass actually did.
///
/// BUG-1770: the bool this replaces could not distinguish "removed nothing
/// because the tag was absent" from "removed nothing because something went
/// wrong", and the caller rendered both as `No changes specified` — a message
/// that sends the user looking for a flag they already passed. Naming the tags
/// that matched nothing turns a silent no-op into a visible one.
// trace:BUG-1770 | ai:claude
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TagDeltaReport {
    /// Tags newly inserted.
    pub(crate) added: Vec<String>,
    /// Tags actually removed.
    pub(crate) removed: Vec<String>,
    /// `--add-tag` values that were already present.
    pub(crate) already_present: Vec<String>,
    /// `--remove-tag` values that matched no existing tag.
    pub(crate) absent: Vec<String>,
}

impl TagDeltaReport {
    pub(crate) fn changed(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty()
    }

    /// One line per outcome, or `None` when nothing was requested at all.
    /// Deterministic order so output is stable for tests and scripts.
    pub(crate) fn summary_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let list = |v: &[String]| v.join(", ");
        if !self.added.is_empty() {
            out.push(format!(
                "added {}: {}",
                plural(self.added.len(), "tag"),
                list(&self.added)
            ));
        }
        if !self.removed.is_empty() {
            out.push(format!(
                "removed {}: {}",
                plural(self.removed.len(), "tag"),
                list(&self.removed)
            ));
        }
        if !self.already_present.is_empty() {
            out.push(format!(
                "already present, nothing added: {}",
                list(&self.already_present)
            ));
        }
        if !self.absent.is_empty() {
            out.push(format!(
                "no matching {} to remove: {}",
                if self.absent.len() == 1 {
                    "tag"
                } else {
                    "tags"
                },
                list(&self.absent)
            ));
        }
        out
    }
}

pub(crate) fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// The flag name used in a whitespace refusal raised for `aida add --tags` /
/// `aida edit --tags`, where the pasteable repair is one comma-separated value.
// trace:BUG-1770 | ai:claude
pub(crate) const TAGS_FLAG: &str = "--tags";

/// How a refused tag value arrived, so the refusal can show a repair the
/// caller can actually paste back.
///
/// A refusal that suggests `--tags a,b` to an MCP client, or a comma list to a
/// repeatable flag, names a form that surface does not accept — which is a
/// worse failure than the blob, because it reads as authoritative.
// trace:BUG-1770 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagRepair {
    /// A comma-separated argument: `--tags a,b`.
    CommaFlag(&'static str),
    /// A repeatable single-value flag: `--add-tag a --add-tag b`.
    RepeatedFlag(&'static str),
    /// A JSON array field on an MCP tool call: `"tags": ["a", "b"]`.
    JsonArray(&'static str),
}

impl TagRepair {
    /// The pasteable repair for `parts`, in this surface's own syntax.
    pub(crate) fn suggestion(&self, parts: &[&str]) -> String {
        match self {
            Self::CommaFlag(flag) => format!("{flag} {}", parts.join(",")),
            Self::RepeatedFlag(flag) => parts
                .iter()
                .map(|p| format!("{flag} {p}"))
                .collect::<Vec<_>>()
                .join(" "),
            Self::JsonArray(field) => format!(
                "\"{field}\": [{}]",
                parts
                    .iter()
                    .map(|p| format!("\"{p}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// Refuse a tag value that contains whitespace.
///
/// A space-separated `--tags` argument is stored as ONE tag whose human
/// rendering is byte-identical to the N tags it was meant to be, so the
/// malformation is invisible by eye while being unmatchable by `--remove-tag`
/// and invisible to every tag-keyed filter and sweep. Refusing is preferred
/// over silently splitting on whitespace: splitting would make a genuinely
/// intended multi-word tag unrepresentable without warning, and the caller's
/// intent here is unambiguous enough to just report.
///
/// `flag` is the flag the value arrived on, so the suggestion is pasteable.
/// `--remove-tag` is deliberately NOT validated: a tag already malformed in the
/// store can only be named by reproducing it verbatim, and refusing that would
/// leave the existing blobs unrepairable by the incremental form.
// trace:BUG-1770 | ai:claude
pub(crate) fn validate_tag_value(tag: &str, repair: TagRepair) -> anyhow::Result<()> {
    if !tag.chars().any(char::is_whitespace) {
        return Ok(());
    }
    let parts: Vec<&str> = tag.split_whitespace().collect();
    let suggestion = repair.suggestion(&parts);
    anyhow::bail!(
        "tag \"{tag}\" contains whitespace — did you mean `{suggestion}` ?\n  \
         A tag may not contain whitespace: the whole value would be stored as ONE tag that \
         renders identically to the {} separate tags it looks like, and would then be \
         invisible to --remove-tag and to every tag-keyed filter.",
        parts.len()
    )
}

/// Parse a comma-separated `--tags` argument into validated tag values,
/// trimmed, empties dropped, order preserved.
///
/// Every `--tags` write path goes through this so the whitespace rule cannot be
/// reintroduced by a fourth site splitting the string itself.
// trace:BUG-1770 | ai:claude
pub(crate) fn parse_tag_list(raw: &str) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    for tag in raw.split(',') {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            continue;
        }
        validate_tag_value(trimmed, TagRepair::CommaFlag(TAGS_FLAG))?;
        out.push(trimmed.to_string());
    }
    Ok(out)
}

/// The reporting form. [`apply_tag_deltas`] is the bool-returning wrapper kept
/// for callers that only need "did anything change".
// trace:BUG-1770 | ai:claude
pub(crate) fn apply_tag_deltas_report(
    tags: &mut HashSet<String>,
    add: &[String],
    remove: &[String],
) -> anyhow::Result<TagDeltaReport> {
    // BUG-1770: validate the entire `add` list BEFORE mutating anything, so a
    // refusal can never leave a half-applied tag set behind for the caller to
    // save. trace:BUG-1770 | ai:claude
    for raw in add {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            validate_tag_value(trimmed, TagRepair::RepeatedFlag("--add-tag"))?;
        }
    }
    let mut report = TagDeltaReport::default();
    for raw in add {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if tags.insert(trimmed.to_string()) {
            report.added.push(trimmed.to_string());
        } else {
            report.already_present.push(trimmed.to_string());
        }
    }
    for raw in remove {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if tags.remove(trimmed) {
            report.removed.push(trimmed.to_string());
        } else {
            report.absent.push(trimmed.to_string());
        }
    }
    Ok(report)
}

/// Apply additive (`--add-tag`) and subtractive (`--remove-tag`) tag deltas
/// without disturbing tags the caller didn't name. Empty / whitespace-only
/// entries are ignored. Adding a present tag or removing an absent one is
/// a graceful no-op. Returns whether the set actually changed.
// trace:TASK-351 | ai:claude
pub(crate) fn apply_tag_deltas(
    tags: &mut HashSet<String>,
    add: &[String],
    remove: &[String],
) -> anyhow::Result<bool> {
    Ok(apply_tag_deltas_report(tags, add, remove)?.changed())
}

/// Build the loud-on-clobber warning shown when `aida edit --tags` REPLACES the
/// whole tag set. `--tags` is a full replace (other scripts depend on that), so
/// we don't change its semantics — we surface the old→new diff so the caller
/// sees that provenance/routing tags were dropped, and point at the incremental
/// `--add-tag` / `--remove-tag` forms. Returns `None` when the set is unchanged
/// (no warning needed). The returned string is sorted for deterministic output.
/// trace:BUG-545 | ai:claude
pub(crate) fn tags_replace_warning(
    old_tags: &HashSet<String>,
    new_tags: &HashSet<String>,
) -> Option<String> {
    if old_tags == new_tags {
        return None;
    }
    let fmt_set = |s: &HashSet<String>| -> String {
        if s.is_empty() {
            "(none)".to_string()
        } else {
            let mut v: Vec<&String> = s.iter().collect();
            v.sort();
            v.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(",")
        }
    };
    Some(format!(
        "--tags REPLACES all tags (was: {} → now: {}). \
         Use --add-tag/--remove-tag to modify incrementally.",
        fmt_set(old_tags),
        fmt_set(new_tags)
    ))
}

// trace:BUG-1252 | ai:codex
/// Structural tags participate in routing and graph integrity. A full tag-set
/// replacement must not silently erase them.
pub(crate) fn dropped_structural_tags(
    old_tags: &HashSet<String>,
    new_tags: &HashSet<String>,
) -> Vec<String> {
    const PREFIXES: &[&str] = &[
        "parent:",
        "batch:",
        "lane:",
        "severity:",
        "lifecycle:",
        "aida:",
    ];
    let mut dropped: Vec<String> = old_tags
        .difference(new_tags)
        .filter(|tag| PREFIXES.iter().any(|prefix| tag.starts_with(prefix)))
        .cloned()
        .collect();
    dropped.sort();
    dropped
}

pub(crate) fn enforce_structural_tag_replacement(
    old_tags: &HashSet<String>,
    new_tags: &HashSet<String>,
    force: bool,
) -> Result<Vec<String>> {
    let dropped = dropped_structural_tags(old_tags, new_tags);
    if !dropped.is_empty() && !force {
        anyhow::bail!(
            "AIDA_AGENT_OUTPUT structural_tags_dropped=[{}] refusal=use_--add-tag/--remove-tag_or_pass_--force; refusing --tags replacement because it would drop structural tags: {}. Use --add-tag/--remove-tag for incremental edits, or --force/--replace-tags to replace intentionally.",
            dropped.join(","), dropped.join(", ")
        );
    }
    Ok(dropped)
}

/// STORY-439: stamp `complexity:<level>` / `estimated-assistance:<level>`
/// tags on `spec` so the new dimension composes with existing tag tooling
/// (`aida queue list --tag-prefix complexity:`, batch routing). Mirrors
/// `load_store_for_lookup` for backend resolution. Best-effort — a missing
/// store / missing spec / save failure logs and returns; the pickup itself
/// is unaffected. trace:STORY-439 | ai:claude
pub(crate) fn apply_calibration_tags(
    storage: &Storage,
    spec: &str,
    complexity: Option<complexity_calibration::ComplexityLevel>,
    assist_est: Option<complexity_calibration::AssistanceLevel>,
) {
    if complexity.is_none() && assist_est.is_none() {
        return;
    }
    // Git-canonical path — direct backend write, exactly like Command::Edit.
    if let Some(store_path) = detect_distributed_store() {
        // Cache-backed so the checked lookup below does not load the whole
        // store. trace:TASK-1468 | ai:claude
        if let Ok(backend) = aida_core::CachedGitBackend::open(
            &store_path,
            &aida_core::CachedGitBackend::default_cache_path(&store_path),
        ) {
            use aida_core::DatabaseBackend;
            // trace:TASK-1468 | ai:claude
            let Ok(Some(mut req)) = backend.get_requirement_unambiguous(spec) else {
                return;
            };
            let mut changed = false;
            if let Some(c) = complexity {
                changed |= complexity_calibration::apply_complexity_tag(&mut req.tags, c);
            }
            if let Some(a) = assist_est {
                changed |= complexity_calibration::apply_assistance_tag(&mut req.tags, a);
            }
            if changed {
                req.modified_at = chrono::Utc::now();
                if let Err(e) = backend.update_requirement(&req) {
                    eprintln!(
                        "  {} could not stamp calibration tags on {spec}: {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            }
            return;
        }
    }
    // Legacy fallback — load the whole store, mutate, save. Heavier but
    // only fires on projects that haven't migrated.
    let Ok(mut store) = storage.load() else {
        return;
    };
    let Some(req) = store
        .requirements
        .iter_mut()
        .find(|r| r.spec_id.as_deref() == Some(spec) || r.agreed_id.as_deref() == Some(spec))
    else {
        return;
    };
    let mut changed = false;
    if let Some(c) = complexity {
        changed |= complexity_calibration::apply_complexity_tag(&mut req.tags, c);
    }
    if let Some(a) = assist_est {
        changed |= complexity_calibration::apply_assistance_tag(&mut req.tags, a);
    }
    if changed {
        req.modified_at = chrono::Utc::now();
        if let Err(e) = storage.save(&store) {
            eprintln!(
                "  {} could not save calibration tags on {spec}: {e}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
    }
}

/// STORY-451: stamp `effort:<touchpoint>:<bucket>` while preserving the
/// other effort touchpoints. Best-effort sibling of [`apply_calibration_tags`].
/// trace:STORY-451 | ai:codex
pub(crate) fn apply_effort_tag(
    storage: &Storage,
    spec: &str,
    touchpoint: effort_calibration::EffortTouchpoint,
    effort: Option<effort_calibration::EffortBucket>,
) {
    let Some(effort) = effort else {
        return;
    };
    if let Some(store_path) = detect_distributed_store() {
        // Cache-backed so the checked lookup below does not load the whole
        // store. trace:TASK-1468 | ai:claude
        if let Ok(backend) = aida_core::CachedGitBackend::open(
            &store_path,
            &aida_core::CachedGitBackend::default_cache_path(&store_path),
        ) {
            use aida_core::DatabaseBackend;
            // trace:TASK-1468 | ai:claude
            let Ok(Some(mut req)) = backend.get_requirement_unambiguous(spec) else {
                return;
            };
            if effort_calibration::apply_effort_tag(&mut req.tags, touchpoint, effort) {
                req.modified_at = chrono::Utc::now();
                if let Err(e) = backend.update_requirement(&req) {
                    eprintln!(
                        "  {} could not stamp effort tag on {spec}: {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            }
            return;
        }
    }
    let Ok(mut store) = storage.load() else {
        return;
    };
    let Some(req) = store
        .requirements
        .iter_mut()
        .find(|r| r.spec_id.as_deref() == Some(spec) || r.agreed_id.as_deref() == Some(spec))
    else {
        return;
    };
    if effort_calibration::apply_effort_tag(&mut req.tags, touchpoint, effort) {
        req.modified_at = chrono::Utc::now();
        if let Err(e) = storage.save(&store) {
            eprintln!(
                "  {} could not save effort tag on {spec}: {e}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
    }
}

#[cfg(test)]
#[path = "tests/apply_tag_deltas_tests.rs"]
mod apply_tag_deltas_tests;

#[cfg(test)]
#[path = "tests/bug_1770_tag_whitespace_tests.rs"]
mod bug_1770_tag_whitespace_tests;

#[cfg(test)]
#[path = "tests/tags_replace_warning_tests.rs"]
mod tags_replace_warning_tests;

/// Edit a requirement non-interactively using CLI flags
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn edit_requirement_cli(
    storage: &Storage,
    id_str: &str,
    title: &Option<String>,
    description: &Option<String>,
    status: &Option<String>,
    priority: &Option<String>,
    req_type: &Option<String>,
    owner: &Option<String>,
    feature: &Option<String>,
    tags: &Option<String>,
    // trace:TASK-351 | ai:claude
    add_tag: &[String],
    remove_tag: &[String],
) -> Result<()> {
    // Load requirements
    let store_for_lookup = storage.load()?;
    let id = parse_requirement_id(id_str, &store_for_lookup)?;

    let mut store = storage.load()?;
    let req = store
        .get_requirement_by_id_mut(&id)
        .context("Requirement not found")?;

    let mut changes: Vec<FieldChange> = Vec::new();
    let spec_id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());

    // Update title
    if let Some(new_title) = title {
        if !new_title.is_empty() && new_title != &req.title {
            changes.push(Requirement::field_change(
                "title",
                req.title.clone(),
                new_title.clone(),
            ));
            req.title = new_title.clone();
        }
    }

    // Update description
    if let Some(new_desc) = description {
        if new_desc != &req.description {
            changes.push(Requirement::field_change(
                "description",
                req.description.clone(),
                new_desc.clone(),
            ));
            req.description = new_desc.clone();
        }
    }

    // Update status
    let mut left_needs_attention = false;
    // STORY-1649: captured against the pre-edit status so the completion
    // linkage warning fires only on the into-Completed transition.
    // trace:STORY-1649 | ai:claude
    let mut into_completed = false;
    if let Some(status_str) = status {
        let new_status = match status_str.to_lowercase().as_str() {
            "draft" => RequirementStatus::Draft,
            "approved" => RequirementStatus::Approved,
            "planned" => RequirementStatus::Planned,
            "in_progress" | "in-progress" | "inprogress" => RequirementStatus::InProgress,
            "done" => RequirementStatus::Done,
            "completed" => RequirementStatus::Completed,
            "rejected" => RequirementStatus::Rejected,
            "needs_attention" | "needs-attention" | "needsattention" => {
                RequirementStatus::NeedsAttention
            }
            _ => anyhow::bail!("Invalid status '{}'. Use: draft, approved, planned, in_progress, done, completed, rejected, needs-attention", status_str),
        };
        // STORY-332: enforce the NeedsAttention transition rules here too.
        if let Some(msg) = forbidden_attention_transition(&req.status, &new_status) {
            anyhow::bail!(msg);
        }
        if new_status != req.status {
            // TASK-358: a triage that takes a spec out of NeedsAttention is
            // the trigger for cleaning up any orchestrator-escalated worktree
            // for it. Capture the transition direction before the field
            // changes — the actual cleanup fires after the store save so a
            // failed save doesn't leave a half-cleaned state.
            // trace:TASK-358 | ai:claude
            left_needs_attention = matches!(req.status, RequirementStatus::NeedsAttention)
                && !matches!(new_status, RequirementStatus::NeedsAttention);
            changes.push(Requirement::field_change(
                "status",
                format!("{:?}", req.status),
                format!("{:?}", new_status),
            ));
            let prior = req.status.clone();
            into_completed = !matches!(prior, RequirementStatus::Completed)
                && matches!(new_status, RequirementStatus::Completed);
            req.status = new_status;
            // trace:TASK-1600 | ai:codex
            completion::record_reopen(req, &prior, Some(&queue_cmd::requeue_project_root(storage)));
        }
    }

    // Update priority
    if let Some(priority_str) = priority {
        let new_priority = match priority_str.to_lowercase().as_str() {
            "high" => RequirementPriority::High,
            "medium" | "med" => RequirementPriority::Medium,
            "low" => RequirementPriority::Low,
            _ => anyhow::bail!(
                "Invalid priority '{}'. Use: high, medium, low",
                priority_str
            ),
        };
        if new_priority != req.priority {
            changes.push(Requirement::field_change(
                "priority",
                format!("{:?}", req.priority),
                format!("{:?}", new_priority),
            ));
            req.priority = new_priority;
        }
    }

    // Update type
    if let Some(type_str) = req_type {
        let new_type = match type_str.to_lowercase().as_str() {
            "functional" | "func" => RequirementType::Functional,
            "non-functional" | "nonfunctional" | "nfr" => RequirementType::NonFunctional,
            "system" | "sys" => RequirementType::System,
            "user" => RequirementType::User,
            "change-request" | "change" | "cr" => RequirementType::ChangeRequest,
            "bug" => RequirementType::Bug,
            "epic" => RequirementType::Epic,
            "story" => RequirementType::Story,
            "task" => RequirementType::Task,
            "spike" => RequirementType::Spike,
            "sprint" => RequirementType::Sprint,
            "folder" => RequirementType::Folder,
            "meta" => RequirementType::Meta,
            // ADR / knowledge-graph family (FR-1-074). trace:TASK-716 | ai:claude
            "principle" | "prin" => RequirementType::Principle,
            "vision" | "vis" => RequirementType::Vision,
            "constraint" | "con" => RequirementType::Constraint,
            "decision" | "adr" => RequirementType::Decision,
            "term" | "glossary" => RequirementType::Term,
            // trace:STORY-104 | ai:claude
            "doc" | "documentation" => RequirementType::Doc,
            "faq" => RequirementType::Faq,
            _ => anyhow::bail!("Invalid type '{}'. Use: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc, faq", type_str),
        };
        if new_type != req.req_type {
            changes.push(Requirement::field_change(
                "type",
                format!("{:?}", req.req_type),
                format!("{:?}", new_type),
            ));
            req.req_type = new_type;
        }
    }

    // Update owner
    if let Some(new_owner) = owner {
        if new_owner != &req.owner {
            changes.push(Requirement::field_change(
                "owner",
                req.owner.clone(),
                new_owner.clone(),
            ));
            req.owner = new_owner.clone();
        }
    }

    // Update feature
    if let Some(new_feature) = feature {
        if new_feature != &req.feature {
            changes.push(Requirement::field_change(
                "feature",
                req.feature.clone(),
                new_feature.clone(),
            ));
            req.feature = new_feature.clone();
        }
    }

    // Update tags
    if let Some(tags_str) = tags {
        // BUG-1770: this is the SECOND edit backend the bug's acceptance names,
        // and it split the argument itself rather than sharing a parser — so
        // the whitespace rule has to be routed through `parse_tag_list` here
        // too or `aida edit --tags "a b"` stays reachable from this path.
        // trace:BUG-1770 | ai:claude
        let new_tags: HashSet<String> = parse_tag_list(tags_str)?.into_iter().collect();
        let old_tags: String = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        let new_tags_str: String = new_tags.iter().cloned().collect::<Vec<_>>().join(", ");
        if new_tags != req.tags {
            changes.push(Requirement::field_change("tags", old_tags, new_tags_str));
            req.tags = new_tags;
        }
    }

    // TASK-351: partial tag edits — additive / subtractive variants that
    // don't clobber the rest. `--tags` and `--add-tag`/`--remove-tag` are
    // mutually exclusive at the clap layer, so at most one of the two
    // blocks fires.
    // trace:TASK-351 | ai:claude
    // BUG-1770: keep the REPORT here too. This backend discarded the bool and
    // then fell through to "No changes made to <ID>" — the same conflation of
    // "nothing was asked" with "a tag flag matched nothing" that AC3 narrows on
    // the canonical backend. trace:BUG-1770 | ai:claude
    let mut tag_report = TagDeltaReport::default();
    if !add_tag.is_empty() || !remove_tag.is_empty() {
        let old_tags: String = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        tag_report = apply_tag_deltas_report(&mut req.tags, add_tag, remove_tag)?;
        if tag_report.changed() {
            let new_tags_str: String = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
            changes.push(Requirement::field_change("tags", old_tags, new_tags_str));
        }
    }

    if changes.is_empty() {
        let tag_lines = tag_report.summary_lines();
        if tag_lines.is_empty() {
            println!("{} No changes made to {}", "!".yellow(), spec_id);
        } else {
            for line in tag_lines {
                println!("{line}");
            }
        }
        return Ok(());
    }

    // BUG-1637: record under the caller's identity (was the literal "CLI"),
    // and the status change through the one shared history helper.
    // trace:BUG-1637 | ai:claude
    record_edit_changes(req, &get_default_author(), &changes);
    // STORY-1649: computed while `req` is still borrowed, printed after the
    // save so the warning never precedes the success line.
    // trace:STORY-1649 | ai:claude
    let completion_warning = if into_completed {
        crate::shipped_linkage::completion_linkage_warning(
            &queue_cmd::requeue_project_root(storage),
            req,
        )
    } else {
        None
    };

    // Save changes
    storage.save(&store)?;
    println!(
        "{} Updated {} ({} field(s) changed)",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        spec_id,
        changes.len()
    );
    if let Some(text) = completion_warning {
        eprintln!("{} {text}", "Warning:".yellow().bold());
    }
    // BUG-1770: say what the tag flags did on the success path too — a caller
    // who removes two tags and mistypes one must not see only "Updated".
    // trace:BUG-1770 | ai:claude
    for line in tag_report.summary_lines() {
        println!("  {line}");
    }

    // TASK-358: triage out of NeedsAttention — clean up any orchestrator-
    // escalated worktree for this spec. The lease's `escalated_to_human`
    // marker (stamped by the `--escalate-blocks` path) is the safety gate:
    // an interactive user session on the same spec, or an
    // `--escalate-defaults` advisor-resume, has the marker absent and is
    // left alone. trace:TASK-358 | ai:claude
    if left_needs_attention {
        if let Ok(project_root) = find_project_root() {
            cleanup_escalated_leases_for_spec(&project_root, &spec_id);
            // BUG-674: close the now-stale open punt-ledger record for a spec
            // resumed out of NeedsAttention. trace:BUG-674 | ai:claude
            let _ = punt::close_open_records(
                &project_root,
                &spec_id,
                punt::RESOLUTION_HUMAN_RESOLVED,
                "resolved",
                None,
                Some("resume"),
            );
        }
    }

    Ok(())
}

pub(crate) fn edit_requirement_interactive(storage: &Storage, id_str: &str) -> Result<()> {
    // Load requirements first (needed for SPEC-ID lookup)
    let store_for_lookup = storage.load()?;

    // Parse UUID or SPEC-ID
    let id = parse_requirement_id(id_str, &store_for_lookup)?;

    // Load again as mutable
    let mut store = storage.load()?;

    // Find the specified requirement
    let req = store
        .get_requirement_by_id_mut(&id)
        .context("Requirement not found")?;

    // Track changes
    let mut changes: Vec<FieldChange> = Vec::new();
    let _old_req = req.clone();

    println!("Editing requirement: {}", req.title);
    println!("Leave field empty to keep current value");

    // Update title
    let title_prompt = format!("Title [{}]:", req.title);
    if let Ok(new_title) = inquire::Text::new(&title_prompt).prompt() {
        if !new_title.is_empty() && new_title != req.title {
            changes.push(Requirement::field_change(
                "title",
                req.title.clone(),
                new_title.clone(),
            ));
            req.title = new_title;
        }
    }

    // Update description
    println!("Current description:");
    println!("{}", req.description);

    let description_prompt = "New description (leave empty to keep current):";
    if let Ok(new_description) = inquire::Editor::new(description_prompt)
        .with_predefined_text(&req.description)
        .prompt()
    {
        if new_description != req.description {
            changes.push(Requirement::field_change(
                "description",
                req.description.clone(),
                new_description.clone(),
            ));
            req.description = new_description;
        }
    }

    // Update status
    let status_options = vec![
        RequirementStatus::Draft,
        RequirementStatus::Approved,
        RequirementStatus::Planned,
        RequirementStatus::InProgress,
        RequirementStatus::Done,
        RequirementStatus::Completed,
        RequirementStatus::Rejected,
    ];
    if let Ok(new_status) = inquire::Select::new("Status:", status_options).prompt() {
        if new_status != req.status {
            // STORY-332: the picker does not offer NeedsAttention (it is
            // reached only via `aida punt`), but a NeedsAttention spec can
            // still be edited here — enforce the transition rules so it
            // only resolves to Approved / In Progress / Rejected.
            if let Some(msg) = forbidden_attention_transition(&req.status, &new_status) {
                anyhow::bail!(msg);
            }
            changes.push(Requirement::field_change(
                "status",
                format!("{:?}", req.status),
                format!("{:?}", new_status),
            ));
            let prior = req.status.clone();
            req.status = new_status;
            // trace:TASK-1600 | ai:codex
            completion::record_reopen(req, &prior, Some(&queue_cmd::requeue_project_root(storage)));
        }
    }

    // Update priority
    let priority_options = vec![
        RequirementPriority::High,
        RequirementPriority::Medium,
        RequirementPriority::Low,
    ];
    if let Ok(new_priority) = inquire::Select::new("Priority:", priority_options).prompt() {
        if new_priority != req.priority {
            changes.push(Requirement::field_change(
                "priority",
                format!("{:?}", req.priority),
                format!("{:?}", new_priority),
            ));
            req.priority = new_priority;
        }
    }

    // Update owner
    let owner_prompt = format!("Owner [{}]:", req.owner);
    if let Ok(new_owner) = inquire::Text::new(&owner_prompt).prompt() {
        if !new_owner.is_empty() && new_owner != req.owner {
            changes.push(Requirement::field_change(
                "owner",
                req.owner.clone(),
                new_owner.clone(),
            ));
            req.owner = new_owner;
        }
    }

    // Update feature
    let feature_prompt = format!("Feature [{}]:", req.feature);
    if let Ok(new_feature) = inquire::Text::new(&feature_prompt).prompt() {
        if !new_feature.is_empty() && new_feature != req.feature {
            changes.push(Requirement::field_change(
                "feature",
                req.feature.clone(),
                new_feature.clone(),
            ));
            req.feature = new_feature;
        }
    }

    // Get author for history
    let author = inquire::Text::new("Your name (for history):")
        .prompt()
        .unwrap_or_else(|_| String::from("Unknown"));

    // Record changes. BUG-1637: the status change goes through the one
    // shared history helper. trace:BUG-1637 | ai:claude
    record_edit_changes(req, &author, &changes);

    // Save changes
    storage.save(&store)?;
    println!("{}", "Requirement updated successfully!".green());

    Ok(())
}

pub(crate) fn delete_requirement(
    storage: &Storage,
    id_str: &str,
    skip_confirm: bool,
) -> Result<()> {
    // Load requirements first (needed for SPEC-ID lookup)
    let store_for_lookup = storage.load()?;

    // Parse UUID or SPEC-ID
    let id = parse_requirement_id(id_str, &store_for_lookup)?;

    // Load again as mutable
    let mut store = storage.load()?;

    // Find the requirement to delete
    let req = store
        .get_requirement_by_id(&id)
        .context("Requirement not found")?;

    // Display requirement info
    println!("{}", "Requirement to delete:".yellow());
    println!("  ID: {}", req.id);
    if let Some(spec_id) = &req.spec_id {
        println!("  SPEC-ID: {}", spec_id);
    }
    println!("  Title: {}", req.title);
    println!("  Description: {}", req.description);

    // Confirm deletion unless --yes flag is used
    if !skip_confirm {
        // trace:STORY-809 | ai:claude
        let card = context_prompt::ContextCard {
            decision: "whether to permanently delete this requirement from the store".to_string(),
            provenance: vec![
                "deletion removes the YAML object outright — unlike archive/reject, no audit row survives".to_string(),
            ],
            answers: vec![
                "y: the requirement and its relationships are removed (recoverable only from store git history)".to_string(),
                "n: nothing changes — consider `aida archive` or `--status rejected` instead".to_string(),
            ],
            recommended_default: "n — archive or reject preserves the audit trail; delete is for mistakes/spam".to_string(),
        };
        let confirm = context_prompt::confirm_with_context(
            "Are you sure you want to delete this requirement?",
            false,
            &card,
        )?;

        if !confirm {
            println!("{}", "Deletion cancelled.".yellow());
            return Ok(());
        }
    }

    // Remove the requirement
    store.requirements.retain(|r| r.id != id);

    // Save changes
    storage.save(&store)?;
    println!("{}", "Requirement deleted successfully!".green());

    Ok(())
}

/// BUG-773: `queue add` must not report success for work that the default queue
/// projection will immediately hide. Deferred and archived specs are parked
/// outside the work queue; unlike terminal rows, there is no queue-list widening
/// flag that makes them visible, so accepting the write creates a silent
/// add/list inconsistency.
// trace:BUG-773 | ai:codex
pub(crate) fn queue_add_hidden_view_reason(req: &aida_core::Requirement) -> Option<&'static str> {
    if req.archived {
        Some("archived")
    } else if req.deferred {
        Some("deferred")
    } else {
        None
    }
}

/// BUG-249: validate that a `queue move` target is actually movable —
/// distinguishing "not in the queue" from "in the queue but terminal".
/// Pure function over `entries`; the handler builds the args from its
/// already-resolved `req`/`entries`/`force` state.
///
/// Returns:
///   - `Err` "is not in the queue" when the target isn't present at all.
///   - `Err` "has terminal status (…) — pass --force …" when present but
///     the spec is Completed/Rejected and `--force` wasn't given.
///   - `Ok(())` otherwise.
///
/// Note: `queue_list(&user_id, /* include_completed */ true)` is the
/// entries we want — terminal entries do linger in the queue file (they
/// just aren't shown in the default `queue list` view), so the
/// in-queue-but-terminal state is real and distinct from the missing-
/// entry state.
/// The epic + its TRANSITIVE descendant UUIDs (children + grandchildren + …),
/// for `aida queue list --epic <ID>`.
///
/// Reuses the one shared subtree closure `aida graph tree <ID>` and
/// `aida focus` use (`graph_walk::subtree_ids`, TASK-1074): every hierarchy edge
/// is oriented parent->child by type rank (rel_type breaking same-rank ties),
/// then walked downward — so the tree is traversed whichever side recorded the
/// edge and in whichever of the two historical orientations, without leaking a
/// descendant's same-rank second parent. `subtree_ids` excludes the root, so we
/// re-insert the epic's own UUID — an item queued directly against the epic
/// counts as "under" it.
// trace:TASK-923 trace:TASK-1074 | ai:claude
pub(crate) fn epic_descendant_uuid_set(
    store: &RequirementsStore,
    epic_id: uuid::Uuid,
) -> std::collections::HashSet<uuid::Uuid> {
    let result = aida_core::graph_walk::subtree_ids(store, epic_id, None);
    let mut set: std::collections::HashSet<uuid::Uuid> = result.nodes.into_iter().collect();
    set.insert(epic_id);
    set
}

/// Pure filter — keep only the queue entries whose `requirement_id` is in
/// `descendants` (an epic's transitive descendant UUID set from
/// [`epic_descendant_uuid_set`]). Order-preserving; composes cleanly with the
/// role / scope / tag filters that run before it (they all reduce the same
/// `&QueueEntry` slice). An empty `descendants` yields an empty result.
// trace:TASK-923 | ai:claude
pub(crate) fn filter_entries_by_descendant_set<'a>(
    entries: &[&'a aida_core::QueueEntry],
    descendants: &std::collections::HashSet<uuid::Uuid>,
) -> Vec<&'a aida_core::QueueEntry> {
    entries
        .iter()
        .copied()
        .filter(|e| descendants.contains(&e.requirement_id))
        .collect()
}

// Unit coverage for the `aida queue list --epic/--parent` closure and pure
// filter. The closure (`epic_descendant_uuid_set`) must be transitive (epic +
// child + grandchild) and include the epic itself; the pure filter
// (`filter_entries_by_descendant_set`) must keep only in-set entries, compose
// with a prior role filter (it reduces the same already-filtered slice), and
// map an empty descendant set to an empty result.
// trace:TASK-923 | ai:claude
#[cfg(test)]
#[path = "tests/epic_queue_filter_tests.rs"]
mod epic_queue_filter_tests;

// trace:BUG-249 | ai:claude
pub(crate) fn classify_queue_move_target(
    target_id: uuid::Uuid,
    target_display: &str,
    target_status: &RequirementStatus,
    entries: &[aida_core::QueueEntry],
    force: bool,
) -> Result<()> {
    if !entries.iter().any(|e| e.requirement_id == target_id) {
        anyhow::bail!(
            "{} is not in the queue (run `aida queue add {}` first)",
            target_display,
            target_display
        );
    }
    if is_terminal_status(target_status) && !force {
        anyhow::bail!(
            "{} has terminal status ({:?}) — pass --force to move it anyway, or supersede with a new spec",
            target_display,
            target_status,
        );
    }
    Ok(())
}

pub(crate) fn parse_status(status_str: &str) -> Result<RequirementStatus> {
    match status_str.to_lowercase().as_str() {
        "draft" => Ok(RequirementStatus::Draft),
        "approved" => Ok(RequirementStatus::Approved),
        "planned" => Ok(RequirementStatus::Planned),
        "in_progress" | "in-progress" | "inprogress" => Ok(RequirementStatus::InProgress),
        "done" => Ok(RequirementStatus::Done),
        "completed" => Ok(RequirementStatus::Completed),
        "rejected" => Ok(RequirementStatus::Rejected),
        // trace:TASK-1176 | ai:claude
        "superseded" => Ok(RequirementStatus::Superseded),
        "needs_attention" | "needs-attention" | "needsattention" => {
            Ok(RequirementStatus::NeedsAttention)
        }
        _ => anyhow::bail!("Invalid status: {}", status_str),
    }
}

pub(crate) fn parse_priority(priority_str: &str) -> Result<RequirementPriority> {
    match priority_str.to_lowercase().as_str() {
        "high" => Ok(RequirementPriority::High),
        "medium" => Ok(RequirementPriority::Medium),
        "low" => Ok(RequirementPriority::Low),
        _ => anyhow::bail!("Invalid priority: {}", priority_str),
    }
}

pub(crate) fn parse_type(type_str: &str) -> Result<RequirementType> {
    match type_str.to_lowercase().as_str() {
        "functional" => Ok(RequirementType::Functional),
        "non-functional" | "nonfunctional" => Ok(RequirementType::NonFunctional),
        "system" => Ok(RequirementType::System),
        "user" => Ok(RequirementType::User),
        "change-request" | "changerequest" | "cr" => Ok(RequirementType::ChangeRequest),
        "bug" => Ok(RequirementType::Bug),
        "epic" => Ok(RequirementType::Epic),
        "story" => Ok(RequirementType::Story),
        "task" => Ok(RequirementType::Task),
        "spike" => Ok(RequirementType::Spike),
        "sprint" => Ok(RequirementType::Sprint),
        "folder" => Ok(RequirementType::Folder),
        "meta" => Ok(RequirementType::Meta),
        // Docs-layer types (FR-1-074). trace:FR-1-074 | ai:claude
        "principle" | "prin" => Ok(RequirementType::Principle),
        "vision" | "vis" => Ok(RequirementType::Vision),
        "constraint" | "con" => Ok(RequirementType::Constraint),
        "decision" | "adr" => Ok(RequirementType::Decision),
        "term" | "glossary" => Ok(RequirementType::Term),
        // trace:STORY-104 | ai:claude
        "doc" | "documentation" => Ok(RequirementType::Doc),
        "faq" => Ok(RequirementType::Faq),
        _ => anyhow::bail!("Invalid requirement type: {}", type_str),
    }
}

/// Truncate a string for table display, with an ellipsis when shortened.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let clipped: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", clipped)
    }
}

// trace:BUG-532 trace:BUG-535 — render a spec's (truncated) title as a dimmed
// trailing cell beside its id, so id-list views (`burndown plan`, `aida human`)
// read as scannable decision surfaces, mirroring how `aida list` / `aida queue
// list` show id+title (60-col truncation via the shared `truncate` helper).
// Returns the leading-space dimmed title cell, or an empty string when a title
// is missing/empty so the id still prints cleanly. Shared so the two id-list
// surfaces can never drift on how a title is rendered.
pub(crate) fn spec_title_cell(
    titles: &std::collections::HashMap<String, String>,
    id: &str,
) -> String {
    match titles.get(id) {
        Some(t) if !t.is_empty() => format!("  {}", truncate(t, 60).dimmed()),
        _ => String::new(),
    }
}

// Compact inline rendering for the `aida list --show-tags` column: show
// the first `max_chips` tags joined by ", ", append " +N more" when the
// row carries additional tags, return empty string for a tagless row.
// trace:TASK-569 | ai:claude
pub(crate) fn format_tags_inline(tags: &[String], max_chips: usize) -> String {
    if tags.is_empty() || max_chips == 0 {
        return String::new();
    }
    let shown = tags
        .iter()
        .take(max_chips)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let extra = tags.len().saturating_sub(max_chips);
    if extra > 0 {
        format!("{shown} +{extra} more")
    } else {
        shown
    }
}

#[cfg(test)]
#[path = "tests/format_tags_inline_tests.rs"]
mod format_tags_inline_tests;

/// A single grep match result
pub(crate) struct GrepMatch {
    pub(crate) field: String,
    pub(crate) line_num: Option<usize>,
    pub(crate) line: String,
    pub(crate) match_start: usize,
    pub(crate) match_end: usize,
    pub(crate) context_before: Vec<String>,
    pub(crate) context_after: Vec<String>,
}

/// Search a single-line field for matches
pub(crate) fn search_field(
    regex: &regex::Regex,
    field: &str,
    content: &str,
    _ctx_before: usize,
    _ctx_after: usize,
) -> Option<GrepMatch> {
    regex.find(content).map(|m| GrepMatch {
        field: field.to_string(),
        line_num: None,
        line: content.to_string(),
        match_start: m.start(),
        match_end: m.end(),
        context_before: vec![],
        context_after: vec![],
    })
}

/// Search a multiline field for matches
pub(crate) fn search_multiline_field(
    regex: &regex::Regex,
    field: &str,
    content: &str,
    ctx_before: usize,
    ctx_after: usize,
) -> Vec<GrepMatch> {
    let lines: Vec<&str> = content.lines().collect();
    let mut matches = Vec::new();

    for (line_idx, line) in lines.iter().enumerate() {
        if let Some(m) = regex.find(line) {
            // Gather context before
            let start = line_idx.saturating_sub(ctx_before);
            let context_before: Vec<String> = lines[start..line_idx]
                .iter()
                .map(|s| s.to_string())
                .collect();

            // Gather context after
            let end = (line_idx + 1 + ctx_after).min(lines.len());
            let context_after: Vec<String> = lines[line_idx + 1..end]
                .iter()
                .map(|s| s.to_string())
                .collect();

            matches.push(GrepMatch {
                field: field.to_string(),
                line_num: Some(line_idx + 1),
                line: line.to_string(),
                match_start: m.start(),
                match_end: m.end(),
                context_before,
                context_after,
            });
        }
    }

    matches
}

/// Print a grep match with highlighting
pub(crate) fn print_grep_match(m: &GrepMatch) {
    let field_display = if let Some(line_num) = m.line_num {
        format!("[{}:{}]", m.field, line_num)
    } else {
        format!("[{}]", m.field)
    };

    // Print context before
    for ctx_line in &m.context_before {
        println!("  {} {}", field_display.dimmed(), ctx_line.dimmed());
    }

    // Print the matching line with highlighted match
    let before_match = &m.line[..m.match_start];
    let match_text = &m.line[m.match_start..m.match_end];
    let after_match = &m.line[m.match_end..];

    println!(
        "  {} {}{}{}",
        field_display.blue(),
        before_match,
        match_text.red().bold(),
        after_match
    );

    // Print context after
    for ctx_line in &m.context_after {
        println!("  {} {}", field_display.dimmed(), ctx_line.dimmed());
    }
}
