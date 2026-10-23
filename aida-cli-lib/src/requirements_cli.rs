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
    edit_requirement_cli_with(
        storage,
        id_str,
        &EditFields {
            title,
            description,
            status,
            priority,
            req_type,
            owner,
            feature,
            tags,
            add_tag,
            remove_tag,
        },
        DeferredStatus::Refuse,
    )
}

/// The flag-driven fields of one [`edit_requirement_cli`] call.
pub(crate) struct EditFields<'a> {
    pub(crate) title: &'a Option<String>,
    pub(crate) description: &'a Option<String>,
    pub(crate) status: &'a Option<String>,
    pub(crate) priority: &'a Option<String>,
    pub(crate) req_type: &'a Option<String>,
    pub(crate) owner: &'a Option<String>,
    pub(crate) feature: &'a Option<String>,
    pub(crate) tags: &'a Option<String>,
    pub(crate) add_tag: &'a [String],
    pub(crate) remove_tag: &'a [String],
}

/// What a status edit does to a deferred spec.
// trace:TASK-1955 | ai:claude (slice 7 round-1 finding 5)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeferredStatus {
    /// A caller's edit: refused (it would change nothing anyone sees).
    Refuse,
    /// An autopilot reversal restoring the stored status it overwrote: it
    /// writes `stored_status`, which reads again once the spec is undeferred.
    Restore,
}

pub(crate) fn edit_requirement_cli_with(
    storage: &Storage,
    id_str: &str,
    fields: &EditFields<'_>,
    deferred_status: DeferredStatus,
) -> Result<()> {
    let EditFields {
        title,
        description,
        status,
        priority,
        req_type,
        owner,
        feature,
        tags,
        add_tag,
        remove_tag,
    } = *fields;
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
        // trace:TASK-1955 | ai:claude
        if deferred_status == DeferredStatus::Refuse {
            if let Some(msg) = deferred_status_edit_refusal(req) {
                anyhow::bail!(msg);
            }
        }
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
            // trace:TASK-1955 | ai:claude (slice 7 round-1 finding 5)
            if let Some(msg) = deferred_status_edit_refusal(req) {
                anyhow::bail!(msg);
            }
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

/// Add a new requirement
#[derive(clap::Parser, Debug)]
#[clap(name = "add")]
pub(crate) struct AddArgs {
    /// Title, positional — the newcomer-friendly form: `aida add "Add a task
    /// from the CLI"`. Same as `--title`; if both are given, `--title` wins.
    // trace:TASK-725 | ai:claude
    pub title_positional: Option<String>,

    /// Title of the requirement
    // trace:BUG-1294 | ai:claude
    #[clap(long, allow_hyphen_values = true)]
    pub title: Option<String>,

    /// Description of the requirement
    // trace:BUG-1294 | ai:claude
    #[clap(long, allow_hyphen_values = true)]
    pub description: Option<String>,

    /// Read the description body from a file. Mutually exclusive with
    /// --description and --description-stdin.
    // trace:BUG-17 | ai:claude
    #[clap(long, conflicts_with_all = ["description", "description_stdin"])]
    pub description_from_file: Option<std::path::PathBuf>,

    /// Read the description body from stdin. Mutually exclusive with
    /// --description and --description-from-file.
    // trace:BUG-17 | ai:claude
    #[clap(long, conflicts_with_all = ["description", "description_from_file"])]
    pub description_stdin: bool,

    /// Status of the requirement (draft, approved, planned, in-progress,
    /// done, completed, rejected)
    #[clap(long)]
    pub status: Option<String>,

    /// Priority of the requirement (high, medium, low)
    #[clap(long)]
    pub priority: Option<String>,

    /// Type of requirement: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc, faq. An faq is lightweight: filed as completed by default.
    // trace:STORY-1642 | ai:claude
    #[clap(long)]
    pub r#type: Option<String>,

    /// Owner of the requirement (defaults to AIDA_AUTHOR env var or system user)
    // trace:BUG-1294 | ai:claude
    #[clap(long, allow_hyphen_values = true)]
    pub owner: Option<String>,

    /// Feature the requirement belongs to (defaults to REQ_FEATURE env var or "Uncategorized")
    // trace:BUG-1294 | ai:claude
    #[clap(long, allow_hyphen_values = true)]
    pub feature: Option<String>,

    /// Tags for the requirement (comma-separated). A tag may not contain
    /// whitespace: `--tags "a b"` is refused, `--tags a,b` is two tags.
    #[clap(long)]
    pub tags: Option<String>,

    /// Custom ID prefix override (uppercase letters only, e.g., SEC, PERF)
    #[clap(long)]
    pub prefix: Option<String>,

    /// Parent requirement ID (UUID or SPEC-ID) to link as child
    #[clap(long)]
    pub parent: Option<String>,

    // trace:STORY-446 | ai:claude
    /// SPEC-ID this new requirement is blocked by (repeatable; alias
    /// `--depends-on`). Creates the BlockedBy edge + the inverse Blocks
    /// edge atomically, so the pickability gate holds pickup until the
    /// blocker is Completed.
    #[clap(
        long = "blocked-by",
        visible_alias = "depends-on",
        value_name = "SPEC-ID"
    )]
    pub blocked_by: Vec<String>,

    /// Override the guard that refuses `--parent <X>` when X is in
    /// a terminal status (Completed/Rejected). Use when intentionally
    /// backfilling a forgotten child onto a closed epic for
    /// traceability.
    // trace:BUG-64 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(long)]
    pub force_parent: bool,

    /// Use interactive mode (prompts)
    #[clap(long)]
    pub interactive: bool,

    /// Mark this spec human-only so the orchestrator never auto-picks it.
    /// Spikes default to human-only automatically (research is human-driven);
    /// pass this on a non-Spike to opt in explicitly.
    // trace:TASK-130 | ai:claude
    #[clap(long, conflicts_with = "no_human_only")]
    pub human_only: bool,

    /// Force this spec to NOT be human-only even when its type would default
    /// to it (use on a Spike that is genuinely heads-down implementation).
    // trace:TASK-130 | ai:claude
    #[clap(long, conflicts_with = "human_only")]
    pub no_human_only: bool,

    /// Open-time effort estimate: 15m, 1h, 4h, 1d, or 1w. Captured
    /// to `.aida/effort-calibration/<SPEC>.yaml` and stamped as an
    /// `effort:open:<value>` tag. `1d` means 8 work-hours; `1w`
    /// means 5 work-days / 40 work-hours.
    // trace:STORY-451 | ai:codex
    #[clap(long, value_enum, value_name = "BUCKET")]
    pub effort: Option<crate::effort_calibration::EffortBucket>,

    /// Optional numeric weight/score for this spec (any finite number,
    /// e.g. 0.75 or 42) — a continuous ranking signal beyond the
    /// high/medium/low priority enum. Sortable (`aida list --sort weight`)
    /// and filterable (`--min-weight` / `--max-weight`).
    // trace:FR-283 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(long, value_name = "N", allow_hyphen_values = true)]
    pub weight: Option<f32>,

    /// Set the execution mode for the new requirement (drain, drive,
    /// guided, operator, or decide). Advisor-authority write.
    // trace:TASK-1267 | ai:codex
    #[clap(long = "mode", value_name = "MODE")]
    pub mode: Option<String>,

    /// File, approve, AND enqueue in one shot — places the new spec on the
    /// queue right after creating it, equivalent to a follow-up `aida
    /// backlog groom`. Only an Approved spec is enqueueable: pass with
    /// `--status approved` (the new spec must end up Approved, not draft).
    /// Refused the same way `aida queue add` is when the session lacks
    /// authority to commit work to the pipeline.
    // trace:TASK-754 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(long)]
    pub queue: bool,

    /// When enqueuing with `--queue`, tag the queued spec `batch:NAME` so
    /// it composes with `aida queue work --batch NAME`. No effect without
    /// `--queue`.
    // trace:TASK-754 | ai:claude
    #[clap(long, value_name = "NAME")]
    pub batch: Option<String>,

    /// When enqueuing with `--queue`, route the spec to ROLE's queue
    /// (mirrors `aida queue add --for`). Defaults to `implementer` — the
    /// common case is an advisor filing implementation work — rather than
    /// the filer's own session role. Pass `--for advisor` to keep it on the
    /// advisor queue, or `--for any` to leave it unrouted. No effect without
    /// `--queue`.
    // trace:BUG-528 | ai:claude
    #[clap(long = "for", value_name = "ROLE")]
    pub r#for: Option<String>,

    /// Run named gates from the shipped library over the new spec's text
    /// before it is filed (comma-separated, e.g. `--gates well-formed`).
    /// Advisory: findings are printed, the spec is filed regardless.
    /// Without this flag `aida add` is unchanged. See `aida gate list`.
    // trace:STORY-1427 | ai:claude
    #[clap(long, value_name = "NAMES", value_delimiter = ',')]
    pub gates: Vec<String>,
}

/// The `aida add` args the handler files: decoded, with requester intake
/// coerced (Draft, default type, `intake:requester` tag) when that
/// stakeholder role is active.
// trace:STORY-1728 | ai:claude
pub(crate) fn add_args_for_dispatch(matches: &clap::ArgMatches) -> anyhow::Result<AddArgs> {
    let mut args = <AddArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    crate::stakeholder_role::apply_requester_intake(&mut args);
    Ok(args)
}

fn run_add(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = self::add_args_for_dispatch(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                AddArgs {
                    title,
                    title_positional,
                    description,
                    description_from_file,
                    description_stdin,
                    status,
                    priority,
                    r#type,
                    owner,
                    tags,
                    prefix,
                    parent,
                    blocked_by,
                    force_parent,
                    interactive,
                    effort,
                    human_only,
                    no_human_only,
                    queue,
                    batch,
                    r#for,
                    // trace:FR-283 | ai:claude
                    weight,
                    mode,
                    gates,
                    ..
                } => {
                    // TASK-725: newcomer-friendly capture — `aida add "do X"`. The
                    // positional title is equivalent to --title; --title wins if both
                    // are given. Without this, the init greeting's own first suggestion
                    // (`aida add "Add a task from the CLI"`) errors — caught by running
                    // the novice's first session. trace:TASK-725 | ai:claude
                    let title = title.clone().or_else(|| title_positional.clone());
                    let title = &title;
                    // BUG-45 + interactive expansion: when the user doesn't pass
                    // --title, decide whether to prompt or bail. When prompting,
                    // also walk through type / description / priority for any
                    // field the user didn't already supply via flags. Title is
                    // always required; the rest skip cleanly when their flag is
                    // present. trace:BUG-45 | ai:claude
                    let interactive_mode = *interactive
                        || (title.is_none() && std::io::IsTerminal::is_terminal(&std::io::stdin()));

                    // Title — required. Either --title, an interactive prompt, or
                    // bail with help.
                    let title_resolved: String = if let Some(t) = title.clone() {
                        t
                    } else if interactive_mode {
                        let answer = inquire::Text::new("Title:")
                            .with_help_message("Required. One sentence describing what this is.")
                            .prompt()
                            .context("Title prompt cancelled")?;
                        let t = answer.trim().to_string();
                        if t.is_empty() {
                            anyhow::bail!("title is required (got empty input)");
                        }
                        t
                    } else {
                        anyhow::bail!(
                        "title is required — pass `--title \"...\"` or run with `--interactive` \
                         for prompts. (See `aida add --help` for the full flag list.)"
                    );
                    };
                    // TASK-888: reject an empty / whitespace-only title — a titleless
                    // spec is never useful, and the silent accept ("Added: TASK-N - ")
                    // hid the mistake. Trim is display-only here; the title is stored
                    // verbatim below. trace:TASK-888 | ai:claude
                    if title_resolved.trim().is_empty() {
                        anyhow::bail!(
                            "title cannot be empty — pass a non-blank `--title \"...\"`."
                        );
                    }
                    // TASK-888: cap the title at a sane length. A 5000-char title was
                    // accepted unbounded; truncate (preserving char boundaries) and warn
                    // rather than hard-fail, so a paste accident still files something
                    // legible. trace:TASK-888 | ai:claude
                    const MAX_TITLE_LEN: usize = 200;
                    let title_resolved: String = if title_resolved.chars().count() > MAX_TITLE_LEN {
                        let truncated: String =
                            title_resolved.chars().take(MAX_TITLE_LEN).collect();
                        eprintln!(
                        "{} title exceeded {} characters — truncated. Put the detail in the description instead.",
                        "Warning:".yellow().bold(),
                        MAX_TITLE_LEN,
                    );
                        truncated
                    } else {
                        title_resolved
                    };
                    // trace:BUG-22 | ai:claude
                    if let Some(msg) = suspicious_title_signal(&title_resolved) {
                        eprintln!("{} {}", "Warning:".yellow().bold(), msg);
                    }

                    // Type — interactive picker when not provided. TASK-728: lead with
                    // the relatable everyday types a newcomer reaches for (task is the
                    // default + first); push the specialized docs-layer / organizational
                    // types to the end so the picker doesn't open with jargon.
                    // trace:TASK-728 | ai:claude
                    let interactive_type: Option<String> = if r#type.is_none() && interactive_mode {
                        let choices = vec![
                            "task",
                            "bug",
                            "story",
                            "epic",
                            "functional",
                            "spike",
                            "non-functional",
                            "system",
                            "user",
                            "principle",
                            "vision",
                            "decision",
                            "constraint",
                            "term",
                            // trace:STORY-1642 | ai:claude
                            "faq",
                            "sprint",
                            "folder",
                            "meta",
                        ];
                        let pick = inquire::Select::new("Type:", choices)
                        .with_help_message("task for most things · bug for defects · story/epic to group work. The rest are for larger or docs-heavy projects.")
                        .prompt()
                        .context("Type prompt cancelled")?;
                        Some(pick.to_string())
                    } else {
                        None
                    };
                    // TASK-725: a newcomer who types `aida add "do X"` means a *task*,
                    // not a "Functional Requirement" — default to the relatable,
                    // catch-all type when none is given (explicit --type and the
                    // interactive picker still win). trace:TASK-725 | ai:claude
                    let effective_type: Option<String> = r#type
                        .clone()
                        .or(interactive_type)
                        .or_else(|| Some("task".to_string()));
                    // STORY-1642 item 4: an FAQ is lightweight — no lifecycle ceremony.
                    // It is filed `completed` (an answered question), skips the
                    // priority prompt (priority is meaningless for an FAQ), is exempt
                    // from the approved+ advisor intake gate (knowledge is never
                    // pickable work, so there is nothing to gate), and its add footer
                    // carries no trace-comment breadcrumb. trace:STORY-1642 | ai:claude
                    let is_faq =
                        crate::knowledge_list::type_filter_is_faq(effective_type.as_deref());

                    // Description — open the user's $EDITOR when not provided.
                    // trace:BUG-17 | ai:claude
                    let resolved_description = if interactive_mode
                        && description.is_none()
                        && description_from_file.is_none()
                        && !*description_stdin
                    {
                        let body = inquire::Editor::new("Description")
                        .with_help_message(
                            "Multi-line. Save + close the editor to continue. Leave empty to skip.",
                        )
                        .prompt()
                        .context("Description prompt cancelled")?;
                        if body.trim().is_empty() {
                            None
                        } else {
                            Some(body)
                        }
                    } else {
                        resolve_description(description, description_from_file, *description_stdin)?
                    };

                    // Priority — picker when not provided.
                    let interactive_priority: Option<String> = if priority.is_none()
                        && interactive_mode
                        && !is_faq
                    {
                        let pick = inquire::Select::new("Priority:", vec!["medium", "high", "low"])
                            .with_help_message(
                                "medium covers most things; high for blockers; low for nice-to-haves.",
                            )
                            .prompt()
                            .context("Priority prompt cancelled")?;
                        Some(pick.to_string())
                    } else {
                        None
                    };
                    let effective_priority: Option<String> =
                        priority.clone().or(interactive_priority);

                    // STORY-1427: `--gates` runs named library gates over the draft text
                    // before filing. Advisory — prints verdicts, never blocks; absent the
                    // flag this is a no-op. trace:STORY-1427 | ai:claude
                    crate::gate_cmd::run_intake_gates(
                        gates,
                        &title_resolved,
                        resolved_description.as_deref().unwrap_or_default(),
                    )?;

                    let mut req =
                        Requirement::new(title_resolved, resolved_description.unwrap_or_default());
                    if let Some(s) = status {
                        let canonical = validate_status_input(s).map_err(|e| anyhow::anyhow!(e))?;
                        // STORY-332: a freshly-added spec cannot be born paused —
                        // NeedsAttention is reached only by punting In-Progress work.
                        if let Some(msg) =
                            forbidden_attention_transition(&req.status, &parse_status(canonical)?)
                        {
                            anyhow::bail!(msg);
                        }
                        req.set_status_from_str(canonical);
                    } else if is_faq {
                        // trace:STORY-1642 | ai:claude
                        req.status = RequirementStatus::Completed;
                    }
                    // A FAQ born Completed carries its completion stamp like any
                    // other Completed record. trace:STORY-1642 | ai:claude
                    let faq_born_completed =
                        is_faq && matches!(req.status, RequirementStatus::Completed);
                    if faq_born_completed {
                        req.implementation_info
                            .get_or_insert_with(aida_core::ImplementationInfo::default)
                            .completed_at
                            .get_or_insert_with(chrono::Utc::now);
                    }
                    // TASK-647 (ADR-3): advisor-gate the production of approved+ specs.
                    // A non-advisor, non-TTY caller (headless agent, drain/auto
                    // capture) can only file `draft`; a requested approved+ status is
                    // downgraded with a one-line triage notice. An interactive human
                    // (TTY) or the advisor role is unaffected. trace:TASK-647 | ai:claude
                    let intake_downgraded_from = if status_requires_advisor_authority(&req.status)
                    && !has_advisor_authority()
                    // trace:STORY-1642 | ai:claude — a Completed FAQ is reference,
                    // never pickable work; any other FAQ status stays gated.
                    && !faq_born_completed
                    {
                        let from = req.status;
                        req.status = RequirementStatus::Draft;
                        Some(from)
                    } else {
                        // BUG-498: a non-downgraded approved+ intake exercised
                        // advisor authority — nudge the operator to seat the
                        // advisor role if it came from an env prefix.
                        if status_requires_advisor_authority(&req.status) && !faq_born_completed {
                            maybe_hint_advisor_seat();
                        }
                        None
                    };
                    if let Some(p) = &effective_priority {
                        let canonical =
                            validate_priority_input(p).map_err(|e| anyhow::anyhow!(e))?;
                        req.set_priority_from_str(canonical);
                    }
                    if let Some(t) = &effective_type {
                        // BUG-48: surface the error instead of dropping silently.
                        let rt = parse_requirement_type(t).map_err(|e| {
                        anyhow::anyhow!(
                            "{} — expected one of: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc, faq",
                            e
                        )
                    })?;
                        // BUG-49: when the new type implies a different prefix
                        // than the existing spec_id, the spec_id does NOT auto-
                        // renumber (it would orphan trace comments, commit
                        // messages, and external references). Warn the user so
                        // they don't expect FR-4 to become PRIN-4.
                        if let Some(ref sid) = req.spec_id {
                            let new_prefix = rt.default_prefix();
                            let current_prefix = sid.split('-').next().unwrap_or("");
                            if !current_prefix.is_empty() && current_prefix != new_prefix {
                                eprintln!(
                                    "{} type changed to `{}` but spec_id stays `{}` — \
                                 AIDA spec_ids are stable so existing trace comments \
                                 and commit refs keep working. To get a `{}-N` id, \
                                 recreate the requirement with the new type.",
                                    "Note:".dimmed(),
                                    t,
                                    sid,
                                    new_prefix
                                );
                            }
                        }
                        req.req_type = rt;
                    }
                    // TASK-130: a Spike is time-boxed research with operator-judgment
                    // decisions (candidate selection, experimental design, rubric
                    // evaluation, report writing) — not heads-down implementation. So
                    // a Spike is born human-only by default (the orchestrator's
                    // pre-pickup gate then skips it instead of an implementer agent
                    // mis-treating research as coding). `--no-human-only` opts a Spike
                    // back into auto-pickup; `--human-only` sets the marker on any
                    // type. Reuses the STORY-333 human_only primitive.
                    // trace:TASK-130 | ai:claude
                    req.human_only = resolve_human_only(&req.req_type, *human_only, *no_human_only);
                    if let Some(o) = owner {
                        req.owner = o.clone();
                    }
                    if let Some(t) = tags {
                        // BUG-1770: one shared parser for every `--tags` write path, so
                        // a whitespace blob cannot be created here and the rule cannot
                        // drift between `add` and `edit`. trace:BUG-1770 | ai:claude
                        for tag in parse_tag_list(t)? {
                            req.tags.insert(tag);
                        }
                    }
                    if let Some(effort) = effort {
                        effort_calibration::apply_effort_tag(
                            &mut req.tags,
                            effort_calibration::EffortTouchpoint::Open,
                            *effort,
                        );
                    }
                    // FR-283: the optional first-class numeric weight/score — a
                    // continuous ranking signal beyond the priority enum.
                    // trace:FR-283 | ai:claude
                    if let Some(w) = weight {
                        if !w.is_finite() {
                            anyhow::bail!("--weight must be a finite number (got {w})");
                        }
                        req.weight = Some(*w);
                    }
                    // A creation-time execution mode is the same advisor routing
                    // judgment as `aida edit --mode`; validate it before persisting.
                    // trace:TASK-1267 | ai:codex
                    if let Some(m) = mode {
                        if !has_advisor_authority() {
                            anyhow::bail!(
                            "--mode sets the advisor's routing classification and needs advisor authority"
                        );
                        }
                        req.execution_mode = Some(
                            m.parse::<aida_core::ExecutionMode>()
                                .map_err(|e: String| anyhow::anyhow!(e))?,
                        );
                    }
                    if let Some(p) = prefix {
                        req.prefix_override = Some(p.to_uppercase());
                    }

                    // Try to assign an agreed ID from a pre-allocated block (FR-2-005).
                    // Behavior is governed by [id_format] policy in .aida/config.toml:
                    //   - node-aware-only:      skip block dispense entirely
                    //   - blocks-then-fallback: try block; fall through silently if missing
                    //   - blocks-only:          require a block; error if missing
                    // trace:EPIC-1-052 Phase 2 | ai:claude

                    // TASK-59: pre-flight validation BEFORE the dispenser
                    // advances. The original flow allocated an agreed_id from
                    // the block, then ran parent / terminal-status / lease
                    // checks — when any of those bailed the counter was
                    // already advanced and committed to blocks.yaml, leaving
                    // a phantom id (BUG-69 etc.) permanently missing from the
                    // sequence. Resolve parent + run all three guards FIRST
                    // so a failed `aida add --parent <Completed-EPIC>` leaves
                    // the dispenser exactly where it was.
                    // trace:TASK-59 | ai:claude
                    let parent_req: Option<aida_core::models::Requirement> =
                        if let Some(parent_str) = parent {
                            let resolved = if let Ok(uuid) = uuid::Uuid::parse_str(parent_str) {
                                backend.get_requirement(&uuid)?
                            } else {
                                backend.get_requirement_unambiguous(parent_str)?
                            };
                            Some(resolved.ok_or_else(|| {
                                anyhow::anyhow!(
                                    "parent `{}` not found — refusing to create child requirement \
                             without a valid parent (no child file written)",
                                    parent_str
                                )
                            })?)
                        } else {
                            None
                        };
                    if let Some(ref pr) = parent_req {
                        if !*force_parent && is_terminal_status(&pr.status) {
                            anyhow::bail!(
                            "parent {} is {} — adding new children to a closed parent is usually a mistake. \
                             Pass `--force-parent` to override, or pick a different parent \
                             (try `aida list --type epic --status approved`).",
                            pr.spec_id.as_deref().unwrap_or("?"),
                            pr.status,
                        );
                        }
                        if let Ok(project_root) = find_project_root() {
                            if !list_leases(&project_root).is_empty() {
                                let store = backend.load()?;
                                enforce_session_lease(
                                    &project_root,
                                    pr,
                                    &store,
                                    "aida add --parent",
                                    false,
                                    // BUG-637: `aida add --parent` has no --force override; never skip.
                                    false,
                                )?;
                            }
                        }
                    }

                    let project_dir = std::env::current_dir().unwrap_or_default();
                    // BUG-372: before allocating a human-readable global SPEC-ID,
                    // refresh the git-canonical store and refuse known duplicate
                    // spec_ids. TASK-281 protects block refill commits, but the
                    // creation path also has to start from fresh store state.
                    pull_store_before_id_allocation(store_path, &project_dir)?;
                    let id_policy = read_id_format_policy(&project_dir);
                    if id_policy.uses_blocks() {
                        let node_id = load_node_id(store_path);
                        let blocks_path = store_path.join("registry").join("blocks.yaml");
                        if !blocks_path.exists() && id_policy.requires_block() {
                            anyhow::bail!(
                                "id_format policy is `blocks-only` but no blocks.yaml exists. \
                             Run `aida db block claim --type FR --size 100` to allocate a block."
                            );
                        }
                        if blocks_path.exists() {
                            // Determine type prefix. Use the canonical
                            // RequirementType::default_prefix() so every type is
                            // covered (no silent fallback to "FR" for new types).
                            // trace:FR-1-074 | ai:claude
                            let type_prefix = match &req.prefix_override {
                                Some(p) => p.clone(),
                                None => req.req_type.default_prefix().to_string(),
                            };

                            // TASK-281: auto-claim a fresh block before dispensing when
                            // aggregate remaining drops below the configured threshold.
                            // Skips when auto-claim is disabled and when we're still
                            // above threshold. With no block yet for a block-allocated
                            // type it claims the node's first one (BUG-1921: nodes
                            // acquired before FAQ/DOC/CR were block-allocated); that
                            // claim is best-effort and never errors. BUG-372 makes a
                            // refill push failure fatal for add: continuing with a
                            // stale local block can recreate the cross-clone
                            // duplicate SPEC-ID race.
                            // trace:TASK-281 BUG-372 | ai:claude
                            match ensure_block_capacity(
                                store_path,
                                &project_dir,
                                &node_id,
                                &type_prefix,
                            ) {
                                // trace:BUG-1921 | ai:claude
                                Ok(Some(outcome)) if outcome.bootstrap => {
                                    eprintln!(
                                        "{} Claimed {} (first {} id block for this clone)",
                                        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                                        outcome.label,
                                        type_prefix.to_uppercase(),
                                    );
                                }
                                Ok(Some(outcome)) => {
                                    eprintln!(
                                        "{} Auto-claimed {} (threshold crossed: {} remaining → {})",
                                        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                                        outcome.label,
                                        outcome.previous_remaining,
                                        outcome.new_remaining,
                                    );
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    anyhow::bail!(
                                        "auto-claim failed before dispensing a new {} id: {}\n\
                                     Refusing to continue with a potentially stale local block. \
                                     Run `aida db sync --pull` and retry.",
                                        type_prefix,
                                        e
                                    );
                                }
                            }

                            // BUG-474: the agreed-id dispense below is a
                            // read-modify-write (load → dispense → save). Two
                            // concurrent `aida add` runs would both load `next = N`,
                            // both dispense `<TYPE>-N`, and both save → a duplicate
                            // stable id. Serialize the whole sequence under an
                            // exclusive advisory lock on the blocks file, mirroring
                            // the FileDispenser pattern (TASK-331). The in-loop
                            // collision-skip stays; the lock is the real fix.
                            // trace:BUG-474 | ai:claude
                            // TASK-1930 (A1): acquire store_write_lock BEFORE blocks.lock
                            // (dispense_lock) so lock ordering is always
                            // store_write_lock -> dispense_lock across all writers and
                            // `git_ops::add(store_path, &["registry/blocks.yaml"])`
                            // runs under the store write lock.
                            // trace:TASK-1930 | ai:agy
                            aida_core::db::with_store_write_lock(store_path, || {
                                aida_core::BlockRegistry::with_dispense_lock(&blocks_path, || {
                                    if let Ok(mut registry) =
                                        aida_core::BlockRegistry::load(&blocks_path)
                                    {
                                        match registry
                                            .find_active_block_or_global(&node_id, &type_prefix)
                                        {
                                            None => {
                                                // No block for this type. Under `blocks-only`
                                                // this is fatal — the project requires every
                                                // id come from an allocated block. Otherwise
                                                // fall through to a node-aware id silently.
                                                if id_policy.requires_block() {
                                                    anyhow::bail!(
                                                        "id_format policy is `blocks-only` but node {} has no \
                                                     {} block. Run `aida db block claim --type {} --size 100` \
                                                     to allocate one (requires network).",
                                                        node_id,
                                                        type_prefix,
                                                        type_prefix
                                                    );
                                                }
                                                // TASK-36: if the only thing missing is an
                                                // *active* block (exhausted blocks exist for
                                                // this type), the user just rolled off the
                                                // last short id and we're silently switching
                                                // to the node-aware format. Surface that with
                                                // the highest issued short id and a pointer
                                                // to merge-gate. trace:TASK-36 | ai:claude
                                                let prefix_upper = type_prefix.to_uppercase();
                                                let last_short = registry
                                                    .blocks
                                                    .iter()
                                                    .filter(|b| {
                                                        b.type_prefix.to_uppercase() == prefix_upper
                                                    })
                                                    .map(|b| b.range_end)
                                                    .max();
                                                if let Some(last) = last_short {
                                                    eprintln!(
                                                        "{} {} agreed-id block exhausted (last short id: {}-{}).",
                                                        crate::glyph(crate::glyphs::Glyph::Warning)
                                                            .yellow()
                                                            .bold(),
                                                        prefix_upper,
                                                        prefix_upper,
                                                        last
                                                    );
                                                    eprintln!(
                                                        "  {} Falling back to node-aware ids ({}-NODE-NN). Run \
                                                         `aida db block claim --type {} --size 100` to allocate \
                                                         a fresh block.",
                                                        "→".dimmed(),
                                                        prefix_upper,
                                                        prefix_upper
                                                    );
                                                }
                                            }
                                            Some(idx) if registry.blocks[idx].is_exhausted() => {
                                                anyhow::bail!(
                                                    "Agreed ID block for {} exhausted on node {}. \
                                                     Run `aida db block claim --type {}` to allocate a new block (requires network).",
                                                    type_prefix,
                                                    node_id,
                                                    type_prefix
                                                );
                                            }
                                            Some(_) => {
                                                // BUG-31: dispense in a loop, skipping any
                                                // candidate id that's already taken in the
                                                // store. Existing reqs (e.g. older sessions
                                                // before this block was claimed, retired-
                                                // legacy-id migrations, imports) can occupy
                                                // ids inside our block range. Block.next is
                                                // monotonic so we just keep advancing until
                                                // we land on a free slot or exhaust.
                                                let mut dispensed: Option<(String, bool)> = None;
                                                let mut skipped: Vec<String> = Vec::new();
                                                while let Some((candidate, is_low)) =
                                                    registry.dispense(&node_id, &type_prefix)
                                                {
                                                    let taken = backend
                                                        .get_requirement_by_spec_id(&candidate)
                                                        .map(|opt| opt.is_some())
                                                        .unwrap_or(false);
                                                    if !taken {
                                                        dispensed = Some((candidate, is_low));
                                                        break;
                                                    }
                                                    skipped.push(candidate);
                                                }
                                                if !skipped.is_empty() {
                                                    eprintln!(
                                                        "{} skipped {} already-taken id(s) in {} block: {}",
                                                        "Note:".dimmed(),
                                                        skipped.len(),
                                                        type_prefix,
                                                        skipped.join(", ")
                                                    );
                                                }
                                                if let Some((agreed_id, _is_low_per_block)) =
                                                    dispensed
                                                {
                                                    // BUG-115: warn on the aggregate remaining
                                                    // across all of this node's non-exhausted
                                                    // blocks for the type. The pre-fix per-
                                                    // block check fired on every `aida add`
                                                    // when the lowest-numbered block was near
                                                    // empty, even though a higher block with
                                                    // full capacity had already been claimed.
                                                    // trace:BUG-115 | ai:claude
                                                    if registry
                                                        .aggregate_is_low(&node_id, &type_prefix)
                                                    {
                                                        let aggregate = registry
                                                            .aggregate_remaining(
                                                                &node_id,
                                                                &type_prefix,
                                                            );
                                                        let active = registry.active_block_count(
                                                            &node_id,
                                                            &type_prefix,
                                                        );
                                                        let span = if active == 1 {
                                                            "in 1 block".to_string()
                                                        } else {
                                                            format!("across {} blocks", active)
                                                        };
                                                        eprintln!(
                                                            "{} {} block running low ({} remaining {}). Run `aida db block claim --type {}` soon.",
                                                            "WARNING:".yellow().bold(),
                                                            type_prefix,
                                                            aggregate,
                                                            span,
                                                            type_prefix
                                                        );
                                                    }
                                                    // Persist the updated next pointer
                                                    if let Err(e) = registry.save(&blocks_path) {
                                                        eprintln!(
                                                            "Warning: could not save blocks.yaml: {}",
                                                            e
                                                        );
                                                    } else if !aida_core::git_ops::store_commit_deferred_to_host(
                                                        store_path,
                                                    ) {
                                                        // Commit the pointer advance to the store
                                                        let _ = aida_core::git_ops::add(
                                                            store_path,
                                                            &["registry/blocks.yaml"],
                                                        );
                                                    }
                                                    req.agreed_id = Some(agreed_id.clone());
                                                    // Use the agreed ID as the spec_id so it is immediately
                                                    // visible as the primary identifier.
                                                    req.spec_id = Some(agreed_id);
                                                }
                                            }
                                        }
                                    }
                                    Ok(())
                                })
                            })?;
                        }
                    }

                    // (parent validation moved BEFORE the dispenser allocation
                    // above for TASK-59 — preserves BUG-62, BUG-64, STORY-48
                    // guards while preventing phantom-id leaks.)

                    // TASK-856: file the new spec through the single-row write path
                    // rather than `update_atomically`. The old path did a FULL-STORE
                    // save on every add — `git add objects` + commit over the entire
                    // object tree (~2.3k files in this repo), a serialize-read-compare
                    // of every requirement, AND a full cache rebuild (DELETE + reinsert
                    // every cache row) — making `aida add` scale O(store size) when the
                    // work is O(1): exactly one new object. `backend.add_requirement`
                    // assigns the spec_id with the SAME store-configured strategy
                    // (`add_requirement_with_id`, via GitBackend::add_requirement),
                    // writes ONLY the new object, does a TARGETED commit (new YAML +
                    // metadata.yaml), and an incremental cache upsert — git-canonical
                    // write + cache write-through stay correct. Measured: full-scale
                    // store add dropped from ~1.5s to ~0.4s for this step (≈1.8x
                    // faster end-to-end median at full scale). This also removes the
                    // redundant second `write_object` (GitBackend::add_requirement
                    // already writes it). trace:TASK-856 | ai:claude
                    let added = backend.add_requirement(req.clone())?;

                    {
                        let last = &added;
                        println!(
                            "Added: {} - {}",
                            last.spec_id.as_deref().unwrap_or("?"),
                            last.title
                        );
                        // TASK-647 (ADR-3): note that a requested approved+ status was
                        // held for advisor triage. The spec is filed (as draft); this
                        // is informational, not an error. trace:TASK-647 | ai:claude
                        if let Some(from) = &intake_downgraded_from {
                            eprintln!(
                            "{} filed as {} (requested {} needs advisor authority) — queued for advisor triage.",
                            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                            "draft".yellow(),
                            from.to_string().to_lowercase().dimmed()
                        );
                        }
                        if let Some(sid) = last.spec_id.as_deref() {
                            record_role_activity(sid, "add");
                        }

                        // STORY-446: apply any --blocked-by / --depends-on edges now
                        // that the spec exists, atomically with the inverse Blocks
                        // edge so the pickability gate holds pickup until the blocker
                        // is Completed. trace:STORY-446 | ai:claude
                        if !blocked_by.is_empty() {
                            if let Some(sid) = last.spec_id.as_deref() {
                                for blocker in blocked_by {
                                    match add_blocked_by_edge(backend, sid, blocker) {
                                        Ok(disp) => println!("  Blocked by: {}", disp.cyan()),
                                        Err(e) => eprintln!(
                                            "  {} could not add blocked-by {}: {}",
                                            "Warning:".yellow().bold(),
                                            blocker,
                                            e
                                        ),
                                    }
                                }
                            }
                        }

                        // BUG-58: when adding from inside a session worktree
                        // WITHOUT --parent, hint that the session's scope is
                        // probably the right parent. Surfaced after the add line
                        // (rather than rejecting the call) so muscle-memory
                        // workflows don't break — the user can ignore it for
                        // off-topic reqs. Only fires when:
                        //   - the lease scope looks like a spec id (so we can
                        //     suggest a concrete `--parent`)
                        //   - the parent actually exists in the store
                        //   - the new req isn't already that scope's spec
                        // trace:BUG-58 | ai:claude
                        if parent.is_none() {
                            if let (Ok(project_root), Ok(cwd)) =
                                (find_project_root(), std::env::current_dir())
                            {
                                // BUG-416: the "this session owns scope X" hint is only
                                // trustworthy when the current process is the SOLE live
                                // agent in this worktree. When two `aida agent new`
                                // sessions share a worktree, active_lease_for_cwd may
                                // resolve a PEER agent's lease, so the hint would
                                // misattribute its scope (the quizdom bleed). Stay
                                // silent then. A human session (0 registered agents) or
                                // a lone agent (1) hints exactly as before.
                                // trace:BUG-416 | ai:claude
                                let shared_worktree =
                                    agent_registry::live_agents_covering_cwd(&project_root, &cwd)
                                        > 1;
                                if let Some(lease) = active_lease_for_cwd(&project_root, &cwd)
                                    .filter(|_| !shared_worktree)
                                {
                                    let scope = lease.scope.clone();
                                    // Cheap heuristic for "looks like a spec id":
                                    // PREFIX-N format. Free-form scopes like
                                    // `feature:auth` or path globs don't match
                                    // and we stay quiet.
                                    if looks_like_spec_id(&scope)
                                        && backend
                                            .get_requirement_by_spec_id(&scope)
                                            .ok()
                                            .flatten()
                                            .is_some()
                                        && last.spec_id.as_deref() != Some(scope.as_str())
                                    {
                                        eprintln!(
                                        "{} this session owns scope {}. \
                                         If {} should be a child of it, link it now: \
                                         `aida rel add --from {} --to {} --type child --bidirectional` \
                                         (or pass `--parent {}` next time).",
                                        "Hint:".dimmed(),
                                        scope.cyan(),
                                        last.spec_id.as_deref().unwrap_or("?").cyan(),
                                        last.spec_id.as_deref().unwrap_or("?"),
                                        scope,
                                        scope,
                                    );
                                    }
                                }
                            }
                        }

                        // FR-215: --parent <id> establishes a parent relationship
                        // in the same shot. Parent was pre-resolved + lease-checked
                        // above (BUG-62) so by this point we know the link is
                        // valid; here we just append the bidirectional edges.
                        // trace:FR-215, BUG-62 | ai:claude
                        if let Some(parent_req) = parent_req {
                            use aida_core::models::{Relationship, RelationshipType};
                            // BUG-615: re-LOAD the freshly-saved child rather than
                            // reusing the `last` snapshot captured at add_requirement
                            // time. The --blocked-by loop above may have already
                            // persisted a BlockedBy edge via add_blocked_by_edge; the
                            // stale `last.clone()` would not carry that edge, so saving
                            // it here clobbered the just-written blocked-by data. Read
                            // the current state (which includes any blocked-by edges),
                            // append the Child edge to THAT, and save once — so both
                            // --parent and --blocked-by survive when combined.
                            // trace:BUG-615 | ai:claude
                            let mut child = last
                                .spec_id
                                .as_deref()
                                .and_then(|sid| {
                                    backend.get_requirement_by_spec_id(sid).ok().flatten()
                                })
                                .unwrap_or_else(|| last.clone());
                            let parent_uuid = parent_req.id;
                            let child_uuid = child.id;
                            // RelationshipType is "I am X to target", so on the
                            // new (child) req we record `Child` pointing at the
                            // parent's uuid, and on the parent we record `Parent`
                            // pointing at the child's uuid. trace:FR-215
                            let now = chrono::Utc::now();
                            child.relationships.push(Relationship {
                                target_id: parent_uuid,
                                rel_type: RelationshipType::Child,
                                created_at: Some(now),
                                created_by: None,
                            });
                            backend.update_requirement(&child)?;
                            let mut parent_mut = parent_req.clone();
                            parent_mut.relationships.push(Relationship {
                                target_id: child_uuid,
                                rel_type: RelationshipType::Parent,
                                created_at: Some(now),
                                created_by: None,
                            });
                            backend.update_requirement(&parent_mut)?;
                            println!(
                                "  Linked: {} → parent of {}",
                                parent_req.spec_id.as_deref().unwrap_or("?"),
                                last.spec_id.as_deref().unwrap_or("?")
                            );
                        }

                        // TASK-928 (SPIKE-71): when no explicit `--parent` was given,
                        // a `parent:<SPEC-ID>` tag must still materialize the real
                        // bidirectional edge, or the spec is orphaned from the graph
                        // (`aida graph tree` / TUI focus-lens blind). Additive +
                        // lenient: the tag stays, an unresolvable target is a no-op.
                        // trace:TASK-928 | ai:claude
                        if parent.is_none() {
                            if let Some(sid) = last.spec_id.as_deref() {
                                match ensure_parent_edge_from_tag(backend, sid) {
                                    Ok(Some(pdisp)) => {
                                        println!(
                                            "  Linked: {} → parent of {} (from parent: tag)",
                                            pdisp, sid
                                        )
                                    }
                                    Ok(None) => {}
                                    Err(e) => eprintln!(
                                        "  {} could not link parent from tag: {}",
                                        "Warning:".yellow().bold(),
                                        e
                                    ),
                                }
                            }
                        }
                        if let Some(spec_id) = last.spec_id.as_deref() {
                            let main_project_dir = main_worktree_root_from(&project_dir);
                            if let Err(e) = effort_calibration::upsert_open(
                                &main_project_dir,
                                spec_id,
                                *effort,
                                Some(current_user_id(None)),
                            ) {
                                eprintln!(
                                    "  {} could not record open effort for {spec_id}: {e}",
                                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                                );
                            }
                            // BUG-372: make newly allocated SPEC-IDs visible to the
                            // remote orphan store immediately when online, so a
                            // sibling clone that files next pulls the allocation
                            // before dispensing from its own block.
                            push_store_after_id_allocation(store_path, &project_dir, spec_id)?;
                        }

                        // TASK-754: `--queue` files + approves + enqueues in one shot,
                        // collapsing the advisor's "file as approved, then groom it"
                        // two-step. It reuses the exact `aida backlog groom` enqueue
                        // helper (user resolution, AIDA_SESSION_ROLE routing, note,
                        // optional batch tag) so the surfaces stay consistent. Two
                        // gates are honored, not invented:
                        //   * AC5 — advisor authority: the intake above already
                        //     downgrades a requested Approved to Draft when the session
                        //     lacks advisor authority (`intake_downgraded_from`), so a
                        //     non-advisor/non-TTY `--queue` lands a draft and is then
                        //     refused by the Approved check below — the same hard gate
                        //     `aida queue add` enforces, with no bypass.
                        //   * AC2 — only Approved is enqueueable, matching
                        //     `aida backlog groom`'s policy; a draft/other status is
                        //     refused with guidance rather than silently queued.
                        // trace:TASK-754 | ai:claude
                        if *queue {
                            if let Some(reason) = queue_at_filing_refusal(
                                &last.status,
                                intake_downgraded_from.is_some(),
                                r#for.as_deref(),
                            ) {
                                let did = last.spec_id.as_deref().unwrap_or("?");
                                match reason {
                                QueueAtFilingRefusal::Downgraded => anyhow::bail!(
                                    "{} was filed but NOT queued: queuing work for execution needs \
                                     advisor authority (advisor role or an interactive session). \
                                     The spec landed as a draft for advisor triage. Re-run as the \
                                     advisor, or have the advisor approve + groom it.",
                                    did
                                ),
                                QueueAtFilingRefusal::NotApproved => anyhow::bail!(
                                    "{} is `{}`, not Approved — `--queue` only enqueues Approved work \
                                     (same policy as `aida backlog groom`). Pass `--status approved` \
                                     when you mean to file-approve-and-queue in one shot.",
                                    did,
                                    last.status
                                ),
                            }
                            }
                            let storage = Storage::new(store_path);
                            let user_id = current_user_id(None);
                            let batch_norm: Option<&str> =
                                batch.as_deref().map(normalize_batch_name);
                            // BUG-528: route the freshly-filed work explicitly rather
                            // than to the FILER's session role. The common case is an
                            // advisor filing implementation work, so `add --queue`
                            // defaults to the `implementer` queue (the overwhelmingly
                            // common target) — overridable with `--for <role>`, and
                            // `--for any` leaves it unrouted. This mirrors the routing
                            // surface of `aida queue add --for`; without it the
                            // advisor-filed bug/task silently landed in the ADVISOR
                            // queue and `burndown run` (which drains the implementer
                            // queue) would miss it. trace:BUG-528 | ai:claude
                            let route_role: Option<String> = add_queue_route_role(r#for.as_deref());
                            backlog::enqueue_groomed_for(
                                &storage,
                                last,
                                batch_norm,
                                Some("advisor-cleared at filing"),
                                &user_id,
                                route_role.clone(),
                            )?;
                            let did = last.spec_id.as_deref().unwrap_or("?");
                            let route_suffix = match route_role.as_deref() {
                                Some(role) => format!(" → {} queue", role),
                                None => " (unrouted)".to_string(),
                            };
                            if let Some(name) = batch_norm {
                                println!(
                                "{} Queued {}{} (advisor-cleared at filing), tagged `batch:{}`.",
                                crate::glyph(crate::glyphs::Glyph::Check).green(),
                                did.bold(),
                                route_suffix,
                                name.bold()
                            );
                            } else {
                                println!(
                                    "{} Queued {}{} (advisor-cleared at filing).",
                                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                                    did.bold(),
                                    route_suffix
                                );
                            }
                        }
                        // TASK-974 (AXI #9): the lifecycle-aware next-step block — the
                        // valid next transition(s) for the freshly-filed spec's status (a
                        // draft becomes approve/reject), templated with its id.
                        //
                        // STORY-737 (delight #2): a brand-new user files spec #1 on the
                        // HUMAN TTY and needs this nudge MOST, yet it used to fire only
                        // under `agent_output_mode()` — the agent got guided, the human
                        // got a bare "Added: …". Render the human `Next:` footer (the same
                        // idiom `aida show` uses) plus a trace-link breadcrumb so the
                        // newcomer learns the one move that wires their code to the spec.
                        // Agent mode keeps its TOON `next` block; the two never both fire.
                        // trace:TASK-974 trace:STORY-737 | ai:claude
                        if let Some(sid) = last.spec_id.as_deref() {
                            if agent_output_mode() {
                                let next =
                                    crate::help_next::spec_next(&last.status.to_string(), sid);
                                if let Some(block) = crate::help_next::render(&next) {
                                    println!("{block}");
                                }
                            } else if is_faq {
                                // trace:STORY-1642 | ai:claude — no trace breadcrumb.
                                if let Some(block) = crate::help_next::render_human(
                                    &crate::help_next::spec_next(&last.status.to_string(), sid),
                                ) {
                                    println!("{block}");
                                }
                            } else {
                                println!(
                                    "{}",
                                    crate::help_next::render_human_add_footer(
                                        &last.status.to_string(),
                                        sid
                                    )
                                );
                            }
                        }
                        // STORY-700: passive first-run chain — advancing past the "file
                        // your first spec" step. Only fires when the arc is sitting at
                        // exactly that step (the first `add` after a fresh `init`); every
                        // later `add` is a silent no-op. trace:STORY-700 | ai:claude
                        if let Ok(project_root) = find_project_root() {
                            first_run::after_first_spec(&project_root);
                        }
                    }
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                AddArgs {
                    title,
                    title_positional,
                    description,
                    description_from_file,
                    description_stdin,
                    status,
                    priority,
                    r#type,
                    owner,
                    feature,
                    tags,
                    prefix,
                    parent,
                    blocked_by: _, // STORY-446: blocked-by edges land on the git-backend path
                    force_parent,
                    interactive,
                    // STORY-333: legacy centralized backend does not persist the
                    // human-only marker; ignore the flags here. trace:TASK-130 | ai:claude
                    human_only: _,
                    no_human_only: _,
                    effort: _,
                    // TASK-754: --queue/--batch are git-backend-path features (they
                    // reuse the backlog-groom enqueue helper); the deprecated
                    // centralized backend ignores them. trace:TASK-754 | ai:claude
                    // BUG-528: --for routes the queued spec; also git-backend-only.
                    // trace:BUG-528 | ai:claude
                    queue: _,
                    batch: _,
                    r#for: _,
                    // FR-283: the numeric weight is git-canonical only; the deprecated
                    // centralized backend ignores it. trace:FR-283 | ai:claude
                    weight: _,
                    // TASK-1267: execution modes are git-canonical only.
                    mode: _,
                    // STORY-1427: intake gates run on the git-canonical path only.
                    gates: _,
                } => {
                    // TASK-725: positional title (`aida add "do X"`) — --title wins.
                    let title = title.clone().or_else(|| title_positional.clone());
                    let title = &title;
                    // trace:BUG-17 | ai:claude — resolve description from inline,
                    // file, or stdin sources before dispatching.
                    let resolved_description = resolve_description(
                        description,
                        description_from_file,
                        *description_stdin,
                    )?;
                    let description = &resolved_description;

                    // trace:BUG-22 | ai:claude — warn if the title looks shell-mangled.
                    if let Some(ref t) = title {
                        if let Some(msg) = suspicious_title_signal(t) {
                            eprintln!("{} {}", "Warning:".yellow().bold(), msg);
                        }
                    }

                    // Default to interactive mode if no specific arguments are provided
                    let should_be_interactive = *interactive
                        || (title.is_none()
                            && description.is_none()
                            && status.is_none()
                            && priority.is_none()
                            && r#type.is_none()
                            && owner.is_none()
                            && feature.is_none()
                            && tags.is_none()
                            && prefix.is_none()
                            && parent.is_none());

                    if should_be_interactive {
                        add_requirement_interactive(&storage)?;
                    } else {
                        add_requirement_cli(
                            &storage,
                            title,
                            description,
                            status,
                            priority,
                            r#type,
                            owner,
                            feature,
                            tags,
                            prefix,
                            parent,
                            *force_parent,
                        )?;
                    }
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static ADD: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "add",
    help_after: "derisk",
    command: <AddArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Intake,
        per_write_auto_push: true,
        skip_role_restore_prompt: false,
        team_identity_write: true,
    },
    run: crate::command_registry::Handler::Store(run_add),
};

/// List all requirements
// trace:TASK-1463 | ai:claude — `aida ls` is a short, discoverable alias.
#[derive(clap::Parser, Debug)]
#[clap(name = "list")]
#[clap(visible_alias = "ls")]
pub(crate) struct ListArgs {
    /// Optional positional shortcut (e.g. `aida list approved`).
    ///
    /// A positional alternative to `--status` / `--user` / the lens flags.
    /// Accepts a single status, a comma-separated OR set (`draft,approved`),
    /// a status alias, a `user:<name>` / `me` owner-or-assignee filter, or a
    /// lens word. An unrecognized token errors with guidance rather than
    /// being silently ignored.
    ///
    /// Statuses (one per line):
    ///   draft           - filed, not yet approved
    ///   approved        - blessed, ready to plan/queue
    ///   planned         - designed, awaiting implementation
    ///   in-progress     - actively being worked
    ///   needs-attention - parked by a drain; needs a human triage
    ///   done            - work finished on a branch (pre-merge)
    ///   completed       - merged to the default branch
    ///   rejected        - declined; will not be done
    ///
    /// Status aliases:
    ///   open            - Draft, Approved, Planned, InProgress, NeedsAttention
    ///   closed          - Done, Completed, Rejected
    ///
    /// User filters (owner OR assignee):
    ///   me              - your shell identity ($AIDA_USER / $USER)
    ///   user:<name>     - the named person (e.g. `user:joe`)
    ///
    /// Lens words (route to a focused view):
    ///   human           - the "what needs me?" human-attention view
    ///   queue           - your role's queue (= `aida queue list`)
    ///   advisor         - the advisor dashboard (= `aida advisor`)
    ///   why             - the burndown classifier (= `aida burndown explain`)
    ///   inflight        - active leases + drain status
    ///
    /// Type words (= `--type <word>`, e.g. `aida list faq`):
    ///   bug, story, task, epic, spike, faq, decision, vision, term, ...
    // trace:TASK-0415 | ai:claude — plain `//` keeps the marker out of `--help`.
    // trace:STORY-1642 | ai:claude — type words in the positional.
    // trace:STORY-662 | ai:claude — verbatim_doc_comment preserves the
    // one-shortcut-per-line layout (clap otherwise reflows into paragraphs).
    #[clap(value_name = "STATUS", verbatim_doc_comment)]
    pub shortcut: Option<String>,

    /// Filter by status. Accepts a comma-separated OR set
    /// (`--status draft,approved`) and the `open` / `closed` aliases:
    /// `open` = Draft, Approved, Planned, InProgress, NeedsAttention;
    /// `closed` = Done, Completed, Rejected. Three non-stored tokens are
    /// also accepted: `shelved` and `needs-decision` narrow the parked
    /// (needs-attention) set, and `deferred` is a spelling of `--deferred`
    /// — the primed/conditional shelf, which is a view flag rather than a
    /// status, so it composes as an AND (`--status deferred,approved`).
    // trace:TASK-0415 | ai:claude
    // trace:BUG-1771 | ai:claude — the two lenses were accepted only by the
    // legacy centralized listing, so this surface could not name them until
    // the git-backend path learned them.
    // trace:BUG-1687 | ai:claude — `deferred` used to be refused here with
    // a message that denied the token existed.
    #[clap(long)]
    pub status: Option<String>,

    /// Filter by priority
    #[clap(long)]
    pub priority: Option<String>,

    /// Filter by type
    #[clap(long)]
    pub r#type: Option<String>,

    /// Filter by feature
    #[clap(long)]
    pub feature: Option<String>,

    /// Filter by tags (comma separated). A trailing `*` is a prefix-glob:
    /// `aida:queue:*` matches every tag under that surface (and an exact
    /// `aida:queue`). e.g. `--tags 'aida:queue:*'`. `--tag` is accepted too.
    // trace:TASK-527 | ai:claude — plain `//` keeps the marker out of `--help`.
    // trace:STORY-1645 | ai:claude — `--tag` alias (`aida faq list --tag <topic>`).
    #[clap(long, alias = "tag")]
    pub tags: Option<String>,

    /// Show only in-progress specs whose derived driver is this kind:
    /// relay, drain, agent, queued or idle. The driver is shown beside
    /// the status of every in-progress row.
    // trace:STORY-1653 | ai:claude
    #[clap(long, value_name = "KIND")]
    pub driver: Option<String>,

    /// Show only machine-filed auto-complete failure drafts. Plain
    /// `aida list --status draft` shows human-filed drafts so advisor
    /// grooming is not dominated by orchestrator records. Machine drafts
    /// remain batchable with `--machine-drafts --short`.
    // trace:BUG-1498 | ai:codex
    #[clap(long, requires = "status")]
    pub machine_drafts: bool,

    /// Bypass the active role's scope filters for this command.
    // trace:TASK-1-021 | ai:claude
    #[clap(long)]
    pub no_scope: bool,

    /// Show the origin id alongside the canonical id. The origin id is
    /// the original spec_id assigned when the requirement was created
    /// (zero-padded form for legacy reqs, node-aware
    /// `<TYPE>-<node>-<NNN>` for distributed-mode reqs). The canonical
    /// id is the agreed-id post-merge-gate when one exists, else the
    /// origin id itself.
    // trace:FR-1-070 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(long, alias = "verbose", short = 'v')]
    pub show_origin: bool,

    /// Include META requirements (AI prompt customization seeded by
    /// `aida init`) in the output. By default META rows are hidden so
    /// they don't drown out user-authored reqs on small projects;
    /// pass `--include-meta` to see them, or filter explicitly with
    /// `--type meta`.
    // trace:BUG-27 | ai:claude
    #[clap(long)]
    pub include_meta: bool,

    /// Include knowledge rows (faq, decision, vision, term, principle) in
    /// the output. They are reference material, not work, so the default
    /// list hides them; `--type <type>` or `aida <type> list` (e.g.
    /// `aida faq list`) also shows them.
    // trace:STORY-1642 | ai:claude
    #[clap(long)]
    pub include_knowledge: bool,

    /// Restrict the listing to direct children of <id> (UUID or
    /// SPEC-ID). Composes with --status / --type / --tags etc., so
    /// e.g. `aida list --parent <epic-id> --status approved` shows
    /// what's still open under that EPIC. Add --recursive to widen this
    /// to the WHOLE transitive subtree instead of just direct children.
    // trace:STORY-62 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(long, value_name = "ID")]
    pub parent: Option<String>,

    /// Widen --parent from direct children to the WHOLE transitive subtree:
    /// the parent plus every descendant via parent->child edges, at any
    /// depth. So `aida list open --parent <epic> --recursive` returns the
    /// open items across the epic's entire subtree, not just its direct
    /// children. Requires --parent; composes with the [STATUS] positional,
    /// --status / --type / --tags. Cache-fast (a single recursive query over
    /// the materialized edge graph, no full-store load).
    // trace:TASK-955 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(long, visible_alias = "subtree", requires = "parent")]
    pub recursive: bool,

    /// Pull from `origin/aida-store` before listing. Opt-in
    /// freshness for collaborators / multi-session workflows;
    /// fast path stays default. No-op when the local orphan
    /// branch is already current; warns and falls back to the
    /// local view when offline.
    // trace:STORY-78 | ai:claude
    #[clap(long)]
    pub sync: bool,

    /// Include archived AND deferred requirements (the
    /// everything-escape-hatch). By default `aida list` hides both archived
    /// and deferred rows; archived/deferred ≠ status, so freshly-Completed
    /// specs stay visible until archived. Mutually exclusive with
    /// `--archived` and `--deferred` (the only-X views).
    // trace:STORY-441 | ai:claude — supersedes TASK-64's terminal-status hide.
    // trace:STORY-584 | ai:claude — now widens the defer axis too.
    #[clap(long, conflicts_with_all = ["archived", "deferred"])]
    pub all: bool,

    /// Show only archived requirements. Use this to audit the archive
    /// itself; `--all` shows the union of active + deferred + archived.
    // trace:STORY-441 | ai:claude
    #[clap(long, conflicts_with_all = ["all", "deferred"])]
    pub archived: bool,

    /// Show only deferred requirements — the primed/conditional shelf,
    /// with each spec's revisit trigger. Honors both the deferred flag and
    /// legacy `deferred:*` parking tags. `--all` shows the union.
    // trace:STORY-584 | ai:claude
    #[clap(long, conflicts_with_all = ["all", "archived"])]
    pub deferred: bool,

    /// Emit `[{spec_id,title,req_type,status,tags}]` as JSON instead
    /// of the human table. Internal-use surface for the TUI launcher
    /// to populate its Backlog / History panes; the
    /// schema may change without notice.
    // trace:STORY-244 | ai:claude
    #[clap(long, hide = true)]
    pub json: bool,

    /// Group the listing by parent EPIC for visual clustering —
    /// children indented under their EPIC, groups sorted by item
    /// count desc, requirements with no EPIC parent under a final
    /// "Unscoped" group. Mirrors `aida queue list --tree`. Composes
    /// with the --status / --type / --priority / --tags / --parent
    /// filters. Mutually exclusive with --json.
    // trace:TASK-568 | ai:claude
    #[clap(long, conflicts_with = "json")]
    pub tree: bool,

    /// Surface each row's tags as an extra column right of Title.
    /// The chip set is truncated to the first three tags with a
    /// "+N more" suffix when more exist. Composes with every
    /// existing filter (`--tags`, `--status`, `--type`, …).
    // trace:TASK-569 | ai:claude
    #[clap(long)]
    pub show_tags: bool,

    /// Also mark specs blocked behind an incomplete blocker with a leading
    /// ⊘ glyph. Off by default; the cheap queued ↑ / supervised queued ⇈
    /// (needs guided/operator session; headless drain skips) / in-flight ▶
    /// overlay is always on.
    // trace:TASK-670 | ai:claude
    // trace:TASK-902 | ai:claude — blocked is now read from the cache (a
    // projected column), no longer a full-store load.
    #[clap(long)]
    pub blocked: bool,

    /// Drop the leading work-routing glyph column (↑ queued / ⇈ queued but
    /// needs guided/operator session; headless drain skips / ▶ in-flight /
    /// ⊘ blocked) but keep the status glyphs. "Show me
    /// the list without the routing overlay."
    // trace:TASK-670 | ai:claude
    #[clap(long)]
    pub no_flow: bool,

    /// Strip ALL glyphs (status + work-routing) for plain-text
    /// output — scripting, grep, non-Unicode terminals,
    /// accessibility. Glyphs are on by default; this is the opt-out.
    // trace:TASK-670 | ai:claude
    #[clap(long)]
    pub no_glyph: bool,

    /// Emit one bare canonical spec ID per line — no header, no count
    /// footer, no color. Directly usable in `$(...)` / xargs:
    /// `aida edit $(aida list open --short --type bug) ...`. Composes
    /// with every filter (`--status` / shortcut / `--type` / `--tags`
    /// / `--parent`) and honors the same archive default as plain
    /// `list`. Mutually exclusive with --json / --tree.
    // trace:TASK-743 | ai:claude
    #[clap(long, visible_alias = "ids-only", visible_alias = "quiet", short = 'q', conflicts_with_all = ["json", "tree"])]
    pub short: bool,

    /// The "what needs me?" view: every OPEN spec needing a human nudge,
    /// grouped by reason — held-for-review, awaiting-decision, drafts to
    /// approve, NeedsAttention to triage. Excludes the self-resolving rest
    /// (in-flight, deferred, awaiting-merge, long-lived, actionable). Reuses
    /// the `aida burndown explain` classifier as the single source of truth.
    /// Also reachable as the positional alias `aida list human`. Composes
    /// with `--short` (prints just the IDs).
    // trace:STORY-562 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(long, conflicts_with_all = ["json", "tree", "status"])]
    pub human: bool,

    /// Order the results. `id` (default) = natural numeric ID order:
    /// type prefix first (ADR, BUG, EPIC, FAQ, STORY, TASK, …), then each
    /// number as an integer, so number 2 comes before 11 and 2 before
    /// 1000 within a type; the agreed ID is the key when a spec has one,
    /// and unpromoted node-aware IDs (prefix-node-seq) follow the agreed
    /// IDs of their type. `modified` = freshest first; `heft` = most
    /// graph-connected first (the deterministic in+out-degree weight), so
    /// load-bearing specs surface at the top; `weight` = heaviest user-set
    /// numeric weight/score first (unweighted specs sort last); `created`
    /// = newest-created first; `completed` = most-recently-completed first
    /// (specs with no completion date sort last). The default is
    /// configurable via `[list] default_sort` in `.aida/config.toml`; an
    /// explicit `--sort` always wins. Same order for humans and agents.
    // trace:STORY-632 | ai:claude — plain `//` keeps the marker out of `--help`.
    // trace:FR-283 | ai:claude — adds the `weight` order.
    // trace:TASK-1464 | ai:claude — adds the `created` / `completed` orders.
    // trace:TASK-1587 | ai:antigravity — adds the `id` order.
    // trace:STORY-1644 | ai:claude — natural `id` default, configurable.
    #[clap(long, value_name = "ORDER")]
    pub sort: Option<String>,

    /// Only specs whose numeric weight/score is >= N. Specs with no
    /// weight set are excluded. Composes with every other filter.
    // trace:FR-283 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(long, value_name = "N", allow_hyphen_values = true)]
    pub min_weight: Option<f64>,

    /// Only specs whose numeric weight/score is <= N. Specs with no
    /// weight set are excluded. Composes with every other filter.
    // trace:FR-283 | ai:claude
    #[clap(long, value_name = "N", allow_hyphen_values = true)]
    pub max_weight: Option<f64>,

    /// Cap the output at the first N rows, applied AFTER sorting — so
    /// `--limit N` returns the first N by ID (the default `--sort id`),
    /// the N most-recent (`--sort modified`) or the N most-connected
    /// (`--sort heft`). `--limit 0` = no cap. Composes with every filter;
    /// applies to both the human table and `--json`. Omit for the full
    /// (filtered) listing — except in agent mode (piped / non-TTY), where
    /// the default table is capped at 100 rows with a notice naming what
    /// was cut. That agent cap is configurable via `[list] agent_limit`
    /// in `.aida/config.toml` (0 = unbounded) or env `AIDA_LIST_LIMIT`;
    /// `--limit` / `--all` beat env, which beats config.
    // trace:TASK-900 | ai:claude — plain `//` keeps the marker out of `--help`.
    // trace:STORY-1644 | ai:claude
    #[clap(long, value_name = "N")]
    pub limit: Option<usize>,

    /// Show only specs assigned to you (assignee == your shell identity).
    /// Composes with every other filter. Mutually exclusive with
    /// `--assigned`.
    // trace:STORY-639 | ai:claude
    #[clap(long, conflicts_with = "assigned")]
    pub mine: bool,

    /// Show only specs assigned to <user>. Composes with every other
    /// filter.
    // trace:STORY-639 | ai:claude
    #[clap(long, value_name = "USER")]
    pub assigned: Option<String>,

    /// Show only specs whose OWNER or ASSIGNEE is <user>. `me` resolves to
    /// your shell identity ($AIDA_USER / $USER — the same resolution the
    /// queue uses). Broader than `--assigned`, which matches assignee only.
    /// Also reachable as the positional `aida list me` / `aida list
    /// user:<name>`. Composes with every other filter.
    // trace:STORY-662 | ai:claude
    #[clap(long, value_name = "USER")]
    pub user: Option<String>,

    /// Ignore the active focus for this listing — show the whole project
    /// instead of the focused subtree. The focus-specific escape hatch
    /// (`--all` also widens, but additionally reveals archived/deferred);
    /// no-op when no focus is set or when `--parent` was passed explicitly.
    // — plain `//` keeps the marker out of `--help`.
    #[clap(long)]
    pub no_focus: bool,

    /// Select AND order the displayed columns, e.g.
    /// `--fields id,title,status,priority,tags`. Renders exactly those
    /// columns in that order — on the human table AND the token-efficient
    /// agent output (a real token win for agents). Valid fields: id, title,
    /// status, type, priority, feature, owner, assignee, tags, heft, weight,
    /// queued, in_flight, blocked. Default (no `--fields`) keeps the standard
    /// columns; agent mode defaults to the minimal id,title,status,type set.
    // trace:TASK-964 | ai:claude — plain `//` keeps the marker out of `--help`.
    // trace:STORY-734 | ai:claude — now drives the human table too.
    #[clap(long, value_name = "CSV")]
    pub fields: Option<String>,

    /// List the topics (tags) in use on live FAQs, with how many FAQs
    /// carry each, most-used first. Requires the FAQ type: `aida faq
    /// topics` (== `aida list --type faq --topics`). Browse one topic
    /// with `aida faq list --tags <topic>`.
    // trace:STORY-1645 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(long)]
    pub topics: bool,
}

fn run_list(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <ListArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => {
            // trace:TASK-1583 | ai:antigravity
            if let ListArgs {
                shortcut: Some(s),
                json,
                ..
            } = &args
            {
                if s == "pr" || s == "mr" {
                    return crate::command_registry::returned(pr_list_handler(*json, false));
                }
                // trace:TASK-1590 | ai:antigravity
                if s == "review" {
                    return crate::command_registry::returned(handle_review_list(*json));
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                ListArgs {
                    shortcut,
                    status,
                    r#type,
                    priority,
                    feature,
                    tags,
                    // trace:STORY-1653 | ai:claude
                    driver,
                    machine_drafts,
                    no_scope,
                    show_origin,
                    include_meta,
                    include_knowledge,
                    parent,
                    recursive,
                    sync,
                    all,
                    archived,
                    deferred,
                    json,
                    tree,
                    show_tags,
                    blocked,
                    no_flow,
                    no_glyph,
                    short,
                    human,
                    sort,
                    // trace:FR-283 | ai:claude
                    min_weight,
                    max_weight,
                    mine,
                    assigned,
                    user,
                    limit,
                    no_focus,
                    fields,
                    // trace:STORY-1645 | ai:claude
                    topics,
                    ..
                } => {
                    // STORY-764: the global `--format json` pin reaches list the same way
                    // its own `--json` flag does. `--format human` / `toon` are handled
                    // upstream by `agent_output_mode()`, so only the json axis needs to be
                    // OR'd in here. trace:STORY-764 | ai:claude
                    let effective_json = *json || output_format_is_json();
                    // STORY-1642 item 7: a built-in type word in the positional is a
                    // type filter — `aida list faq` == `aida list --type faq` — rather
                    // than an "Unknown status filter" error. No type word collides with
                    // a status, alias, lens or user token, so peeling it off here
                    // shadows nothing. trace:STORY-1642 | ai:claude
                    let positional_type =
                        crate::knowledge_list::positional_type_word(shortcut.as_deref());
                    let type_filter: Option<String> =
                        match (positional_type.clone(), r#type.clone()) {
                            (Some(p), Some(t)) => anyhow::bail!(
                                "Pass the type filter once: either the positional `aida list {p}` \
                         or `--type {t}`, not both."
                            ),
                            (Some(t), None) | (None, Some(t)) => Some(t),
                            (None, None) => None,
                        };
                    let r#type: &Option<String> = &type_filter;
                    // STORY-1645: `aida faq topics` (== `aida list --type faq
                    // --topics`) lists the tags on live FAQs with counts. It reads the
                    // whole live FAQ set itself, so no other list lens applies.
                    // trace:STORY-1645 | ai:claude
                    if *topics {
                        if !crate::knowledge_list::type_filter_is_faq(r#type.as_deref()) {
                            anyhow::bail!(
                                "`--topics` lists FAQ topics: run `aida faq topics` \
                             (or `aida list --type faq --topics`)."
                            );
                        }
                        if *sync {
                            maybe_sync_pull(store_path)?;
                        }
                        return crate::command_registry::returned(
                            crate::faq_topics::run_faq_topics(backend, effective_json),
                        );
                    }
                    let shortcut: Option<String> = if positional_type.is_some() {
                        None
                    } else {
                        shortcut.clone()
                    };
                    // STORY-562: `aida list human` (positional alias) and `aida list
                    // --human` both resolve to the "what needs me?" view — every open
                    // spec the `burndown explain` classifier flags as needing a human
                    // nudge, grouped by reason. Detect it BEFORE the status-shortcut
                    // expansion so the positional `human` token isn't rejected as an
                    // unknown status. Composes with `--short` (ids-only). trace:STORY-562
                    let want_human = *human
                        || shortcut
                            .as_deref()
                            .map(|s| s.eq_ignore_ascii_case("human"))
                            .unwrap_or(false);
                    if want_human {
                        if let Some(s) = status.as_deref() {
                            anyhow::bail!(
                                "`aida list human` is the human-attention view, not a status \
                             filter — drop `--status {s}`. (Use plain `aida list --status \
                             {s}` for a status listing.)"
                            );
                        }
                        if *sync {
                            maybe_sync_pull(store_path)?;
                        }
                        return crate::command_registry::returned(handle_list_human(
                            *short, backend,
                        ));
                    }
                    // STORY-662: resolve the owner-or-assignee `--user` filter. The
                    // positional `me` / `user:<name>` token is an alternate spelling of
                    // `--user me` / `--user <name>` — peel it off BEFORE the status
                    // shortcut expansion so it isn't rejected as an unknown status. `me`
                    // resolves to the shell identity via `current_user_id` (the same
                    // resolution the queue keys off — $AIDA_USER / $USER). `aida list`
                    // does NOT default to the current user; this is an opt-in filter.
                    // trace:STORY-662 | ai:claude
                    let positional_user: Option<String> = shortcut.as_deref().and_then(|s| {
                        if s.eq_ignore_ascii_case("me") {
                            Some("me".to_string())
                        } else {
                            s.strip_prefix("user:")
                                .filter(|rest| !rest.is_empty())
                                .map(|rest| rest.to_string())
                        }
                    });
                    let raw_user: Option<String> = match (positional_user, user.clone()) {
                        (Some(_), Some(_)) => {
                            anyhow::bail!(
                                "Pass the user filter once: either the positional \
                             `aida list me` / `aida list user:<name>` or `--user <name>`, \
                             not both."
                            );
                        }
                        (Some(u), None) | (None, Some(u)) => Some(u),
                        (None, None) => None,
                    };
                    // `me` → the shell identity; any other value is a literal handle.
                    let user_filter: Option<String> =
                        raw_user.map(|u| resolve_list_user_filter(&u, &current_user_id(None)));
                    // The positional `me` / `user:<name>` token was consumed as a user
                    // filter, so it must not also flow into the status-shortcut path.
                    let shortcut: Option<String> = if user_filter.is_some()
                        && shortcut
                            .as_deref()
                            .map(|s| s.eq_ignore_ascii_case("me") || s.starts_with("user:"))
                            .unwrap_or(false)
                    {
                        None
                    } else {
                        shortcut.clone()
                    };
                    // TASK-0415: resolve the optional positional status shortcut.
                    // `aida list approved` == `aida list --status approved`; the
                    // positional and `--status` are two spellings of the same axis,
                    // so reject using both at once rather than silently picking one.
                    // Both forms run through `expand_filter_spec`, which expands the
                    // `open` / `closed` aliases and the comma-separated OR set, and
                    // errors clearly on an unrecognized token (pointing at the real
                    // filters) instead of swallowing it.
                    // trace:TASK-0415 | ai:claude
                    let raw_status: Option<String> = match (shortcut.as_deref(), status.as_deref())
                    {
                        (Some(_), Some(_)) => {
                            anyhow::bail!(
                                "Pass the status filter once: either the positional \
                             `aida list <status>` or `--status <status>`, not both."
                            );
                        }
                        (Some(s), None) | (None, Some(s)) => Some(s.to_string()),
                        (None, None) => None,
                    };
                    if *machine_drafts
                        && !raw_status
                            .as_deref()
                            .is_some_and(crate::status_spec_is_exact_draft)
                    {
                        anyhow::bail!(
                            "`--machine-drafts` requires the exact draft view: \
                         `aida list --status draft --machine-drafts`"
                        );
                    }
                    // BUG-788: capture whether this is the explicit `open` shortcut
                    // BEFORE `raw_status` is expanded into the canonical status set —
                    // once expanded, `open` is indistinguishable from a hand-typed
                    // `draft,approved,...` list. The explicit `open` verb must share the
                    // bare-list default lens's accepted-decision exclusion (BUG-781), so
                    // `aida list` and `aida list open` agree. trace:BUG-788 | ai:claude
                    let explicit_open_alias =
                        crate::status_spec_is_open_alias(raw_status.as_deref());
                    let exact_draft_view = raw_status
                        .as_deref()
                        .is_some_and(crate::status_spec_is_exact_draft);
                    // BUG-1771: `shelved` and `needs-decision` are STORY-1023 display
                    // lenses over the stored `NeedsAttention` status, not statuses — no
                    // cache column holds them, so `expand_filter_spec` rejected both and
                    // the two lenses were unreachable from the shipped CLI even though
                    // the legacy `list_requirements` path accepted them. Split the lens
                    // tokens out, widen the cache query to `needs-attention` so the
                    // superset comes back, and narrow the rows by lens after the query
                    // (see `parked_lens_keys` at the retain below).
                    //
                    // `lens_widened_needs_attention` records that `needs-attention`
                    // reached the query ONLY as the lens widening, not because the caller
                    // named it. That distinction is what lets a mixed set honour both
                    // halves (`--status shelved,approved` keeps Approved rows and the
                    // Shelved subset of the parked rows) without silently dropping a
                    // token, while an explicit `--status shelved,needs-attention` still
                    // shows every parked row.
                    // trace:BUG-1771 | ai:claude
                    let mut parked_lens_keys: Vec<&'static str> = Vec::new();
                    let mut lens_widened_needs_attention = false;
                    // BUG-1687: `--status deferred` is the other non-stored token.
                    // Deferral is STORY-584's view-flag, orthogonal to status, so it
                    // has no status to widen to — it opens the defer axis instead,
                    // exactly as `--deferred` does. trace:BUG-1687 | ai:claude
                    let mut status_asked_deferred = false;
                    let status: Option<String> = match raw_status {
                        Some(spec) => {
                            let split = crate::status_display::split_status_filter_spec(&spec);
                            parked_lens_keys = split.lens_keys;
                            status_asked_deferred = split
                                .view_axes
                                .contains(&crate::status_display::ViewFilterAxis::Deferred);
                            let mut expanded = match split.residual.as_deref() {
                                Some(residual) => {
                                    aida_core::RequirementStatus::expand_filter_spec(residual)
                                        .map_err(|tok| {
                                            anyhow::anyhow!(
                                                "Unknown status filter '{tok}'. Use a status \
                                             (draft, approved, planned, in-progress, done, \
                                             completed, rejected, needs-attention), a parked \
                                             lens ({lenses}), a view lens ({views}), an alias \
                                             (open, closed), or a comma-separated set \
                                             (draft,approved). To filter by something else try \
                                             --type, --tags, or `aida search`.",
                                                lenses =
                                                    crate::status_display::lens_filter_token_list(),
                                                views =
                                                    crate::status_display::view_filter_token_list()
                                            )
                                        })?
                                }
                                // Non-stored tokens only: nothing stored was named, so
                                // the lens widening below (or the defer axis) supplies
                                // the whole query.
                                None => Vec::new(),
                            };
                            if !parked_lens_keys.is_empty() {
                                let needs_attention = aida_core::RequirementStatus::NeedsAttention
                                    .cache_key()
                                    .to_string();
                                if !expanded.contains(&needs_attention) {
                                    expanded.push(needs_attention);
                                    lens_widened_needs_attention = true;
                                }
                            }
                            if expanded.is_empty() {
                                // A bare `--status deferred` names no status at all.
                                // `Some("")` would hand the cache an empty status
                                // clause; the honest value is "no status constraint",
                                // which also keeps the STORY-723 default open lens from
                                // re-hiding what the caller asked for (see
                                // `want_deferred_only`). trace:BUG-1687 | ai:claude
                                None
                            } else {
                                // expand_filter_spec returns canonical cache-keys; join
                                // them back into the comma-OR spec the cache understands.
                                Some(expanded.join(","))
                            }
                        }
                        None => None,
                    };
                    // STORY-78: opt-in implicit sync-pull before reading. Quiet
                    // on no-op (already current), warns + falls back on errors.
                    // Must run BEFORE the cache-backed list query because the
                    // cache rebuild on next read will pick up the new orphan
                    // HEAD. trace:STORY-78 | ai:claude
                    if *sync {
                        maybe_sync_pull(store_path)?;
                    }
                    // Cache-backed list (EPIC-1-001 Phase 2). The CachedGitBackend
                    // ensures the cache is fresh before querying, so this is one
                    // SQLite query instead of ~360 YAML reads.
                    // trace:EPIC-1-001 | ai:claude
                    //
                    // Phase 3 scope filters: when a role is active and has scope set,
                    // its tags/status are AND'd into the filter. Explicit --tags or
                    // --status on the command line override the role scope; --no-scope
                    // bypasses it entirely.
                    // trace:TASK-1-021 | ai:claude
                    let cli_tags: Vec<String> = tags
                        .as_deref()
                        .map(|t| {
                            t.split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect()
                        })
                        .unwrap_or_default();
                    let scope = if *no_scope { None } else { active_role_scope() };
                    let (effective_tags, effective_status) = match scope {
                        Some((scope_tags, scope_status)) => {
                            let final_tags = if !cli_tags.is_empty() {
                                cli_tags
                            } else {
                                scope_tags
                            };
                            let final_status = status.clone().or(scope_status);
                            (final_tags, final_status)
                        }
                        None => (cli_tags, status.clone()),
                    };
                    // STORY-441: three-way archive axis. Default hides archived rows;
                    // `--archived` shows only archived; `--all` shows both. The
                    // pre-STORY-441 terminal-status hide is gone — Completed/Rejected
                    // specs stay visible until archived.
                    // trace:STORY-441 | ai:claude
                    let archive = if *all {
                        aida_core::ArchiveFilter::Both
                    } else if *archived {
                        aida_core::ArchiveFilter::ArchivedOnly
                    } else {
                        aida_core::ArchiveFilter::NonArchivedOnly
                    };
                    // STORY-584: three-way defer axis, parallel to archive. Default
                    // hides deferred rows (flag set OR `deferred:*`-tagged); `--deferred`
                    // shows only the primed shelf; `--all` shows the union. `--archived`
                    // keeps the defer axis open so the archive audit is complete.
                    // If the user explicitly filters on a `deferred:` tag, opening the
                    // axis is implied — otherwise the default hide would contradict the
                    // tag filter and return nothing. trace:STORY-584 | ai:claude
                    let asked_for_defer_tag = effective_tags
                        .iter()
                        .any(|t| t.starts_with("deferred:") || t.starts_with("deferred*"));
                    // BUG-1687: `--status deferred` is a spelling of `--deferred`, so
                    // it reaches the axis through the same variable and inherits the
                    // same precedence — `--all` / `--archived` still widen to Both.
                    // trace:BUG-1687 | ai:claude
                    let want_deferred_only = *deferred || status_asked_deferred;
                    let defer = if *all || *archived || asked_for_defer_tag {
                        aida_core::DeferFilter::Both
                    } else if want_deferred_only {
                        aida_core::DeferFilter::DeferredOnly
                    } else {
                        aida_core::DeferFilter::NonDeferredOnly
                    };
                    // STORY-723: bare `aida list` defaults to the OPEN/actionable lens.
                    // A newcomer's first list should show LIVE work (draft → approved →
                    // planned → in-progress → needs-attention), not a wall of completed/
                    // rejected history. The default applies ONLY when no status filter is
                    // already in play (neither `--status`/positional nor a role scope
                    // status) and the view wasn't deliberately widened — `--all`,
                    // `--archived`, and `--deferred` each opt back into closed rows, and
                    // an explicit `aida list closed` / `--status completed` is untouched.
                    // The closed set stays one flag away (`--all` / `--status closed`).
                    // trace:STORY-723 | ai:claude
                    // BUG-1687: a bare `--status deferred` leaves `effective_status`
                    // None, so the defer axis is what has to stop the open-lens default
                    // from re-hiding a deferred-and-Completed row the caller asked for.
                    // trace:BUG-1687 | ai:claude
                    // STORY-1642 item 3: a knowledge-typed listing (`aida faq list`,
                    // `aida list --type decision`) shows every status except archived —
                    // FAQs live as `completed`, so the open lens would hide them all.
                    // trace:STORY-1642 | ai:claude
                    let knowledge_type_listing =
                        crate::knowledge_list::type_filter_is_knowledge(r#type.as_deref());
                    let default_open_lens = list_default_open_lens(
                        effective_status.is_some() || knowledge_type_listing,
                        *all,
                        *archived,
                        want_deferred_only,
                    );
                    let effective_status = if default_open_lens {
                        Some(
                            aida_core::RequirementStatus::open_statuses()
                                .iter()
                                .map(|s| s.cache_key())
                                .collect::<Vec<_>>()
                                .join(","),
                        )
                    } else {
                        effective_status
                    };
                    // STORY-632: resolve the --sort order. Unknown values fall back to
                    // the default with a stderr note rather than erroring the listing.
                    // trace:STORY-632 | ai:claude
                    //
                    // STORY-1644: the default is natural numeric `id` order for humans
                    // and agents alike; `[list] default_sort` overrides it and an
                    // explicit `--sort` always wins. trace:STORY-1644 | ai:claude
                    let list_project_root = find_project_root().ok();
                    let (sort_key, sort_config_warning) = crate::resolve_list_sort(
                        sort.as_deref(),
                        list_project_root
                            .as_deref()
                            .and_then(crate::read_list_default_sort)
                            .as_deref(),
                    );
                    if let Some(w) = sort_config_warning {
                        eprintln!("{w}");
                    }
                    let mut sort_key = sort_key.to_ascii_lowercase();
                    let sort_order = match sort_key.as_str() {
                        "heft" | "centrality" => aida_core::SortOrder::HeftDesc,
                        // trace:FR-283 | ai:claude — heaviest user-set weight first.
                        "weight" => aida_core::SortOrder::WeightDesc,
                        // trace:TASK-1464 | ai:claude — creation / completion date sorts.
                        "created" => aida_core::SortOrder::CreatedDesc,
                        "completed" => aida_core::SortOrder::CompletedDesc,
                        // trace:TASK-1587 | ai:antigravity
                        "id" | "" => aida_core::SortOrder::IdAsc,
                        "modified" => aida_core::SortOrder::ModifiedDesc,
                        other => {
                            eprintln!(
                            "warning: unknown --sort '{other}' (expected one of: {}); using '{}'",
                            crate::LIST_SORT_VALUES.join(", "),
                            crate::LIST_DEFAULT_SORT
                        );
                            sort_key = crate::LIST_DEFAULT_SORT.to_string();
                            aida_core::SortOrder::IdAsc
                        }
                    };
                    // STORY-639: `--mine` resolves to the shell identity (the same
                    // user the queue keys off); `--assigned <user>` filters on that
                    // user. The two are mutually exclusive (enforced by clap).
                    // trace:STORY-639 | ai:claude
                    let assignee_filter: Option<String> = if *mine {
                        Some(current_user_id(None))
                    } else {
                        assigned.clone()
                    };
                    // trace:TASK-845 | ai:claude — expand the assignee filter to the
                    // canonical person's full alias set so `--mine` / `--assigned`
                    // surfaces specs assigned under any of this human's cross-host owner
                    // strings. Empty by default (no aliases linked) → plain match.
                    let assignee_aliases: Vec<String> = match &assignee_filter {
                        Some(a) => {
                            let registry = aida_core::alias::AliasRegistry::load(store_path);
                            let folded = aida_core::node::canonical_user_id(a);
                            registry
                                .members_of(a)
                                .into_iter()
                                .filter(|m| *m != folded)
                                .collect()
                        }
                        None => Vec::new(),
                    };
                    let filter = aida_core::ListFilter {
                        status: effective_status.clone(),
                        req_type: r#type.clone(),
                        // trace:TASK-1-107 | ai:claude — was missing before;
                        // CLI accepted --priority but it never reached the query.
                        priority: priority.clone(),
                        feature: feature.clone(),
                        tags: effective_tags.clone(),
                        archive,
                        defer,
                        sort: sort_order,
                        assignee: assignee_filter,
                        assignee_aliases,
                        // trace:STORY-662 | ai:claude — `--user` / `me` / `user:<name>`.
                        owner_or_assignee: user_filter.clone(),
                        // trace:FR-283 | ai:claude — numeric weight bounds.
                        min_weight: *min_weight,
                        max_weight: *max_weight,
                        ..Default::default()
                    };
                    let mut reqs = backend.list_summaries(&filter)?;
                    let mut lazy_store = None;

                    // BUG-1771: narrow the widened query back down to the requested
                    // parked lens(es). Runs before every downstream lens so the rows the
                    // renderers see are already the asked-for set.
                    //
                    // A row that is NOT parked got here by matching one of the stored
                    // statuses the caller named in the same comma set, so it passes
                    // untouched — that is the "honours both" half of a mixed
                    // `--status shelved,approved`. Skipped entirely when the caller named
                    // `needs-attention` itself, because every parked row is then in scope
                    // by its stored status regardless of lens.
                    // trace:BUG-1771 | ai:claude
                    if !parked_lens_keys.is_empty() && lens_widened_needs_attention {
                        reqs.retain(|r| {
                            if !r.status.eq_ignore_ascii_case("NeedsAttention") {
                                return true;
                            }
                            row_parked_lens(backend, &mut lazy_store, r)
                                .is_some_and(|lens| parked_lens_keys.contains(&lens.palette_key()))
                        });
                    }

                    // TASK-1456 (follow-up to BUG-1515): the open lens's status set
                    // excludes `Done` (it sits with Completed/Rejected on the closed
                    // side), but a Done spec whose PR carries a still-live refusal at
                    // its tip is not finished — it needs a rework round, and BUG-1515
                    // already taught `aida queue work`/`next` to read it that way.
                    // Left alone, it simply vanished from the main work list. Re-run
                    // the SAME filter (every other axis unchanged) scoped to `done`
                    // and fold in only the rows `select_done_rework_rows` confirms
                    // have an outstanding refusal, so the fold-in can never disagree
                    // with `queue_fresh_pickup_policy`. Runs before every downstream
                    // lens (parent/focus/meta/standing-type/etc.) so these rows are
                    // scoped identically to the rest of the view.
                    //
                    // Perf (review follow-up): `select_done_rework_rows` resolves the
                    // primary worktree root ONCE for the whole batch, reads local
                    // verdict files only (no git spawn) to narrow to candidates, and
                    // pays for the git tip/ancestry check on that small, capped
                    // candidate set — see its doc comment. `rework_ids` is the set it
                    // computed; every render site below reuses it by plain membership
                    // check instead of recomputing. trace:TASK-1456 | ai:claude
                    let mut rework_ids: std::collections::HashSet<uuid::Uuid> =
                        std::collections::HashSet::new();
                    if default_open_lens {
                        let mut done_filter = filter.clone();
                        done_filter.status =
                            Some(aida_core::RequirementStatus::Done.cache_key().to_string());
                        let done_rows = backend.list_summaries(&done_filter)?;
                        let (rework_rows, ids) =
                            queue_cmd::select_done_rework_rows(done_rows, store_path.parent());
                        let folded_in = !rework_rows.is_empty();
                        reqs.extend(rework_rows);
                        rework_ids = ids;
                        // STORY-1644: keep the folded-in rows in natural ID order
                        // rather than appended at the end. trace:STORY-1644 | ai:claude
                        if folded_in && sort_order == aida_core::SortOrder::IdAsc {
                            aida_core::sort_summaries_natural_id(&mut reqs);
                        }
                    }
                    // BUG-1535: never two rows under one id. A row whose displayed
                    // (agreed) id also names another spec is shown by its native id
                    // instead, with a note. trace:BUG-1535 | ai:claude
                    relabel_ambiguous_list_rows(backend, &mut reqs);

                    // STORY-62: --parent <id> restricts to direct children of <id>.
                    // We don't materialize a parent->children index in the cache;
                    // for one parent it's a single YAML read to grab the
                    // relationships array, which is fast enough for the
                    // interactive `aida list` cadence.
                    // trace:STORY-62 | ai:claude
                    //
                    // TASK-955: --recursive widens this to the WHOLE transitive subtree
                    // (the parent + every descendant via parent->child edges, any depth)
                    // instead of direct children. It reads the materialized hierarchy
                    // edges with a single WITH RECURSIVE query over the cache — no
                    // backend.load() — so the deep-epic view stays as cache-fast as the
                    // direct-children view. clap's `requires = "parent"` guarantees we
                    // only land here with a --parent value. trace:TASK-955 | ai:claude
                    if let Some(parent_ref) = parent {
                        let parent_req = backend
                            .get_requirement_by_spec_id(parent_ref)?
                            .ok_or_else(|| {
                                anyhow::anyhow!("--parent {}: requirement not found", parent_ref)
                            })?;
                        if *recursive {
                            // Full transitive subtree closure from the cache's edge graph.
                            // The set includes the root; the existing list filters
                            // (STATUS / --type / --tags) then apply to the whole subtree.
                            // trace:TASK-955 | ai:claude
                            let subtree_ids = backend.descendant_ids(&parent_req.id)?;
                            reqs.retain(|r| subtree_ids.contains(&r.id));
                        } else {
                            let child_ids: HashSet<Uuid> = parent_req
                                .relationships
                                .iter()
                                .filter(|r| r.rel_type == RelationshipType::Parent)
                                .map(|r| r.target_id)
                                .collect();
                            reqs.retain(|r| child_ids.contains(&r.id));
                        }
                    }

                    // STORY-706: a persistent focus scopes `aida list` to the focused
                    // spec's transitive subtree — the SAME cache-fast closure
                    // `--parent <id> --recursive` uses (`backend.descendant_ids`), with
                    // the focus as the implicit parent. Skipped when an explicit
                    // `--parent` already scopes the view, when `--all` widens to
                    // everything (incl. archived/deferred), or via the focus-specific
                    // `--no-focus` escape. The header below makes the scoping LOUD so a
                    // focused subset is never mistaken for the whole project (the
                    // kubectl-namespace footgun).
                    let mut active_focus = None;
                    if parent.is_none() && !*all && !*no_focus {
                        if let Some(focus_ref) = find_project_root()
                            .ok()
                            .and_then(|root| crate::focus::resolve_focus(&root))
                        {
                            match backend.get_requirement_by_spec_id(&focus_ref)? {
                                Some(focus_req) => {
                                    let subtree_ids = backend.descendant_ids(&focus_req.id)?;
                                    active_focus = Some((focus_req.display_id(), subtree_ids));
                                }
                                None => {
                                    // A focus pointing at a now-missing spec must not
                                    // silently hide everything — warn and widen.
                                    eprintln!(
                                        "{} focus `{}` no longer resolves to a spec; showing all. \
                                     Run `aida focus clear` or re-set it.",
                                        "Note:".yellow(),
                                        focus_ref,
                                    );
                                }
                            }
                        }
                    }

                    // Keep hidden-row footers scoped to focus while computing
                    // the widened count through all the same non-focus lenses.
                    // trace:BUG-2190 | ai:codex
                    let scoped_len = |rows: &[aida_core::RequirementSummary]| {
                        rows.iter()
                            .filter(|r| {
                                active_focus
                                    .as_ref()
                                    .is_none_or(|(_, ids)| ids.contains(&r.id))
                            })
                            .count()
                    };

                    // Hide META reqs (AI prompt customization seeded by init) from
                    // the default view — they're plumbing, not user-authored work.
                    // The user can still see them via `--type meta` (which forces
                    // them into the result regardless of this filter) or by
                    // passing `--include-meta`. trace:BUG-27 | ai:claude
                    let user_asked_for_meta = r#type
                        .as_deref()
                        .map(|t| t.eq_ignore_ascii_case("meta"))
                        .unwrap_or(false);
                    if !*include_meta && !user_asked_for_meta {
                        reqs.retain(|r| !r.req_type.eq_ignore_ascii_case("meta"));
                    }

                    // STORY-1642 item 2: knowledge types (faq, decision, vision, term,
                    // principle — the `is_work_item_type` rule) are reference, not
                    // work. Hidden from the default list the way META is, under every
                    // lens including `--all`; shown for a knowledge `--type`, the
                    // `aida <type> list` alias, or `--include-knowledge`. Runs before
                    // the TASK-773 standing-type pass so the footer count includes
                    // the vision/term/principle rows that pass would also drop.
                    // Item 9: the FAQ listing hides superseded FAQs unless `--all`.
                    // trace:STORY-1642 | ai:claude
                    let before_knowledge_hidden = scoped_len(&reqs);
                    crate::knowledge_list::hide_knowledge_rows(
                        &mut reqs,
                        *include_knowledge,
                        r#type.as_deref(),
                    );
                    let knowledge_hidden = before_knowledge_hidden - scoped_len(&reqs);
                    crate::knowledge_list::hide_superseded_faqs(&mut reqs, r#type.as_deref(), *all);

                    // TASK-773: `aida list` / `aida list open` is a WORK view, so
                    // perpetual standing-artifact types — vision, principle, term,
                    // constraint, folder, meta — are NOT work and are hidden from the
                    // default view (this stops e.g. VIS-1 reading as 'InProgress
                    // work'). They still surface via an explicit `--type <T>` filter
                    // (which overrides the exclusion) and via `--all`. This is
                    // type-based filtering, orthogonal to STORY-584's deferred
                    // view-flag. trace:TASK-773
                    //
                    // STORY-1642: a comma `--type` list (`faq,vision`) opts in every
                    // standing type it names, and a knowledge opt-in
                    // (`--include-knowledge`, or a knowledge `--type`) keeps the
                    // knowledge-class standing rows (vision/term/principle) the
                    // footer promised. trace:STORY-1642 | ai:claude
                    let knowledge_opt_in = crate::knowledge_list::knowledge_opted_in(
                        *include_knowledge,
                        r#type.as_deref(),
                    );
                    let user_asked_for_standing_type = crate::knowledge_list::type_filter_any(
                        r#type.as_deref(),
                        is_standing_artifact_type,
                    );
                    if !*all && !user_asked_for_standing_type {
                        reqs.retain(|r| {
                            !is_standing_artifact_type(&r.req_type)
                                || (knowledge_opt_in
                                    && aida_core::lifecycle::is_knowledge_type_str(&r.req_type))
                        });
                    }

                    // BUG-1498: draft grooming is human-first. Auto-complete failure
                    // records retain their provenance and remain directly reachable
                    // (and batchable) through --machine-drafts or an explicit
                    // --tags auto-drafted filter. Older records that predate the tag
                    // are recognized by their stable description preamble. Apply
                    // after parent/focus/type lenses so the hidden count is local to
                    // exactly the view the advisor requested.
                    let explicitly_asked_for_machine_tag =
                        effective_tags.iter().any(|tag| tag == "auto-drafted");
                    let before_machine_drafts_hidden = scoped_len(&reqs);
                    crate::apply_machine_draft_lens(
                        &mut reqs,
                        exact_draft_view,
                        *machine_drafts,
                        explicitly_asked_for_machine_tag,
                    );
                    let machine_drafts_hidden = before_machine_drafts_hidden - scoped_len(&reqs);

                    // BUG-781: a decision spec (an ADR) sitting at `Approved` is
                    // ACCEPTED — that class's TERMINAL state — so it belongs with the
                    // closed rows the default open lens already hides, not in the open
                    // backlog forever waiting on a manual archive. Applied only under
                    // the same default open lens STORY-723 established: an explicit
                    // `--status`, `--all`, `--archived`, `--deferred`, or a `--type
                    // decision` ask all keep accepted ADRs visible. Sibling of the
                    // TASK-773 standing-artifact pass above; the count feeds the footer
                    // hint so the hidden rows stay one flag away. trace:BUG-781
                    // STORY-1642: a knowledge opt-in (`--include-knowledge`, or a
                    // comma `--type` list naming a knowledge type) keeps accepted
                    // ADRs too. trace:STORY-1642 | ai:claude
                    let user_asked_for_decision_type = knowledge_opt_in
                        || crate::knowledge_list::type_filter_names(r#type.as_deref(), "decision");
                    // BUG-788: the explicit `open` shortcut sets a status filter, which
                    // clears `default_open_lens` — but it is the SAME work view as bare
                    // `aida list`, so it must hide accepted (terminal) ADRs identically.
                    // Fold the two into one lens so the verbs agree; `--type decision`
                    // and `--all` still keep accepted ADRs visible (both bypass this).
                    // trace:BUG-788 | ai:claude
                    let open_work_lens = crate::list_applies_open_work_lens(
                        default_open_lens,
                        explicit_open_alias,
                        *all,
                        *archived,
                        *deferred,
                    );
                    let before_accepted_decisions_hidden = scoped_len(&reqs);
                    crate::hide_accepted_decisions(
                        &mut reqs,
                        open_work_lens,
                        user_asked_for_decision_type,
                    );
                    let accepted_decisions_hidden =
                        before_accepted_decisions_hidden - scoped_len(&reqs);

                    // TASK-900: cap the result at the first N rows AFTER the sort
                    // (list_summaries already ordered by --sort; default is
                    // natural ID order per STORY-1644) and after the parent / meta /
                    // standing-type retain passes — so `--limit N` is the first N of
                    // the fully-filtered set in the chosen order. Applied here, before the --short early
                    // return and the json / table / tree rendering, so every output
                    // shape sees the same bounded row set.
                    // trace:TASK-900 | ai:claude
                    //
                    // TASK-970: in AGENT MODE a bare `aida list` (no explicit
                    // `--limit`/`--all`) gets a DEFAULT row cap so an agent reading
                    // ~900 rows as context doesn't blow its token budget. The cap is
                    // scoped to the default human-readable TABLE render only: explicit
                    // machine shapes (`--short` / `--json`) and the grouped `--tree`
                    // view stay unbounded so existing agent/skill consumers that
                    // enumerate every spec keep working, and the human TTY path is
                    // untouched. An explicit `--limit`/`--all` always overrides.
                    // trace:TASK-970
                    //
                    // STORY-1644: the agent cap is configurable — `--limit`/`--all`
                    // beat env `AIDA_LIST_LIMIT`, which beats `[list] agent_limit`,
                    // which beats the built-in 100 (0 = unbounded). `--limit 0` also
                    // means "no cap". trace:STORY-1644 | ai:claude
                    // The derived driver of every in-progress row (never stored on
                    // the spec), and the `--driver <kind>` filter over it.
                    // trace:STORY-1653 | ai:claude
                    let list_drivers =
                        crate::driver_view::list_driver_map(list_project_root.as_deref(), &reqs);
                    if let Some(want) = driver.as_deref() {
                        crate::driver_view::retain_by_driver(&mut reqs, &list_drivers, want)?;
                    }
                    // Focus count follows every other filter and precedes the
                    // row limit, so it equals the view --no-focus would reveal.
                    // trace:BUG-2190 | ai:codex
                    let mut focus_banner = None;
                    if let Some((label, ids)) = &active_focus {
                        let widened_count = reqs.len();
                        reqs.retain(|r| ids.contains(&r.id));
                        focus_banner = Some(format!(
                            "{}; {} matching specs hidden by focus; run `aida focus clear` to clear",
                            crate::focus::focus_header(label, reqs.len(), widened_count),
                            widened_count - reqs.len(),
                        ).cyan().bold().to_string());
                    }
                    let total_after_filters = reqs.len();
                    let agent_cap = crate::resolve_agent_list_limit(
                        std::env::var(crate::AGENT_LIST_LIMIT_ENV).ok().as_deref(),
                        list_project_root
                            .as_deref()
                            .and_then(crate::read_list_agent_limit),
                    );
                    let agent_default_cap = agent_list_default_cap(
                        *limit,
                        *all,
                        *short,
                        effective_json,
                        *tree,
                        agent_output_mode(),
                        agent_cap,
                    );
                    if let Some(n) = limit.filter(|n| *n > 0).or(agent_default_cap) {
                        reqs.truncate(n);
                    }
                    let rows_truncated = total_after_filters > reqs.len();
                    // STORY-1644: the machine JSON shape is a bare array (STORY-1352
                    // contract) and the agent cap never applies to it, so an explicit
                    // `--limit` cut is announced on stderr instead of reshaping stdout.
                    if effective_json && rows_truncated {
                        eprintln!(
                        "note: truncated: true, total: {total_after_filters} — showing first {} by {sort_key}; `--limit 0` for all",
                        reqs.len()
                    );
                    }

                    // Disclose scope before every renderer's early return.
                    // trace:BUG-2190 | ai:codex
                    if let Some(banner) = &focus_banner {
                        if effective_json || *short {
                            eprintln!("{banner}");
                        } else if agent_output_mode() && !*tree {
                            println!("note: {banner}");
                        } else {
                            println!("{banner}");
                        }
                    }

                    // TASK-743: --short emits one bare canonical spec ID per line —
                    // no header, no count footer, no color, no routing glyphs — so
                    // the output is directly pipeable into `$(...)` / xargs. Runs
                    // AFTER the shared filter + parent + meta passes (same row set
                    // the human table would show) and returns early, before any of
                    // the routing-probe / table / tree / json rendering below.
                    // trace:TASK-743 | ai:claude
                    if *short {
                        for r in &reqs {
                            let id = r.agreed_id.as_deref().or(r.spec_id.as_deref());
                            if let Some(id) = id {
                                println!("{id}");
                            }
                        }
                        return crate::command_registry::returned(Ok(()));
                    }

                    // TASK-670: compute the leading work-routing overlay (queued /
                    // in-flight / blocked markers) once for the visible row set, then map
                    // each row to a glyph. Two cheap reads are always on (a queues
                    // dir scan + a live-lease probe); the blocked axis needs a graph
                    // walk so it's gated behind --blocked. `--no-glyph` short-circuits
                    // ALL of it (restores the pure-cache fast path); `--no-flow` keeps
                    // the cheap probes off too since the column is dropped anyway.
                    // trace:TASK-670 | ai:claude
                    let show_flow = !*no_glyph && !*no_flow;
                    // The routing sets feed BOTH the flow-glyph column AND the JSON
                    // routing fields. The display-suppression flags (`--no-glyph` /
                    // `--no-flow`) hide the COLUMN but must not blank the machine
                    // fields, so compute the cheap probes whenever we render glyphs OR
                    // emit JSON. trace:TASK-670 | ai:claude
                    let need_routing = show_flow || effective_json;
                    // project root = the parent of the `.aida-store` worktree.
                    let routing_root = store_path.parent().map(|p| p.to_path_buf());
                    let queued_ids: HashSet<Uuid> = if need_routing {
                        routing_root
                            .as_deref()
                            .map(all_queued_requirement_ids)
                            .unwrap_or_default()
                    } else {
                        HashSet::new()
                    };
                    let in_flight_scopes: HashSet<String> = if need_routing {
                        routing_root
                            .as_deref()
                            .map(in_flight_lease_scopes)
                            .unwrap_or_default()
                    } else {
                        HashSet::new()
                    };
                    // Blocked axis: only when --blocked. TASK-902: the blocked flag is
                    // now cache-projected ("has an incomplete BlockedBy edge", computed
                    // into the SQLite cache at write/rebuild time), so each summary
                    // already carries `r.blocked` — no full backend.load() over every
                    // object. This was the cockpit board's slowest leg (~1.4s); reading
                    // the cache like --status/--archived cuts it to sub-second.
                    // trace:TASK-902 | ai:claude
                    // Per-row routing classifier. A row is in-flight when a live lease's
                    // scope matches its canonical or origin id (case-insensitive); the
                    // in-flight set is empty unless a live lease exists.
                    let want_blocked = need_routing && *blocked;
                    let row_routing = |r: &aida_core::RequirementSummary| -> (bool, bool, bool) {
                        let queued = queued_ids.contains(&r.id);
                        // Surface the cached blocked flag only when --blocked was passed
                        // (parity with the prior overlay; off by default).
                        let blocked = want_blocked && r.blocked;
                        let in_flight = if in_flight_scopes.is_empty() {
                            false
                        } else {
                            [r.agreed_id.as_deref(), r.spec_id.as_deref()]
                                .into_iter()
                                .flatten()
                                .any(|id| in_flight_scopes.contains(&id.to_ascii_lowercase()))
                        };
                        (in_flight, blocked, queued)
                    };

                    // BUG-783: the archived + deferred view-tier nudges are OFF by
                    // default — an explicit open-work request doesn't need reminding on
                    // every invocation that the other two tiers exist. `[list]
                    // show_hidden_hints = true` in `.aida/config.toml` opts back in.
                    // The lens hints (closed / accepted decisions) are unaffected.
                    // trace:BUG-783 | ai:claude
                    let show_view_tier_hints = find_project_root()
                        .ok()
                        .map(|root| read_list_show_hidden_hints(&root))
                        .unwrap_or(false);
                    // The two tier counts are each an extra cache query. With the nudges
                    // suppressed the only remaining consumer is the BUG-684 empty-state
                    // signpost (which must not tell a filtered-empty project to file its
                    // first spec), so on the hot non-empty path we skip both queries.
                    let need_tier_counts = show_view_tier_hints || reqs.is_empty();

                    // STORY-441: count of archived rows that would have surfaced if
                    // `--all` was set. Cheap second query, used only to render the
                    // "(N archived hidden — pass --all …)" footer hint. Skipped when
                    // the user explicitly asked for archived or all rows.
                    // trace:STORY-441 | ai:claude
                    let archived_hidden_count = if need_tier_counts
                        && matches!(archive, aida_core::ArchiveFilter::NonArchivedOnly)
                    {
                        let archived_filter = aida_core::ListFilter {
                            archive: aida_core::ArchiveFilter::ArchivedOnly,
                            ..filter.clone()
                        };
                        backend.list_summaries(&archived_filter)?.len()
                    } else {
                        0
                    };

                    // STORY-584: parallel count of deferred rows hidden from the default
                    // view, used to render the "(N deferred hidden — pass --deferred …)"
                    // footer nudge so the primed shelf stays discoverable.
                    // trace:STORY-584 | ai:claude
                    let deferred_hidden_count = if need_tier_counts
                        && matches!(defer, aida_core::DeferFilter::NonDeferredOnly)
                    {
                        let deferred_filter = aida_core::ListFilter {
                            defer: aida_core::DeferFilter::DeferredOnly,
                            ..filter.clone()
                        };
                        backend.list_summaries(&deferred_filter)?.len()
                    } else {
                        0
                    };

                    // STORY-723: count of closed (done/completed/rejected) rows the
                    // default OPEN lens is now hiding, so the discoverability footer can
                    // tell the user the rest is one flag away. Only computed when the
                    // default-open lens is actually active. trace:STORY-723 | ai:claude
                    let closed_hidden_count = if default_open_lens {
                        let closed_filter = aida_core::ListFilter {
                            status: Some(
                                aida_core::RequirementStatus::closed_statuses()
                                    .iter()
                                    .map(|s| s.cache_key())
                                    .collect::<Vec<_>>()
                                    .join(","),
                            ),
                            ..filter.clone()
                        };
                        // STORY-1642: knowledge rows (completed FAQs, …) are not
                        // closed *work*; don't count them unless opted in.
                        // trace:STORY-1642 | ai:claude
                        backend
                            .list_summaries(&closed_filter)?
                            .iter()
                            .filter(|r| {
                                knowledge_opt_in
                                    || !aida_core::lifecycle::is_knowledge_type_str(&r.req_type)
                            })
                            .count()
                    } else {
                        0
                    };

                    // Shared footer nudge for both view axes. Each prints a dimmed,
                    // 2-space-indented line only when rows on that axis were hidden.
                    // trace:STORY-441 trace:STORY-584 trace:STORY-723 | ai:claude
                    let print_hidden_hints = || {
                        // BUG-1737: scope disclosure for the count, printed under the
                        // open-work lens whether or not the counted STORY-441 /
                        // STORY-584 tier nudges below are opted in. Same dimmed,
                        // 2-space footer idiom. trace:BUG-1737 | ai:claude
                        if let Some(scope) = crate::list_lens_scope_disclosure(open_work_lens) {
                            println!(
                                "{}",
                                format!("  ({scope} — pass --all to see them)").dimmed()
                            );
                        }
                        for line in list_hidden_hint_lines(
                            show_view_tier_hints,
                            closed_hidden_count,
                            archived_hidden_count,
                            deferred_hidden_count,
                            accepted_decisions_hidden,
                        ) {
                            println!("{}", line.dimmed());
                        }
                        if machine_drafts_hidden > 0 {
                            println!(
                            "{}",
                            format!(
                                "  ({machine_drafts_hidden} machine-filed drafts hidden — pass --machine-drafts to groom them)"
                            )
                            .dimmed()
                        );
                        }
                        // STORY-1642 item 2: say how many knowledge rows were hidden
                        // and how to show them. trace:STORY-1642 | ai:claude
                        if let Some(line) =
                            crate::knowledge_list::knowledge_hidden_hint_line(knowledge_hidden)
                        {
                            println!("{}", line.dimmed());
                        }
                    };

                    // BUG-684: a truly empty listing dead-ends the FIRST command a new
                    // user runs — `No requirements found.` with no next step. Teach the
                    // create move with a soft, forward-pointing signpost (matches the
                    // empty `queue work` tone). Fires only when NOTHING is merely hidden
                    // behind a filter (else the hidden-hints above already point at
                    // `--all` — the specs exist, they're just filtered out).
                    // trace:BUG-684 | ai:claude
                    let print_empty_list_hint = || {
                        // STORY-1642: hidden knowledge rows also mean "the project is
                        // not empty", so they suppress the file-your-first-spec nudge.
                        if let Some(line) = empty_list_hint_line(
                            closed_hidden_count,
                            archived_hidden_count,
                            deferred_hidden_count,
                            accepted_decisions_hidden + knowledge_hidden,
                        ) {
                            println!(
                                "{} {}",
                                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                                line.dimmed()
                            );
                        }
                    };

                    // BUG-1520: validate `--fields` BEFORE the format branch, so no
                    // format can skip the check. Previously only the human and TOON
                    // paths validated; `--format json` returned full rows with exit
                    // code 0 for a bogus field, and that is the one surface machine
                    // consumers read. One hoisted call makes the three agree by
                    // construction rather than by three copies of it.
                    // trace:BUG-1520 | ai:claude
                    let selected_fields = toon_list_fields(fields.as_deref())?;

                    // STORY-244: internal JSON output for the TUI launcher's
                    // Backlog / History panes. Hidden from --help; schema is
                    // internal and may change. Emits the row set straight as
                    // `[{spec_id,title,req_type,status,tags}]` without the
                    // human chrome (banner / footer count / colours).
                    // trace:STORY-244 | ai:claude
                    if effective_json {
                        // TASK-670: carry the work-routing axis as machine fields
                        // (glyphs are display-only). `blocked` reflects the graph walk
                        // only when --blocked was passed; otherwise it's always false.
                        // trace:TASK-670 | ai:claude
                        #[derive(serde::Serialize)]
                        struct ListJsonRow<'a> {
                            spec_id: &'a str,
                            title: &'a str,
                            req_type: &'a str,
                            // BUG-1520: `type` is what `--fields`, the human header and
                            // the TOON header all call this, so a consumer reading the
                            // documented key off a JSON row got null. Emitted ALONGSIDE
                            // `req_type`, never instead of it — STORY-1352 declared this
                            // a versioned read-only monitor contract, and a rename is
                            // the exact silent break that contract exists to prevent.
                            // Retiring `req_type` is a later deprecation, not this fix.
                            // trace:BUG-1520 | ai:claude
                            r#type: &'a str,
                            status: &'a str,
                            // STORY-1023: richer display label for stored
                            // NeedsAttention rows, e.g. "Shelved (ci-red)" or
                            // "Needs Decision (design-fork)". `status` stays the
                            // cache/status token for existing machine consumers.
                            #[serde(skip_serializing_if = "Option::is_none")]
                            status_label: Option<String>,
                            #[serde(skip_serializing_if = "Option::is_none")]
                            status_lens: Option<&'static str>,
                            tags: &'a [String],
                            queued: bool,
                            in_flight: bool,
                            blocked: bool,
                            // STORY-639: assignee carried as a machine field; omitted
                            // (None) when unassigned. trace:STORY-639 | ai:claude
                            #[serde(skip_serializing_if = "Option::is_none")]
                            assignee: Option<&'a str>,
                            // STORY-703: the revisit trigger of a deferred spec, so the
                            // cockpit's advisor-backlog panel can surface WHY each parked
                            // item is parked inline. Omitted (None) when not deferred.
                            // trace:STORY-703 | ai:claude
                            #[serde(skip_serializing_if = "Option::is_none")]
                            deferred_until: Option<&'a str>,
                            // TASK-1231: the groomed execution mode, so an unattended
                            // selector (`aida autoprogress`) can fence the supervised
                            // modes (drive/guided/operator/decide) it must never
                            // headless-drive. Omitted (None) when ungroomed/unset —
                            // unset stays drainable. trace:TASK-1231 | ai:claude
                            #[serde(skip_serializing_if = "Option::is_none")]
                            execution_mode: Option<&'a str>,
                            // The derived driver of an in-progress row, e.g.
                            // `relay:lib-split` / `idle 26h`, and its kind (relay /
                            // drain / agent / queued / idle). Omitted otherwise.
                            // trace:STORY-1653 | ai:claude
                            #[serde(skip_serializing_if = "Option::is_none")]
                            driver: Option<String>,
                            #[serde(skip_serializing_if = "Option::is_none")]
                            driver_kind: Option<&'static str>,
                        }
                        let out: Vec<ListJsonRow> = reqs
                            .iter()
                            .map(|r| {
                                let (in_flight, blocked, queued) = row_routing(r);
                                // trace:BUG-1771 | ai:claude — one lens computation
                                // shared with the `--status shelved` filter.
                                let parked_lens = row_parked_lens(backend, &mut lazy_store, r);
                                // TASK-1456: a Done row folded in by
                                // `select_done_rework_rows` still carries `status:
                                // "Done"` — the machine-consumer contract that field
                                // is (STORY-1352) — so the annotation goes in the
                                // SAME status_label/status_lens channel NeedsAttention
                                // rows already use, not a mutated status. Perf: a
                                // plain `rework_ids` membership check — the set was
                                // already computed once above, no recomputation.
                                // trace:TASK-1456 | ai:claude
                                let rework = rework_ids.contains(&r.id);
                                let (status_label, status_lens) =
                                    list_json_status_label_lens(rework, parked_lens.as_ref());
                                ListJsonRow {
                                    spec_id: r
                                        .agreed_id
                                        .as_deref()
                                        .or(r.spec_id.as_deref())
                                        .unwrap_or(""),
                                    title: r.title.as_str(),
                                    req_type: r.req_type.as_str(),
                                    r#type: r.req_type.as_str(),
                                    status: r.status.as_str(),
                                    status_label,
                                    status_lens,
                                    tags: &r.tags,
                                    queued,
                                    in_flight,
                                    blocked,
                                    assignee: r.assignee.as_deref(),
                                    deferred_until: r.deferred_until.as_deref(),
                                    execution_mode: r.execution_mode.as_deref(),
                                    driver: list_drivers.get(&r.id).map(|d| d.label()),
                                    driver_kind: list_drivers.get(&r.id).map(|d| d.kind()),
                                }
                            })
                            .collect();
                        // BUG-1520 criterion 2: `--fields` NARROWS the JSON payload,
                        // selecting the same field set TOON selects.
                        //
                        // Parity with TOON is about WHICH fields come back, not how
                        // they are rendered. The narrowed row reuses the full row's own
                        // values, so `--fields` can never silently change data — a
                        // consumer that narrows gets a strict SUBSET of what it would
                        // have got unnarrowed, with booleans still booleans and `tags`
                        // still an array. Rendering through TOON's display tokens
                        // instead would have lowercased `req_type` ("Task" -> "task")
                        // only when narrowed, which is a sharper version of the very
                        // defect this fixes.
                        //
                        // Fields TOON accepts that the JSON row has no key for
                        // (priority, feature, owner, heft, weight, modified_at, origin)
                        // fall back to the shared cell renderer, so a valid `--fields`
                        // entry is never silently dropped.
                        // trace:BUG-1520 | ai:claude
                        if fields.is_some() {
                            let narrowed: Vec<serde_json::Value> = out
                                .iter()
                                .zip(reqs.iter())
                                .map(|(full_row, r)| {
                                    let routing = row_routing(r);
                                    let full = serde_json::to_value(full_row)
                                        .unwrap_or(serde_json::Value::Null);
                                    let mut row = serde_json::Map::new();
                                    for f in &selected_fields {
                                        // `type` is carried under both spellings so a
                                        // narrowed row's keys stay a subset of the full
                                        // row's rather than a different vocabulary.
                                        let keys: Vec<&str> = match f.as_str() {
                                            "id" => vec!["spec_id"],
                                            "type" => vec!["req_type", "type"],
                                            "mode" => vec!["execution_mode"],
                                            other => vec![other],
                                        };
                                        let mut hit = false;
                                        for key in &keys {
                                            if let Some(v) = full.get(*key) {
                                                row.insert((*key).to_string(), v.clone());
                                                hit = true;
                                            }
                                        }
                                        if !hit {
                                            row.insert(
                                                f.clone(),
                                                serde_json::Value::String(toon_list_cell(
                                                    r, routing, f,
                                                )),
                                            );
                                        }
                                    }
                                    serde_json::Value::Object(row)
                                })
                                .collect();
                            println!("{}", crate::cache_output::json_pretty(&narrowed)?);
                            return crate::command_registry::returned(Ok(()));
                        }
                        println!("{}", crate::cache_output::json_pretty(&out)?);
                        return crate::command_registry::returned(Ok(()));
                    }

                    // TASK-964: AGENT-MODE token-efficient TOON render. Replaces the
                    // emoji/table default for non-interactive agent callers (non-TTY or
                    // AIDA_AGENT_OUTPUT). The human TTY path drops through to the
                    // byte-identical table below. The `--tree` grouped view keeps its own
                    // shape (it's a structured rollup, not a flat row set). A leading
                    // `count: N of M` header states the bounded slice; `--fields` widens
                    // the minimal id/title/status/type schema. trace:TASK-964
                    if agent_output_mode() && !*tree {
                        // trace:BUG-1520 | ai:claude — one validation, hoisted above.
                        // STORY-1642 item 9: the FAQ table is `id` + question only.
                        // trace:STORY-1642 | ai:claude
                        let selected = match crate::knowledge_list::faq_default_fields(
                            r#type.as_deref(),
                            fields.as_deref(),
                        ) {
                            Some(faq_fields) => toon_list_fields(Some(faq_fields))?,
                            None => selected_fields.clone(),
                        };
                        // trace:STORY-1653 | ai:claude — `driver`, `driver_kind`
                        let selected = crate::driver_view::with_driver_columns(
                            selected,
                            fields.is_some(),
                            &list_drivers,
                        );
                        let rows: Vec<Vec<String>> = reqs
                            .iter()
                            .map(|r| {
                                let routing = row_routing(r);
                                selected
                                    .iter()
                                    .map(|f| {
                                        crate::driver_view::list_cell(&list_drivers, r, f)
                                            .unwrap_or_else(|| toon_list_cell(r, routing, f))
                                    })
                                    .collect()
                            })
                            .collect();
                        // STORY-723: label the denominator so it reconciles with the
                        // other surfaces. The default `aida list` is now the OPEN lens,
                        // so `M` counts open (active, non-archived) specs — distinct from
                        // `aida status`'s `total` (every active spec, all statuses). An
                        // explicit status filter relabels to plain "matched".
                        // trace:STORY-723 | ai:claude
                        let denom_label = list_count_denom_label(default_open_lens);
                        // STORY-1644: truncation is never silent — when rows were cut
                        // the header names the order, what was hidden, and how to
                        // see it. trace:STORY-1644 | ai:claude
                        if rows_truncated {
                            println!(
                                "{}",
                                crate::agent_list_truncation_header(
                                    reqs.len(),
                                    total_after_filters,
                                    denom_label,
                                    &sort_key,
                                )
                            );
                        } else {
                            println!(
                                "count: {} of {} {}",
                                reqs.len(),
                                total_after_filters,
                                denom_label
                            );
                        }
                        let field_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
                        println!("{}", crate::toon::table_raw("specs", &field_refs, &rows));
                        if agent_default_cap.is_some() && rows_truncated {
                            println!(
                            "note: agent default cap {agent_cap} — set [list] agent_limit or {} (0 = unbounded), or pass `--all`",
                            crate::AGENT_LIST_LIMIT_ENV
                        );
                        }
                        // BUG-1737: the notes below enumerate what this view withheld,
                        // which made them read as the complete account — they were not.
                        // Lead with the two tiers that had no line at all, in the same
                        // position the human footer gives it (immediately under the
                        // count), so both surfaces read alike. Static text: no extra
                        // backend query on the agent hot path. trace:BUG-1737 | ai:claude
                        if let Some(scope) = crate::list_lens_scope_disclosure(open_work_lens) {
                            println!("note: {scope} — `aida list --all` for every spec");
                        }
                        // STORY-723: tell the agent the closed history exists but is
                        // hidden behind the default open lens. trace:STORY-723
                        if closed_hidden_count > 0 {
                            println!(
                            "note: {closed_hidden_count} closed hidden (open lens) — `aida list --all` or `--status closed`"
                        );
                        }
                        // BUG-781: same for the terminal (accepted) decisions the open
                        // lens drops — an agent must not conclude they were deleted.
                        if accepted_decisions_hidden > 0 {
                            println!(
                            "note: {accepted_decisions_hidden} accepted decisions hidden (terminal) — `aida list --type decision`"
                        );
                        }
                        if machine_drafts_hidden > 0 {
                            println!(
                            "note: {machine_drafts_hidden} machine-filed drafts hidden — `aida list --status draft --machine-drafts`"
                        );
                        }
                        // trace:STORY-1642 | ai:claude
                        if let Some(note) =
                            crate::knowledge_list::knowledge_hidden_agent_note(knowledge_hidden)
                        {
                            println!("{note}");
                        }
                        // TASK-1456: the agent-mode row for a folded-in Done+refusal
                        // spec renders `status: Done`, same as an ordinary
                        // awaiting-merge row — flag it explicitly so an agent
                        // consuming this table doesn't read it as merge-ready. Perf:
                        // `rework_ids` membership only — the set was already computed
                        // once above, no recomputation. trace:TASK-1456 | ai:claude
                        for r in &reqs {
                            if !rework_ids.contains(&r.id) {
                                continue;
                            }
                            let display_id = r
                                .agreed_id
                                .as_deref()
                                .or(r.spec_id.as_deref())
                                .unwrap_or("?");
                            println!(
                                "note: {display_id} needs rework — reviewer requested changes; \
                             `aida queue rework {display_id}`"
                            );
                        }
                        // TASK-974 (AXI #9): trailing next-step block — drill into a row
                        // (placeholder id, so no concrete spec id is echoed twice into
                        // the id stream) and carry the active status filter forward into
                        // the valid next transition for that state. trace:TASK-974
                        let next = crate::help_next::list_next(&crate::help_next::ListContext {
                            status: status.as_deref(),
                            // BUG-684: an empty result routes the next[] block to `aida
                            // add` instead of the nonsensical `aida show <id>`.
                            is_empty: reqs.is_empty(),
                        });
                        if let Some(block) = crate::help_next::render(&next) {
                            println!("{block}");
                        }
                        return crate::command_registry::returned(Ok(()));
                    }

                    // TASK-970: in AGENT MODE, when the default cap trimmed the table,
                    // lead with a `count: N of M` header + a widen hint so the agent
                    // knows it's seeing a bounded slice and how to widen. Only fires on
                    // the default table path (agent_default_cap is None for explicit
                    // shapes and when nothing was trimmed). trace:TASK-970
                    if agent_default_cap.is_some() && rows_truncated {
                        // trace:STORY-1644 | ai:claude
                        println!(
                            "{}",
                            crate::agent_list_truncation_header(
                                reqs.len(),
                                total_after_filters,
                                list_count_denom_label(default_open_lens),
                                &sort_key,
                            )
                        );
                    }

                    // TASK-568: --tree groups the (already-filtered) listing by
                    // parent EPIC for visual clustering, mirroring the shape of
                    // `aida queue list --tree`. Children indent under their EPIC,
                    // groups sort by item count desc then name, and requirements
                    // with no EPIC parent fall into a final "Unscoped" group.
                    // Summaries don't carry relationships (the cache is a flat
                    // projection), so we load the full store once to resolve each
                    // row's parent epic via derive_parent_epic_label — the same
                    // opt-in YAML-read tradeoff `aida show --tree` accepts.
                    // trace:TASK-568 | ai:claude
                    if *tree {
                        if reqs.is_empty() {
                            println!("No requirements found.");
                            print_empty_human_list_truncation_footer(
                                total_after_filters,
                                *limit,
                                default_open_lens,
                            );
                            print_hidden_hints();
                            print_empty_list_hint();
                            return crate::command_registry::returned(Ok(()));
                        }
                        let store = backend.load()?;
                        let by_uuid: std::collections::HashMap<Uuid, &aida_core::Requirement> =
                            store.requirements.iter().map(|r| (r.id, r)).collect();

                        use std::collections::BTreeMap;
                        let unscoped_key = "~unscoped".to_string();
                        let mut groups: BTreeMap<String, Vec<&aida_core::RequirementSummary>> =
                            BTreeMap::new();
                        for summary in &reqs {
                            let key = by_uuid
                                .get(&summary.id)
                                .and_then(|req| derive_parent_epic_label(req, &store))
                                .unwrap_or_else(|| unscoped_key.clone());
                            groups.entry(key).or_default().push(summary);
                        }

                        // BUG-659: a requirement used as a real parent-backed group header
                        // (its display id is a group key) must NOT also appear as an
                        // ordinary row under Unscoped — it is already represented at the
                        // parent/group level. Drop such rows from the Unscoped bucket, and
                        // drop the bucket entirely if it empties. trace:BUG-659 | ai:claude
                        {
                            let header_ids: std::collections::HashSet<String> = groups
                                .keys()
                                .filter(|k| k.as_str() != unscoped_key.as_str())
                                .cloned()
                                .collect();
                            if let Some(unscoped) = groups.get_mut(&unscoped_key) {
                                unscoped.retain(|s| {
                                    let id = s
                                        .agreed_id
                                        .as_deref()
                                        .or(s.spec_id.as_deref())
                                        .unwrap_or("");
                                    !header_ids.contains(id)
                                });
                            }
                            if groups.get(&unscoped_key).is_some_and(|v| v.is_empty()) {
                                groups.remove(&unscoped_key);
                            }
                        }

                        // Sort groups: real EPICs by count desc then name; unscoped last.
                        let mut ordered: Vec<(String, Vec<&aida_core::RequirementSummary>)> =
                            groups.into_iter().collect();
                        ordered.sort_by(|a, b| {
                            let a_unscoped = a.0 == unscoped_key;
                            let b_unscoped = b.0 == unscoped_key;
                            match (a_unscoped, b_unscoped) {
                                (true, false) => std::cmp::Ordering::Greater,
                                (false, true) => std::cmp::Ordering::Less,
                                _ => b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)),
                            }
                        });

                        // BUG: a real parent-backed group header used to print only the
                        // parent's id + child count (`EPIC-56 (5 items)`), making a real
                        // scoped parent indistinguishable from a synthetic bucket. Resolve
                        // each parent label back to its requirement so the header can carry
                        // the parent's own title + status badge; the child count becomes
                        // trailing metadata. The label is the parent's canonical display id
                        // (agreed_id else spec_id) — the same key derive_parent_epic_label
                        // groups on — so look it up by that. trace:TASK-0439 | ai:claude
                        let parent_by_label: std::collections::HashMap<
                            &str,
                            &aida_core::Requirement,
                        > = store
                            .requirements
                            .iter()
                            .filter_map(|p| {
                                p.agreed_id
                                    .as_deref()
                                    .or(p.spec_id.as_deref())
                                    .map(|id| (id, p))
                            })
                            .collect();
                        for (key, group) in &ordered {
                            println!();
                            println!(
                                "{}",
                                tree_group_header(
                                    key,
                                    &unscoped_key,
                                    group.len(),
                                    parent_by_label.get(key.as_str()).copied(),
                                    &store,
                                )
                            );
                            let id_col_width = group
                                .iter()
                                .map(|r| {
                                    r.agreed_id
                                        .as_deref()
                                        .or(r.spec_id.as_deref())
                                        .unwrap_or("???")
                                        .len()
                                })
                                .max()
                                .unwrap_or(0);
                            for (i, summary) in group.iter().enumerate() {
                                let is_last = i + 1 == group.len();
                                let display_id = summary
                                    .agreed_id
                                    .as_deref()
                                    .or(summary.spec_id.as_deref())
                                    .unwrap_or("???");
                                // BUG-781: an accepted ADR badges as terminal `Accepted`,
                                // not the task-style `Approved`. trace:BUG-781
                                let status_badge = status_display::status_badge(
                                    status_display::display_status_for_type(
                                        &summary.req_type,
                                        &summary.status,
                                    ),
                                );
                                let glyph = if is_last { "└─" } else { "├─" };
                                let pad = " ".repeat(id_col_width.saturating_sub(display_id.len()));
                                let tag_set: std::collections::HashSet<String> =
                                    summary.tags.iter().cloned().collect();
                                let tag_chip = format_tag_chip(&tag_set)
                                    .map(|c| format!("  {}", format!("[{}]", c).dimmed()))
                                    .unwrap_or_default();
                                println!(
                                    "  {} {}{}  {}  [{}]{}",
                                    glyph.dimmed(),
                                    display_id.bold(),
                                    pad,
                                    summary.title,
                                    status_badge,
                                    tag_chip,
                                );
                            }
                        }
                        println!(
                            "\n{}",
                            list_human_count_footer(
                                reqs.len(),
                                total_after_filters,
                                *limit,
                                default_open_lens,
                            )
                        );
                        print_hidden_hints();
                        print_deferred_triggers(*deferred, &reqs);
                        print_rework_needed_notes(&reqs, &rework_ids);
                        maybe_print_whats_left_tip(status.as_deref(), &reqs);
                        return crate::command_registry::returned(Ok(()));
                    }

                    // STORY-734: `--fields <csv>` selects AND orders the columns of the
                    // HUMAN table too (was agent-mode-only — the operator-reported gap).
                    // Validate the field set (an unknown name errors with the valid set),
                    // then render the dynamic column table and skip the fixed layouts
                    // below. Default (no `--fields`) drops straight through, unchanged.
                    // trace:STORY-734 | ai:claude
                    // STORY-1642 item 9: the FAQ table defaults to `id,title` through
                    // this same path. trace:STORY-1642 | ai:claude
                    let human_fields =
                        fields
                            .as_deref()
                            .or(crate::knowledge_list::faq_default_fields(
                                r#type.as_deref(),
                                fields.as_deref(),
                            ));
                    if let Some(csv) = human_fields {
                        let selected = toon_list_fields(Some(csv))?;
                        if reqs.is_empty() {
                            println!("No requirements found.");
                            print_empty_human_list_truncation_footer(
                                total_after_filters,
                                *limit,
                                default_open_lens,
                            );
                            print_hidden_hints();
                            print_empty_list_hint();
                        } else {
                            render_list_fields_table(&reqs, &selected, &row_routing);
                            println!(
                                "\n{}",
                                list_human_count_footer(
                                    reqs.len(),
                                    total_after_filters,
                                    *limit,
                                    default_open_lens,
                                )
                            );
                            print_hidden_hints();
                            print_deferred_triggers(*deferred, &reqs);
                            print_rework_needed_notes(&reqs, &rework_ids);
                            maybe_print_whats_left_tip(status.as_deref(), &reqs);
                        }
                        return crate::command_registry::returned(Ok(()));
                    }

                    if reqs.is_empty() {
                        println!("No requirements found.");
                        print_empty_human_list_truncation_footer(
                            total_after_filters,
                            *limit,
                            default_open_lens,
                        );
                        print_hidden_hints();
                        print_empty_list_hint();
                    } else {
                        // Default rendering: one ID column (canonical = agreed_id
                        // when present, else spec_id). Pass --show-origin to
                        // surface the original spec_id alongside as "Origin ID".
                        // Replaces the older two-column-by-default layout where
                        // both columns were FR-NNN-shaped and confusing to grep
                        // against. trace:FR-1-070 | ai:claude
                        // TASK-569: --show-tags adds a "Tags" column right of
                        // Title. Title is truncated to a fixed width so the tag
                        // chips have room without breaking word-wrap on narrow
                        // terminals; chip set itself is truncated to 3 with a
                        // "+N more" suffix. trace:TASK-569 | ai:claude
                        // TASK-670: the leading work-routing column. `flow_prefix` is
                        // "<glyph> " (2 visible cols) per row when the column is shown,
                        // else "". `flow_header` reserves the same 2 cols in the header
                        // + divider so the ID column stays aligned. `render_status`
                        // drops the status glyph under --no-glyph for plain-text output.
                        // trace:TASK-670 | ai:claude
                        let flow_header = if show_flow
                            && crate::glyphs::active_profile(
                                crate::find_project_root().ok().as_deref(),
                            ) == crate::glyphs::GlyphProfile::Ascii
                        {
                            "   "
                        } else if show_flow {
                            "  "
                        } else {
                            ""
                        };
                        let options = ListTableOptions {
                            show_origin: *show_origin,
                            show_tags: *show_tags,
                            no_glyph: *no_glyph,
                            flow_header: flow_header.to_string(),
                            terminal_width: terminal_list_width(),
                            // trace:STORY-1653 | ai:claude
                            drivers: list_drivers.iter().map(|(k, d)| (*k, d.label())).collect(),
                        };
                        for line in render_list_table(&reqs, &options, row_routing) {
                            println!("{line}");
                        }
                        println!(
                            "\n{}",
                            list_human_count_footer(
                                reqs.len(),
                                total_after_filters,
                                *limit,
                                default_open_lens,
                            )
                        );
                        print_hidden_hints();
                        print_deferred_triggers(*deferred, &reqs);
                        print_rework_needed_notes(&reqs, &rework_ids);
                        maybe_print_whats_left_tip(status.as_deref(), &reqs);
                    }
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                ListArgs {
                    status,
                    priority,
                    r#type,
                    feature,
                    tags,
                    json,
                    ..
                } => {
                    // Legacy SQLite path doesn't honor role scope (deprecated
                    // backend). STORY-244 `--json` is git-backend only — the
                    // legacy path stays human-only since `aida init --centralized`
                    // is deprecated. trace:STORY-244 | ai:claude
                    if *json {
                        anyhow::bail!(
                            "`aida list --json` requires the default git-canonical \
                         backend; the deprecated --centralized SQLite mode does \
                         not emit JSON. Run `aida init` (no flags) on a new \
                         project, or upgrade an existing one."
                        );
                    }
                    list_requirements(&storage, status, priority, r#type, feature, tags)?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static LIST: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "list",
    help_after: "fasttrack",
    command: <ListArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Read,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Store(run_list),
};

/// Show details for a specific requirement
#[derive(clap::Parser, Debug)]
#[clap(name = "show")]
pub(crate) struct ShowArgs {
    /// The ID of the requirement to show
    pub id: String,

    /// Print comment bodies inline after the description (instead of just
    /// the count). Equivalent to following up with `aida comment list <ID>`.
    #[clap(long, short = 'c', conflicts_with = "no_comments")]
    pub comments: bool,

    /// Suppress comment bodies (enabled by default for human output).
    #[clap(long, conflicts_with = "comments")]
    pub no_comments: bool,

    /// Render an indented hierarchy of <id> and its descendants instead
    /// of the standard detail view. Each row shows status + title.
    // trace:STORY-62 | ai:claude
    #[clap(long)]
    pub tree: bool,

    /// Depth limit for `--tree` (default 3). Only meaningful with
    /// `--tree`.
    // trace:STORY-62 | ai:claude
    #[clap(long, default_value = "3", value_name = "N")]
    pub depth: usize,

    /// Pull from `origin/aida-store` before reading the requirement.
    /// See `aida list --sync`.
    // trace:STORY-78 | ai:claude
    #[clap(long)]
    pub sync: bool,

    /// Skip the git-linkage section (commits / files traced /
    /// branch / PR). Faster for read-only contexts that only need
    /// the requirement fields.
    // trace:TASK-241 | ai:claude
    #[clap(long)]
    pub no_git: bool,

    /// Expand the git-linkage section: every referencing commit
    /// (not just the most recent 5) plus per-commit diff stats.
    // trace:TASK-241 | ai:claude
    #[clap(long, short = 'v')]
    pub verbose: bool,

    /// Render a compact, boxed "spec card" instead of the linear
    /// detail view: a header rule, an ID/type/priority/status
    /// one-liner, key fields, a truncated description, the
    /// acceptance criteria, and the git-linkage summary. The
    /// /aida-pickup skill renders this at session start so the
    /// spec's contract stays in terminal scrollback.
    #[clap(long)]
    pub card: bool,

    /// With --card: minimal density — a single-line
    /// id/type/priority/status/title summary, no box. For
    /// autonomous or scripted flows that just need the spec named.
    #[clap(long, conflicts_with = "full")]
    pub brief: bool,

    /// With --card: full density — the complete description with
    /// no paragraph truncation. For deep dives.
    #[clap(long)]
    pub full: bool,

    // trace:TASK-102 | ai:claude
    /// Enumerate every relationship edge inline (direction + type +
    /// target), regardless of count. Alias `--relations`. Without it,
    /// `aida show` lists edges only when there are few (≤5) and otherwise
    /// prints a count + a pointer to `aida rel list`.
    #[clap(long = "rels", visible_alias = "relations")]
    pub rels: bool,

    /// Emit the requirement as a JSON object instead of the human detail
    /// view. Includes the deterministic in/out-degree centrality + heft
    /// score, typed relationships, and `git_linkage` (commits, traced
    /// files, shipped/branch/worktree/PR/repo fields). `git_linkage` is
    /// null with --no-git. Same document as `--format json`.
    // trace:STORY-632 | ai:claude — plain `//` keeps the marker out of `--help`.
    // trace:BUG-1502 | ai:codex
    #[clap(long, conflicts_with_all = ["tree", "card"])]
    pub json: bool,

    /// Print the description and comments as raw markdown source instead
    /// of rendering headings, emphasis, lists and tables for the terminal.
    /// Rendering only happens in human output at an interactive terminal;
    /// piped, TOON and JSON output are always raw. `AIDA_MARKDOWN=raw`
    /// sets this for every invocation.
    // trace:STORY-1652 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(long)]
    pub raw: bool,
}

fn run_show(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <ShowArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                ShowArgs {
                    id,
                    comments,
                    no_comments,
                    tree,
                    depth,
                    sync,
                    no_git,
                    verbose,
                    card,
                    brief,
                    full,
                    rels,
                    json,
                    raw,
                } => {
                    // trace:TASK-518 | ai:antigravity
                    let mut resolved_id = id.clone();
                    if let Some(pr) = parse_pr_arg(id) {
                        let store = backend.load()?;
                        let project_root = store_path.parent().ok_or_else(|| {
                            anyhow::anyhow!("cannot derive project root from store path")
                        })?;
                        // TASK-888: a bare number (no `pr`/`#` prefix) routes to PR
                        // lookup, so a user expecting a spec lookup gets a PR-flavored
                        // error with no clue why. When the arg is a prefixless digit
                        // string, append a hint that spec ids carry a TYPE- prefix.
                        // trace:TASK-888 | ai:claude
                        let bare_number = id.trim().chars().all(|c| c.is_ascii_digit());
                        let resolved =
                            resolve_pr_to_spec(project_root, pr, &store).map_err(|e| {
                                if bare_number {
                                    anyhow::anyhow!(
                                "{e}\n  note: spec ids look like TASK-12 — did you mean a spec id? \
                                 bare numbers are treated as PR lookups."
                            )
                                } else {
                                    e
                                }
                            })?;
                        println!("showing {} (backs {})", resolved, id);
                        resolved_id = resolved;
                    }
                    let id = &resolved_id;

                    // STORY-78: opt-in sync-pull before show. trace:STORY-78 | ai:claude
                    if *sync {
                        maybe_sync_pull(store_path)?;
                    }
                    // BUG-68: record activity only AFTER the lookup succeeds, so
                    // a typo'd `aida show STORY-99` doesn't leave a phantom
                    // STORY-99 in the activity log (which would then surface
                    // as @SPEC in statusline). Each lookup branch below
                    // records the canonical spec_id from the resolved req.
                    // trace:BUG-68 | ai:claude
                    // STORY-62: --tree replaces the detail view with an indented
                    // hierarchy walk. Children are read via rel_type:Parent edges
                    // on the parent's record; recursion descends until depth is
                    // exhausted or no more children. Hidden tradeoff: each level
                    // hits one more YAML read per visited node, but for typical
                    // EPIC trees (~50 nodes max) that's fine, and the user opts
                    // in. trace:STORY-62 | ai:claude
                    // BUG-97: attach the parse-failure recovery hint to ANY error
                    // from the lookup. Previously the swallowed-error path returned
                    // None for both "file missing" AND "file failed to parse",
                    // sending the user down a wrong-spec-id chase. git_backend's
                    // get_requirement_by_spec_id now propagates parse errors;
                    // wrap them here with the actionable hint. trace:BUG-97
                    // BUG-599: a malformed id the user typed (not TYPE-SEQ /
                    // TYPE-NODE-SEQ) is a typo, not an on-disk parse failure — give a
                    // friendly format hint instead of the version-mismatch/rebuild wall
                    // (which `parse_failure_hint` is reserved for below). trace:BUG-599
                    if !is_spec_id_or_uuid(id) {
                        return crate::command_registry::returned(Err(
                            not_found::invalid_spec_id_format(id),
                        ));
                    }
                    // The id is well-formed, so any Err here is a genuine on-disk parse
                    // failure (binary/version skew) worth the rebuild hint. A
                    // well-formed-but-absent id comes back as Ok(None) and is handled
                    // as a plain not-found below. trace:BUG-97 trace:BUG-599
                    // BUG-1535: an ambiguous id is refused (non-zero exit, every
                    // candidate listed) — never answered with one of them — and that
                    // refusal passes through unwrapped. trace:BUG-1535 | ai:claude
                    // Read-only: the tolerant resolver keeps `show` off the cache
                    // write lock. trace:BUG-1670 | ai:claude
                    let lookup_phase = crate::usage::phase("lookup");
                    let lookup = backend
                        .get_requirement_unambiguous_for_read(id)
                        .map_err(|e| {
                            if e.downcast_ref::<aida_core::id_collisions::AmbiguousIdError>()
                                .is_some()
                            {
                                return e;
                            }
                            anyhow::anyhow!(
                                "Parse failed: {}\n  Detail: {:#}\n{}",
                                id,
                                e,
                                aida_core::object_store::parse_failure_hint(None),
                            )
                        });
                    drop(lookup_phase);
                    if *tree {
                        match lookup? {
                            Some(root) => {
                                record_role_activity(root.spec_id.as_deref().unwrap_or(id), "show");
                                render_tree(backend, &root, *depth)?;
                            }
                            None => {
                                // BUG-600: a not-found lookup is a failure — return the
                                // error (exit non-zero) like edit/defer/archive, don't
                                // print-and-exit-0. trace:BUG-600 | ai:claude
                                return crate::command_registry::returned(Err(
                                    not_found::requirement_not_found(id, Some(store_path)),
                                ));
                            }
                        }
                        return crate::command_registry::returned(Ok(()));
                    }
                    match lookup? {
                        Some(req) => {
                            // Share the fallback index across relationships and blockers.
                            // trace:BUG-1678 | ai:codex
                            let read_target = backend.requirement_reader();
                            record_role_activity(req.spec_id.as_deref().unwrap_or(id), "show");
                            // STORY-632: deterministic local graph-centrality, read from
                            // the cache (recomputed on rebuild from the relationship
                            // graph; never stored in YAML). trace:STORY-632 | ai:claude
                            let context_phase = crate::usage::phase("context");
                            let degrees = backend.degrees(&req.id).unwrap_or_default();
                            let (cached_epic_status, serialize_cluster_command) =
                                show_cached_context(backend, &req);
                            drop(context_phase);
                            // BUG-626: the cache projects an EPIC's read-only child
                            // rollup. Using that projection avoids loading every YAML
                            // object merely to render one spec. trace:BUG-626
                            let effective_status_str = cached_epic_status
                                .unwrap_or_else(|| format!("{}", req.effective_status()));
                            // BUG-781: JSON is the machine projection of the same
                            // human detail view, so use the type-aware display label
                            // (a stored Approved decision is displayed as Accepted).
                            // Keep the stored status separately below; consumers must
                            // not have to infer which value is presentation-only.
                            // trace:BUG-1502 | ai:codex
                            let display_status = status_display::display_status_for_type(
                                &format!("{:?}", req.req_type),
                                &effective_status_str,
                            )
                            .to_string();
                            // BUG-1767: the displayed (effective) status can be a
                            // derived rollup that disagrees with the stored field.
                            // Compute the stored value and the disclosure once; every
                            // render surface below must not present the derived value
                            // as if it were stored, and must not prescribe `archive`
                            // while the stored status is non-terminal.
                            // trace:BUG-1767 | ai:claude
                            let stored_status_str = req.status.to_string();
                            let derived_annotation = status_display::derived_status_annotation(
                                &effective_status_str,
                                &stored_status_str,
                            );
                            // STORY-632: `--json` emits the spec as a machine object,
                            // including the centrality fields, then returns early.
                            // trace:STORY-632 | ai:claude
                            // The global format pin and the dedicated flag are two
                            // spellings of the same machine contract. Keeping this
                            // test at the renderer (rather than only in dispatch)
                            // also covers AIDA_OUTPUT_FORMAT=json.
                            // trace:BUG-1502 | ai:codex
                            if *json || output_format_is_json() {
                                // BUG-1558: the machine JSON projection used to omit
                                // relationships entirely — an agent reading `aida show
                                // --json` had no way to see the typed graph at all,
                                // which is a stronger form of the same collapse the
                                // human/TOON surfaces had (a missing field instead of
                                // a mislabeled one). Emit the same stored `rel_type`
                                // label the TOON table uses (`rel_type_label`) so all
                                // three surfaces agree on vocabulary.
                                // trace:BUG-1558 | ai:claude
                                #[derive(serde::Serialize)]
                                struct RelJson {
                                    rel_type: String,
                                    id: String,
                                    title: String,
                                }
                                #[derive(serde::Serialize)]
                                struct GitCommitJson {
                                    sha: String,
                                    short_sha: String,
                                    subject: String,
                                }
                                #[derive(serde::Serialize)]
                                struct GitFileJson {
                                    file: String,
                                    symbol: Option<String>,
                                }
                                #[derive(serde::Serialize)]
                                struct GitLinkageJson {
                                    commits: Vec<GitCommitJson>,
                                    files: Vec<GitFileJson>,
                                    // BUG-1993: `null` when a bounded scan timed
                                    // out before the shipped state was known — never
                                    // a false "not shipped". trace:BUG-1993 | ai:claude
                                    shipped: Option<bool>,
                                    branch: Option<String>,
                                    worktree: Option<String>,
                                    shipped_pr: Option<u64>,
                                    repo: Option<String>,
                                    // BUG-1594 (PRIN-5): `false` when a bounded scan
                                    // stopped early; `incomplete` names which part.
                                    // trace:BUG-1594 | ai:claude
                                    complete: bool,
                                    incomplete: Vec<String>,
                                    // TASK-1475: filing-drift hint (CR-8 acceptance 6
                                    // follow-up), omitted when there's nothing to
                                    // say. trace:TASK-1475 | ai:claude
                                    #[serde(skip_serializing_if = "Option::is_none")]
                                    drift_since_filing: Option<String>,
                                }
                                let relationships: Vec<RelJson> = req
                                    .relationships
                                    .iter()
                                    .map(|rel| {
                                        let (id, title) = match read_target(&rel.target_id) {
                                            Ok(Some(t)) => (t.display_id(), t.title.clone()),
                                            _ => ("(unknown)".to_string(), String::new()),
                                        };
                                        RelJson {
                                            rel_type: rel_type_label(&rel.rel_type),
                                            id,
                                            title,
                                        }
                                    })
                                    .collect();
                                let git_linkage = if *no_git {
                                    None
                                } else {
                                    let project_root = store_path
                                        .parent()
                                        .map(|p| p.to_path_buf())
                                        .unwrap_or_else(|| {
                                            std::env::current_dir().unwrap_or_default()
                                        });
                                    let mut ids = vec![req.display_id()];
                                    if let Some(agreed) = req.agreed_id.as_deref() {
                                        if !ids.iter().any(|id| id == agreed) {
                                            ids.push(agreed.to_string());
                                        }
                                    }
                                    if let Some(origin) = req.spec_id.as_deref() {
                                        if !ids.iter().any(|id| id == origin) {
                                            ids.push(origin.to_string());
                                        }
                                    }
                                    let (linkage, drift_since_filing) = {
                                        let _phase = crate::usage::phase("linkage");
                                        // BUG-1993: one deadline for collection
                                        // and the drift hint.
                                        let deadline = std::time::Instant::now()
                                            + crate::SHOW_LINKAGE_DEADLINE;
                                        let linkage = crate::collect_git_linkage_until(
                                            &project_root,
                                            &ids,
                                            deadline,
                                        );
                                        // TASK-1475: compute before `linkage.{files,commits}`
                                        // are moved into the JSON payload below.
                                        // trace:TASK-1475 trace:BUG-1993 | ai:claude
                                        let drift_since_filing = crate::filing_drift_hint_until(
                                            &project_root,
                                            req.filed_at.as_ref(),
                                            &linkage.files,
                                            &linkage.commits,
                                            deadline,
                                        );
                                        (linkage, drift_since_filing)
                                    };
                                    // trace:BUG-1594 trace:BUG-1993 | ai:claude
                                    let incomplete = linkage.incomplete_notes();
                                    Some(GitLinkageJson {
                                        complete: incomplete.is_empty(),
                                        incomplete,
                                        commits: linkage
                                            .commits
                                            .into_iter()
                                            .map(|(sha, short_sha, subject)| GitCommitJson {
                                                sha,
                                                short_sha,
                                                subject,
                                            })
                                            .collect(),
                                        files: linkage
                                            .files
                                            .into_iter()
                                            .map(|(file, symbol)| GitFileJson { file, symbol })
                                            .collect(),
                                        shipped: if linkage.shipped_unknown {
                                            None
                                        } else {
                                            Some(linkage.shipped)
                                        },
                                        branch: linkage.branch,
                                        worktree: linkage.worktree,
                                        shipped_pr: linkage.shipped_pr,
                                        repo: linkage.repo,
                                        drift_since_filing,
                                    })
                                };
                                // BUG-527: carry the human-visible queue membership
                                // into JSON as structured role/position pairs. The
                                // helper uses the same per-role rank as the human
                                // `Queued:` line, including the `general` label for
                                // unrouted entries. trace:BUG-1502 | ai:codex
                                let project_root = store_path
                                    .parent()
                                    .map(|p| p.to_path_buf())
                                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                                let queue_membership: Vec<serde_json::Value> =
                                queue_memberships_for(&project_root, &req.id)
                                    .into_iter()
                                    .map(|(role, position)| {
                                        serde_json::json!({
                                            "role": role.unwrap_or_else(|| "general".to_string()),
                                            "position": position,
                                        })
                                    })
                                    .collect();
                                // Start from the complete stored requirement rather
                                // than maintaining a lossy parallel projection. Then
                                // overlay every derived field the human view computes
                                // (effective lifecycle values, graph context, next
                                // actions, and git linkage). New stored human fields
                                // now arrive in JSON automatically instead of silently
                                // disappearing until another bug is filed.
                                // trace:BUG-1502 | ai:codex
                                let mut out = serde_json::to_value(&req)?;
                                let object = out.as_object_mut().ok_or_else(|| {
                                    anyhow::anyhow!("serialized requirement was not a JSON object")
                                })?;
                                object.insert(
                                    "display_id".to_string(),
                                    serde_json::Value::String(req.display_id()),
                                );
                                object.insert(
                                    "uuid".to_string(),
                                    serde_json::Value::String(req.id.to_string()),
                                );
                                object.insert(
                                    "opened".to_string(),
                                    serde_json::to_value(req.created_at)?,
                                );
                                object.insert(
                                    "modified".to_string(),
                                    serde_json::to_value(req.modified_at)?,
                                );
                                // BUG-1687: the machine projection reports the same
                                // DISPLAY answer the human and TOON surfaces do, so a
                                // deferred spec never reads as "act now" on one surface
                                // and not another (AC4). `stored_status` is why that is
                                // safe — it exists so a consumer never has to infer
                                // which of the two values is presentation-only — and the
                                // defer axis itself is reported as its own fields rather
                                // than left to be parsed out of a label.
                                // trace:BUG-1687 | ai:claude
                                let json_status = if status_display::is_deferred(&req) {
                                    status_display::DEFERRED_LABEL.to_string()
                                } else {
                                    display_status.clone()
                                };
                                object.insert(
                                    "status".to_string(),
                                    serde_json::Value::String(json_status),
                                );
                                object.insert(
                                    "stored_status".to_string(),
                                    serde_json::Value::String(req.status.to_string()),
                                );
                                object.insert(
                                    "deferred".to_string(),
                                    serde_json::Value::Bool(status_display::is_deferred(&req)),
                                );
                                object.insert(
                                    "deferred_until".to_string(),
                                    match status_display::deferred_revisit_trigger(&req) {
                                        Some(trigger) => serde_json::Value::String(trigger),
                                        None => serde_json::Value::Null,
                                    },
                                );
                                object.insert(
                                    "deferred_reason".to_string(),
                                    match req.deferred_reason.as_deref() {
                                        Some(reason) => {
                                            serde_json::Value::String(reason.to_string())
                                        }
                                        None => serde_json::Value::Null,
                                    },
                                );
                                object.insert(
                                    "priority".to_string(),
                                    serde_json::Value::String(format!(
                                        "{}",
                                        req.effective_priority()
                                    )),
                                );
                                object.insert(
                                    "relationships".to_string(),
                                    serde_json::to_value(&relationships)?,
                                );
                                object.insert(
                                    "in_degree".to_string(),
                                    serde_json::to_value(degrees.in_degree)?,
                                );
                                object.insert(
                                    "out_degree".to_string(),
                                    serde_json::to_value(degrees.out_degree)?,
                                );
                                object.insert(
                                    "heft".to_string(),
                                    serde_json::to_value(degrees.heft)?,
                                );

                                let blockers: Vec<serde_json::Value> = req
                                .relationships
                                .iter()
                                .filter(|rel| matches!(rel.rel_type, RelationshipType::BlockedBy))
                                .map(|rel| match read_target(&rel.target_id) {
                                    Ok(Some(blocker)) => serde_json::json!({
                                        "id": blocker.display_id(),
                                        "status": blocker.status.to_string(),
                                        "satisfied": matches!(blocker.status, RequirementStatus::Completed),
                                    }),
                                    _ => serde_json::json!({
                                        "id": rel.target_id.to_string(),
                                        "status": "missing",
                                        "satisfied": false,
                                    }),
                                })
                                .collect();
                                object.insert(
                                    "blocked".to_string(),
                                    serde_json::Value::Bool(blockers.iter().any(|v| {
                                        v.get("satisfied").and_then(|v| v.as_bool()) == Some(false)
                                    })),
                                );
                                object.insert(
                                    "blockers".to_string(),
                                    serde_json::Value::Array(blockers),
                                );

                                // trace:BUG-1767 | ai:claude — stored-aware, so a
                                // derived-complete epic is prescribed the close move,
                                // not `archive`.
                                let mut next = crate::help_next::show_spec_next(
                                    &effective_status_str,
                                    &stored_status_str,
                                    &req.display_id(),
                                );
                                crate::help_next::push_serialize_cluster(
                                    &mut next,
                                    serialize_cluster_command.clone(),
                                );
                                object.insert(
                                "next".to_string(),
                                serde_json::Value::Array(
                                    next.into_iter()
                                        .map(|step| serde_json::json!({"cmd": step.cmd, "to": step.to}))
                                        .collect(),
                                ),
                            );
                                object.insert(
                                    "git_linkage".to_string(),
                                    serde_json::to_value(git_linkage)?,
                                );
                                object.insert(
                                    "queue_membership".to_string(),
                                    serde_json::Value::Array(queue_membership),
                                );
                                println!("{}", crate::cache_output::json_pretty(&out)?);
                                return crate::command_registry::returned(Ok(()));
                            }
                            // TASK-964: AGENT-MODE token-efficient TOON render for the
                            // single-spec detail. A flat scalar head (the minimal field
                            // set an agent needs) plus a uniform relationships table.
                            // The human TTY detail view drops through unchanged. The
                            // description body is truncated unless `--full`/`--verbose`,
                            // mirroring `--card` density. trace:TASK-964
                            if agent_output_mode() && !*card {
                                let mut lines: Vec<String> = Vec::new();
                                lines.push(crate::toon::scalar("id", &req.display_id()));
                                if let Some(origin) = req.spec_id.as_deref() {
                                    if origin != req.display_id() {
                                        lines.push(crate::toon::scalar("origin_id", origin));
                                    }
                                }
                                lines.push(crate::toon::scalar("title", &req.title));
                                lines.push(crate::toon::scalar(
                                    "type",
                                    &format!("{:?}", req.req_type).to_ascii_lowercase(),
                                ));
                                // BUG-1687: this is the surface that misled two agent
                                // sessions — both read `status: needs-attention` off a
                                // spec they had just deferred. A deferred spec reports
                                // `deferred` here, keeps the stored value beside it as
                                // `stored_status` (so nothing is hidden, and AC3's
                                // round trip is readable), and emits the revisit trigger
                                // as its own scalar so the agent sees until WHEN without
                                // a second command. trace:BUG-1687 | ai:claude
                                // BUG-1767: on the non-deferred path (and inside
                                // `stored_status` for a deferred epic) a derived rollup
                                // status still carries its disclosure, so the agent can
                                // tell it from a stored one. trace:BUG-1767 | ai:claude
                                if status_display::is_deferred(&req) {
                                    lines.push(crate::toon::scalar(
                                        "status",
                                        status_display::DEFERRED_TOKEN,
                                    ));
                                    lines.push(crate::toon::scalar(
                                        "stored_status",
                                        &toon_show_status(
                                            &effective_status_str,
                                            &stored_status_str,
                                        ),
                                    ));
                                    if let Some(trigger) =
                                        status_display::deferred_revisit_trigger(&req)
                                    {
                                        lines.push(crate::toon::scalar("deferred_until", &trigger));
                                    }
                                    if let Some(reason) = req.deferred_reason.as_deref() {
                                        lines.push(crate::toon::scalar("deferred_reason", reason));
                                    }
                                } else {
                                    lines.push(crate::toon::scalar(
                                        "status",
                                        &toon_show_status(
                                            &effective_status_str,
                                            &stored_status_str,
                                        ),
                                    ));
                                }
                                lines.push(crate::toon::scalar(
                                    "priority",
                                    &format!("{}", req.effective_priority()).to_ascii_lowercase(),
                                ));
                                lines.push(crate::toon::scalar("owner", &req.owner));
                                if let Some(a) = req.assignee.as_deref() {
                                    lines.push(crate::toon::scalar("assignee", a));
                                }
                                if !req.feature.is_empty() {
                                    lines.push(crate::toon::scalar("feature", &req.feature));
                                }
                                lines.push(crate::toon::scalar("heft", &degrees.heft.to_string()));
                                // FR-283: the numeric weight/score, emitted only when
                                // set. trace:FR-283 | ai:claude
                                if let Some(w) = req.weight {
                                    lines.push(crate::toon::scalar(
                                        "weight",
                                        &crate::format_weight(w as f64),
                                    ));
                                }
                                if !req.tags.is_empty() {
                                    let mut tags: Vec<&str> =
                                        req.tags.iter().map(String::as_str).collect();
                                    tags.sort_unstable();
                                    lines.push(crate::toon::scalar("tags", &tags.join(" ")));
                                }
                                // Description body: truncated by char count unless the
                                // user opted into the full view. ASCII marker only.
                                let want_full = *full || *verbose;
                                let desc = if want_full {
                                    req.description.clone()
                                } else {
                                    const CAP: usize = 280;
                                    if req.description.chars().count() <= CAP {
                                        req.description.clone()
                                    } else {
                                        let head: String =
                                            req.description.chars().take(CAP).collect();
                                        format!("{head} ... (truncated; --full for the rest)")
                                    }
                                };
                                lines.push(crate::toon::scalar("description", &desc));
                                // TASK-1148: the three optional narrative fields, emitted
                                // as scalars only when set. trace:TASK-1148 | ai:claude
                                if let Some(v) = req.implementation_summary.as_deref() {
                                    if !v.trim().is_empty() {
                                        lines
                                            .push(crate::toon::scalar("implementation_summary", v));
                                    }
                                }
                                if let Some(v) = req.risk_notes.as_deref() {
                                    if !v.trim().is_empty() {
                                        lines.push(crate::toon::scalar("risk_notes", v));
                                    }
                                }
                                if let Some(v) = req.test_coverage_notes.as_deref() {
                                    if !v.trim().is_empty() {
                                        lines.push(crate::toon::scalar("test_coverage_notes", v));
                                    }
                                }
                                // STORY-776: the advisor's dispatch classification,
                                // emitted only when groomed. trace:STORY-776 | ai:claude
                                if let Some(m) = req.execution_mode {
                                    lines.push(crate::toon::scalar(
                                        "execution_mode",
                                        &m.to_string(),
                                    ));
                                }
                                // STORY-634: the multi-repo origin dimension, emitted
                                // only when set. trace:STORY-634 | ai:claude
                                if let Some(o) = &req.origin {
                                    lines.push(crate::toon::scalar("origin", &o.to_string()));
                                }
                                // trace:STORY-1673 | ai:claude
                                if let Some(v) = &req.verify {
                                    lines.push(crate::toon::scalar("verify", &v.to_string()));
                                }
                                // CR-8: filing provenance, one compact scalar, only
                                // when stamped. trace:CR-8 | ai:claude
                                if let Some(p) = req.filed_at.as_ref().filter(|p| !p.is_empty()) {
                                    lines.push(crate::toon::scalar("filed_at", &p.summary_line()));
                                }
                                lines.extend(toon_show_comment_lines(&req.comments, *comments));
                                // STORY-1649: one cheap default-branch linkage scalar;
                                // `--no-git` turns it off. trace:STORY-1649 | ai:claude
                                if !*no_git {
                                    if let Some(root) = store_path.parent() {
                                        let ids = crate::shipped_linkage::spec_lookup_ids(&req);
                                        let _phase = crate::usage::phase("linkage");
                                        lines.push(crate::shipped_linkage::toon_show_linkage_line(
                                            root, &ids,
                                        ));
                                    }
                                }
                                println!("{}", lines.join("\n"));

                                // Relationships as a uniform TOON table (rel,id,title).
                                // Unlike the human card's presentation buckets, the
                                // agent surface must preserve the stored edge type.
                                // trace:BUG-1442 | ai:codex
                                if !req.relationships.is_empty() {
                                    let mut rows: Vec<Vec<String>> = Vec::new();
                                    for rel in &req.relationships {
                                        let label = rel_type_label(&rel.rel_type);
                                        let (tid, ttitle) = match read_target(&rel.target_id) {
                                            Ok(Some(t)) => (t.display_id(), t.title.clone()),
                                            _ => ("(unknown)".to_string(), String::new()),
                                        };
                                        rows.push(vec![label, tid, ttitle]);
                                    }
                                    println!(
                                        "{}",
                                        crate::toon::table_raw(
                                            "relationships",
                                            &["rel", "id", "title"],
                                            &rows
                                        )
                                    );
                                }
                                let blocker_targets: Vec<uuid::Uuid> = req
                                    .relationships
                                    .iter()
                                    .filter(|rel| {
                                        matches!(rel.rel_type, RelationshipType::BlockedBy)
                                    })
                                    .map(|rel| rel.target_id)
                                    .collect();
                                if !blocker_targets.is_empty() {
                                    let mut rows = Vec::new();
                                    let mut unsatisfied = 0usize;
                                    for target in blocker_targets {
                                        let (id, status, satisfied) = match read_target(&target)? {
                                            Some(blocker) => {
                                                let satisfied = matches!(
                                                    blocker.status,
                                                    RequirementStatus::Completed
                                                );
                                                (
                                                    blocker.display_id(),
                                                    blocker.status.to_string(),
                                                    satisfied,
                                                )
                                            }
                                            None => {
                                                (target.to_string(), "missing".to_string(), false)
                                            }
                                        };
                                        if !satisfied {
                                            unsatisfied += 1;
                                        }
                                        rows.push(vec![id, status, satisfied.to_string()]);
                                    }
                                    println!(
                                        "{}",
                                        crate::toon::table_raw(
                                            "blockers",
                                            &["id", "status", "satisfied"],
                                            &rows
                                        )
                                    );
                                    println!(
                                        "{}",
                                        crate::toon::scalar(
                                            "blocked",
                                            &(unsatisfied > 0).to_string()
                                        )
                                    );
                                    if unsatisfied > 0 {
                                        println!(
                                            "{}",
                                            crate::toon::scalar(
                                                "pickup",
                                                "refused until all blockers are Completed"
                                            )
                                        );
                                    }
                                }
                                // TASK-974 (AXI #9): lifecycle-aware next-step block —
                                // the valid next transition(s) for THIS spec's current
                                // state, templated with its id. trace:TASK-974
                                // trace:BUG-1767 | ai:claude — stored-aware.
                                let mut next = crate::help_next::show_spec_next(
                                    &effective_status_str,
                                    &stored_status_str,
                                    &req.display_id(),
                                );
                                crate::help_next::push_serialize_cluster(
                                    &mut next,
                                    serialize_cluster_command.clone(),
                                );
                                if let Some(block) = crate::help_next::render(&next) {
                                    println!("{block}");
                                }
                                return crate::command_registry::returned(Ok(()));
                            }
                            // TASK-265: --card renders a compact boxed spec card
                            // instead of the linear detail view, so /aida-pickup can
                            // drop the spec's contract into terminal scrollback at
                            // session start. The plain `aida show` stays the
                            // canonical detail surface. trace:TASK-265 | ai:claude
                            if *card {
                                let density = if *brief {
                                    CardDensity::Brief
                                } else if *full {
                                    CardDensity::Full
                                } else {
                                    CardDensity::Balanced
                                };
                                // Resolve each relationship target into a `CardRel`
                                // row so the card can name parents, related specs AND
                                // custom edges, not just count them.
                                // trace:BUG-1471 | ai:claude
                                let mut rels: Vec<CardRel> = Vec::new();
                                for rel in &req.relationships {
                                    let (rid, rtitle) = match read_target(&rel.target_id) {
                                        Ok(Some(t)) => (t.display_id(), t.title.clone()),
                                        _ => ("(unknown)".to_string(), String::new()),
                                    };
                                    rels.push(CardRel::new(&rel.rel_type, rid, rtitle));
                                }
                                render_spec_card(
                                    &req, &rels, store_path, density, *no_git, *verbose,
                                );
                                let mut next = Vec::new();
                                crate::help_next::push_serialize_cluster(
                                    &mut next,
                                    serialize_cluster_command.clone(),
                                );
                                if let Some(block) = crate::help_next::render_human(&next) {
                                    println!("{block}");
                                }
                                return crate::command_registry::returned(Ok(()));
                            }
                            println!("{}: {}", "ID".bold(), req.display_id());
                            // Only show Agreed ID / Origin ID when they actually
                            // differ from the canonical display id — otherwise it's
                            // three lines of the same string. trace:BUG-29
                            let canonical = req.display_id();
                            if let Some(ref agreed) = req.agreed_id {
                                if agreed.as_str() != canonical {
                                    println!("{}: {}", "Agreed ID".bold(), agreed);
                                }
                            }
                            if let Some(ref origin) = req.spec_id {
                                if origin.as_str() != canonical {
                                    // trace:FR-1-070 | ai:claude
                                    println!("{}: {}", "Origin ID".bold(), origin);
                                }
                            }
                            println!("{}: {}", "UUID".bold(), req.id);
                            println!("{}: {}", "Title".bold(), req.title);
                            // TASK-91: surface PR-N for auto-queued review stories
                            // so `aida show STORY-108` reads "About: PR-15" right
                            // next to the title, mirroring what `aida queue list`
                            // promotes to the prominent id. Dual-id discoverable
                            // from a single command. trace:TASK-91 | ai:claude
                            if let Some((primary, _)) = format_review_story_display(
                                req.display_id().as_str(),
                                req.title.as_str(),
                            ) {
                                // primary is "PR-N (STORY-NNN)"; strip the
                                // parenthetical for a cleaner About: line.
                                let about = primary
                                    .split_once(' ')
                                    .map(|(pr, _)| pr.to_string())
                                    .unwrap_or(primary);
                                println!("{}: {}", "About".bold(), about.cyan());
                            }
                            println!("{}: {:?}", "Type".bold(), req.req_type);
                            // TASK-269: colour-code Status inline so a quick scan
                            // catches it; the same value is reprinted at the foot
                            // of the output below. trace:TASK-269 | ai:claude
                            // BUG-626: derived rollup status for epics. trace:BUG-626
                            // BUG-781: for a decision spec, the stored `Approved` IS
                            // ACCEPTED — the terminal state — so display it that way
                            // here and in the reprint at the foot. trace:BUG-781
                            let status = display_status.clone();
                            // BUG-1767: a derived rollup status carries its disclosure
                            // on BOTH prints (here and the foot reprint), so neither
                            // reads as a stored value. trace:BUG-1767 | ai:claude
                            let status_note = derived_annotation
                                .as_deref()
                                .map(|a| format!(" {}", format!("({a})").dimmed()))
                                .unwrap_or_default();
                            // BUG-1687: a deferred spec must not advertise the status it
                            // was deferred OUT of — `spec_status_badge` applies the
                            // display override and falls through to the stored badge for
                            // everything else. trace:BUG-1687 | ai:claude
                            println!(
                                "{}: {}{}",
                                "Status".bold(),
                                status_display::spec_status_badge(&req, &status),
                                status_note
                            );
                            println!("{}: {}", "Priority".bold(), req.effective_priority());
                            // FR-283: the numeric weight/score, shown only when set.
                            // trace:FR-283 | ai:claude
                            if let Some(w) = req.weight {
                                println!("{}: {}", "Weight".bold(), crate::format_weight(w as f64));
                            }
                            // STORY-634: where this spec's code lives in a multi-repo
                            // shared-store workspace ("repo" or "repo/component");
                            // absent = single-repo, prints nothing.
                            if let Some(o) = &req.origin {
                                println!("{}: {}", "Origin".bold(), o.to_string().cyan());
                            }
                            // The data-only constraint check, verbatim; probes run
                            // only under `aida verify constraints`.
                            // trace:STORY-1673 | ai:claude
                            if let Some(v) = &req.verify {
                                println!("{}: {}", "Verify".bold(), v.to_string().dimmed());
                            }
                            // CR-8: where (code state) and with what (tooling) the
                            // spec was filed — one compact line, only when stamped.
                            // trace:CR-8 | ai:claude
                            if let Some(p) = req.filed_at.as_ref().filter(|p| !p.is_empty()) {
                                println!("{}: {}", "Filed at".bold(), p.summary_line().dimmed());
                                if p.build_diverges_from_code() && crate::cwd_is_aida_source_repo()
                                {
                                    println!(
                                    "  {} filed by a binary built from a different commit than \
                                     the checked-out code",
                                    "note:".yellow()
                                );
                                }
                            }
                            // BUG-524: surface when the spec was opened / last touched so
                            // `aida show` reveals its age. Stored UTC, rendered in local
                            // time (feedback_local_time) reusing history.rs's format.
                            // trace:BUG-524 | ai:claude
                            println!(
                                "{}: {}",
                                "Opened".bold(),
                                req.created_at
                                    .with_timezone(&chrono::Local)
                                    .format("%Y-%m-%d %H:%M")
                            );
                            println!(
                                "{}: {}",
                                "Modified".bold(),
                                req.modified_at
                                    .with_timezone(&chrono::Local)
                                    .format("%Y-%m-%d %H:%M")
                            );
                            if !req.owner.is_empty() {
                                println!("{}: {}", "Owner".bold(), req.owner);
                            }
                            // STORY-639: surface the assignee only when set, so an
                            // unassigned spec renders exactly as before. trace:STORY-639
                            if let Some(assignee) = req.assignee.as_deref() {
                                println!("{}: {}", "Assignee".bold(), assignee);
                            }
                            if !req.tags.is_empty() {
                                println!(
                                    "{}: {}",
                                    "Tags".bold(),
                                    req.tags.iter().cloned().collect::<Vec<_>>().join(", ")
                                );
                            }
                            // STORY-476: one-way external issue refs rendered as links
                            // via the `[external_refs]` base URLs in config. A ref with
                            // no resolvable base URL prints its bare `provider:id`.
                            // trace:STORY-476 | ai:claude
                            if !req.external_refs.is_empty() {
                                let base_urls = read_external_ref_base_urls(store_path);
                                println!("{}:", "External refs".bold());
                                for raw in &req.external_refs {
                                    match aida_core::external_refs::parse_ref(raw) {
                                        Ok(parsed) => {
                                            match aida_core::external_refs::render_ref_url(
                                                &parsed, &base_urls,
                                            ) {
                                                Some(url) => println!(
                                                    "  ↗ {} → {}",
                                                    parsed.canonical().yellow(),
                                                    url.cyan()
                                                ),
                                                None => {
                                                    println!("  ↗ {}", parsed.canonical().yellow())
                                                }
                                            }
                                        }
                                        // Stored ref that no longer parses (e.g. a
                                        // provider removed from the known set) — show
                                        // it verbatim rather than dropping it.
                                        Err(_) => println!("  ↗ {}", raw.yellow()),
                                    }
                                }
                            }
                            // STORY-542: user-facing interface changes captured at
                            // close — the deterministic source the operator digest
                            // reads. trace:STORY-542 | ai:claude
                            if let Some(ic) = req.interface_changes.as_ref() {
                                if !ic.is_empty() {
                                    println!("{}:", "Interface changes".bold());
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
                            // TASK-102: enumerate relationships inline when there are
                            // few (≤5) or `--rels` is passed; otherwise print the count
                            // + a pointer to `aida rel list`. Truncate past 10 with a
                            // "…N more" line so highly-connected specs don't flood the
                            // view. Each edge shows direction-phrase + target + title.
                            // trace:TASK-102 | ai:claude
                            if !req.relationships.is_empty() {
                                let total = req.relationships.len();
                                const INLINE_THRESHOLD: usize = 5;
                                const TRUNCATE_AT: usize = 10;
                                let spec_label = req.spec_id.as_deref().unwrap_or(id);
                                if *rels || total <= INLINE_THRESHOLD {
                                    println!("{}:", "Relations".bold());
                                    for rel in req.relationships.iter().take(TRUNCATE_AT) {
                                        let phrase = relationship_phrase(&rel.rel_type);
                                        match read_target(&rel.target_id)? {
                                            Some(t) => println!(
                                                "  {} {} {} ({})",
                                                crate::glyph(crate::glyphs::Glyph::SubArrow),
                                                phrase.cyan(),
                                                t.spec_id.as_deref().unwrap_or("?").yellow(),
                                                t.title
                                            ),
                                            None => println!(
                                                "  {} {} {} (missing)",
                                                crate::glyph(crate::glyphs::Glyph::SubArrow),
                                                phrase.cyan(),
                                                rel.target_id.to_string().dimmed()
                                            ),
                                        }
                                    }
                                    if total > TRUNCATE_AT {
                                        println!(
                                            "  … {} more — see `aida rel list {}`",
                                            total - TRUNCATE_AT,
                                            spec_label
                                        );
                                    }
                                } else {
                                    // TASK-1417: the count alone doesn't tell a reader
                                    // whether rows exist and were withheld by policy,
                                    // or the spec genuinely has nothing more — say so
                                    // explicitly so the two cases can't be confused.
                                    // TOON is unchanged by this (it always enumerates,
                                    // above); this line is the human-only view limit.
                                    // trace:TASK-1417 | ai:claude
                                    println!(
                                    "{}: {} relationship(s) — withheld by the view limit, not enumerated below (use {} to show every edge, or `aida rel list {}`)",
                                    "Relations".bold(),
                                    total,
                                    "--rels".cyan(),
                                    spec_label
                                );
                                }
                            }
                            // STORY-632: deterministic centrality readout — inbound +
                            // outbound degree (tracked separately: high inbound =
                            // load-bearing/foundational, high outbound = coupling) plus
                            // the type-weighted heft score. Shown only when the spec is
                            // connected. trace:STORY-632 | ai:claude
                            if degrees.in_degree > 0 || degrees.out_degree > 0 {
                                println!(
                                    "{}: {} in / {} out  (heft {})",
                                    "Centrality".bold(),
                                    degrees.in_degree,
                                    degrees.out_degree,
                                    degrees.heft
                                );
                            }
                            // STORY-446: Blockers section — one line per BlockedBy edge
                            // with the blocker's status + a pickability glyph (check when
                            // Completed/satisfied, in-flight when it still blocks pickup), so the
                            // pickability gate's verdict is visible at a glance.
                            // trace:STORY-446 | ai:claude
                            let blocker_targets: Vec<uuid::Uuid> = req
                                .relationships
                                .iter()
                                .filter(|r| matches!(r.rel_type, RelationshipType::BlockedBy))
                                .map(|r| r.target_id)
                                .collect();
                            if !blocker_targets.is_empty() {
                                println!("\n{}:", "Blockers".bold());
                                let mut unsatisfied = 0;
                                for target in &blocker_targets {
                                    let (sid, status, satisfied) = match read_target(target)? {
                                        Some(b) => {
                                            let satisfied =
                                                matches!(b.status, RequirementStatus::Completed);
                                            (
                                                b.spec_id
                                                    .clone()
                                                    .unwrap_or_else(|| "?".to_string()),
                                                format!("{}", b.status),
                                                satisfied,
                                            )
                                        }
                                        None => (target.to_string(), "missing".to_string(), false),
                                    };
                                    if !satisfied {
                                        unsatisfied += 1;
                                    }
                                    let glyph = if satisfied {
                                        crate::glyph(crate::glyphs::Glyph::Check)
                                            .green()
                                            .to_string()
                                    } else {
                                        crate::glyph(crate::glyphs::Glyph::InFlight)
                                            .yellow()
                                            .to_string()
                                    };
                                    println!("  {} {} ({})", glyph, sid, status);
                                }
                                if unsatisfied > 0 {
                                    println!(
                                    "  {} blocked — {} blocker(s) not yet Completed; `aida queue work` will refuse pickup",
                                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                    unsatisfied
                                );
                                }
                            }
                            if !req.comments.is_empty() && !crate::agent_output_mode() {
                                println!(
                                    "{}: {} comment(s)",
                                    "Comments".bold(),
                                    req.comments.len()
                                );
                            }
                            {
                                // trace:STORY-1173 | ai:codex
                                let mut approval_entries = Vec::new();
                                collect_proxy_approval_entries(
                                    &mut approval_entries,
                                    &req.display_id(),
                                    &req.title,
                                    &req.comments,
                                    None,
                                    None,
                                );
                                if !approval_entries.is_empty() {
                                    println!();
                                    print_proxy_approval_entries(&approval_entries, false);
                                }
                            }
                            // STORY-81: surface the auto-stamped completion
                            // context (who/when/source-tool/optional summary)
                            // when `aida queue done` populated it. Skips when
                            // `implementation_info` is None or `implemented`
                            // is false — keeps the section out of `aida show`
                            // for reqs that were closed via `aida edit
                            // --status completed` directly (no queue done flow).
                            // trace:STORY-81 | ai:claude
                            if let Some(info) =
                                req.implementation_info.as_ref().filter(|i| i.implemented)
                            {
                                println!("\n{}:", "Implementation".green().bold());
                                if let Some(ts) = info.implemented_at {
                                    println!(
                                        "  Completed: {}",
                                        ts.with_timezone(&chrono::Local)
                                            .format("%Y-%m-%d %H:%M %Z")
                                    );
                                }
                                if let Some(ref who) = info.implemented_by {
                                    println!("  By:        {}", who);
                                }
                                if let Some(ref tool) = info.source_tool {
                                    println!("  Source:    {}", tool);
                                }
                                if let Some(ref summary) = info.summary {
                                    println!("  Summary:");
                                    for line in summary.lines() {
                                        println!("    {}", line);
                                    }
                                }
                                // STORY-698: the verification steps the builder ran,
                                // captured at `aida queue done` — the audit trail the
                                // PR body surfaces. trace:STORY-698 | ai:claude
                                if let Some(steps) = info
                                    .test_coverage_notes
                                    .as_ref()
                                    .filter(|s| !s.trim().is_empty())
                                {
                                    println!("  Verification:");
                                    for line in steps.lines().filter(|l| !l.trim().is_empty()) {
                                        println!("    - {}", line);
                                    }
                                }
                            }
                            // STORY-332: surface the punt reason when the spec is
                            // paused in NeedsAttention, so triage sees the fork
                            // without grepping the ledger. `attention_reason` is
                            // present only while paused (cleared on triage-out).
                            if let Some(reason) = req.attention_reason.as_ref() {
                                println!("\n{}:", "Needs Attention — punted".magenta().bold());
                                println!("  Category:  {}", reason.category);
                                println!("  Reason:    {}", reason.detail);
                                if let Some(lean) = &reason.lean {
                                    println!("  Lean:      {}", lean);
                                }
                                if let Some(who) = &reason.raised_by {
                                    println!("  Raised by: {}", who);
                                }
                                println!(
                                    "  Raised at: {}",
                                    reason
                                        .raised_at
                                        .with_timezone(&chrono::Local)
                                        .format("%Y-%m-%d %H:%M %Z")
                                );
                            }
                            // STORY-522: surface a PENDING DecisionRequest so the
                            // human sees the question + choices in `aida show`,
                            // self-contained, and can answer it with
                            // `aida questions answer`. trace:STORY-522 | ai:claude
                            if let Some(dr) =
                                req.decision_request.as_ref().filter(|d| d.is_pending())
                            {
                                println!("\n{}:", "Decision needed".yellow().bold());
                                println!("  {}", dr.question);
                                for (i, choice) in dr.choices.iter().enumerate() {
                                    let marker = if dr.recommended == Some(i) {
                                        " (recommended)".green().to_string()
                                    } else {
                                        String::new()
                                    };
                                    println!(
                                        "    {}{} {} — {}",
                                        format!("{}.", i + 1).bold(),
                                        marker,
                                        choice.label.bold(),
                                        choice.consequence.dimmed()
                                    );
                                }
                                if let Some(rationale) = &dr.rationale {
                                    println!("  {} {}", "Why:".dimmed(), rationale);
                                }
                                println!("  {}", "Answer with `aida questions answer`.".dimmed());
                            }
                            // STORY-776: the advisor's dispatch classification — how
                            // `aida do` will route this spec. Rendered only when
                            // groomed, with the human contract so the routing is
                            // explicit at a glance. trace:STORY-776 | ai:claude
                            if let Some(m) = req.execution_mode {
                                println!(
                                    "\n{} {} — {}",
                                    "Execution mode:".bold(),
                                    m.to_string().cyan(),
                                    crate::do_dispatch::human_contract(m).dimmed()
                                );
                            }
                            // Human output at an interactive terminal renders the
                            // markdown in the description (the knowledge body for
                            // FAQs and other knowledge types) and in comment bodies.
                            // `--raw` / AIDA_MARKDOWN=raw and piped output print the
                            // source exactly as before. trace:STORY-1652 | ai:claude
                            let markdown_width =
                                crate::markdown_render::show_renders_markdown(*raw)
                                    .then(crate::markdown_render::terminal_width);
                            if !req.description.is_empty() {
                                match markdown_width {
                                    Some(width) => println!(
                                        "\n{}",
                                        crate::markdown_render::render_markdown_for_terminal(
                                            &req.description,
                                            width
                                        )
                                    ),
                                    None => println!("\n{}", req.description),
                                }
                            }
                            // STORY-1434: corrections and carve-outs recorded as
                            // comments must be visible in the DEFAULT view (no
                            // `-c`/`--comments`), right after the description — the
                            // bug this closes is exactly a correction that only
                            // `-c`, or a scroll hundreds of lines into the YAML,
                            // would have shown. Runs unconditionally (not gated on
                            // `*comments`) and duplicates into the full `-c` comment
                            // list below without harm. trace:STORY-1434 | ai:claude
                            if !*no_comments && (!crate::agent_output_mode() || *comments) {
                                let mut visible_comments: Vec<&aida_core::Comment> = Vec::new();
                                crate::collect_default_visible_comments(
                                    &req.comments,
                                    &mut visible_comments,
                                );
                                if !visible_comments.is_empty() {
                                    println!("\n{}:", "Corrections / carve-outs".yellow().bold());
                                    for c in &visible_comments {
                                        crate::print_comment_with(c, 0, markdown_width);
                                    }
                                }
                            }
                            // TASK-1148: the three optional narrative fields — the
                            // genuinely-new metadata not derivable from git/status/trace.
                            // Each renders its own labelled block only when set; absent
                            // fields print nothing. trace:TASK-1148 | ai:claude
                            {
                                let narrative_blocks: [(&str, &Option<String>); 3] = [
                                    ("Implementation summary", &req.implementation_summary),
                                    ("Risk notes", &req.risk_notes),
                                    ("Test coverage notes", &req.test_coverage_notes),
                                ];
                                for (label, value) in narrative_blocks {
                                    if let Some(text) = value {
                                        let text = text.trim();
                                        if !text.is_empty() {
                                            println!("\n{}:", label.bold());
                                            for line in text.lines() {
                                                println!("  {line}");
                                            }
                                        }
                                    }
                                }
                            }
                            if !*no_comments
                                && (*comments || !crate::agent_output_mode())
                                && !req.comments.is_empty()
                            {
                                println!("\n{}:", "Comments".green().bold());
                                for c in &req.comments {
                                    crate::print_comment_with(c, 0, markdown_width);
                                }
                            }
                            // STORY-582: surface the durable processing record — the
                            // committed audit of what was done + why at completion.
                            // Always shown (not gated on --verbose) so the audit trail
                            // is visible on a plain `aida show`. trace:STORY-582
                            if !req.processing_record.is_empty() {
                                print_processing_records(&req.processing_record);
                            }
                            // BUG-527: surface queue membership — for-role + the
                            // 1-based position the operator sees on `aida queue
                            // list`. Mirrors how git-linkage is surfaced below; the
                            // line is OMITTED when the spec sits in no queue so a
                            // not-queued spec's card stays clean. Mirrored onto the
                            // MCP `show_requirement` tool for parity (STORY-82).
                            // trace:BUG-527
                            {
                                let project_root = store_path
                                    .parent()
                                    .map(|p| p.to_path_buf())
                                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                                let memberships = queue_memberships_for(&project_root, &req.id);
                                if let Some(value) = format_queue_membership(&memberships) {
                                    println!("{}: {}", "Queued".green().bold(), value);
                                }
                            }
                            // TASK-241: append the git-linkage section unless
                            // --no-git. Grep against every id form the spec has
                            // worn (canonical / agreed / origin) so commits and
                            // trace comments survive id migration.
                            if !*no_git {
                                let mut ids: Vec<String> = vec![req.display_id()];
                                if let Some(ref a) = req.agreed_id {
                                    if !ids.contains(&a.to_string()) {
                                        ids.push(a.to_string());
                                    }
                                }
                                if let Some(ref o) = req.spec_id {
                                    if !ids.contains(o) {
                                        ids.push(o.clone());
                                    }
                                }
                                let project_root = store_path
                                    .parent()
                                    .map(|p| p.to_path_buf())
                                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                                let _phase = crate::usage::phase("linkage");
                                print_git_linkage(
                                    &project_root,
                                    &ids,
                                    *verbose,
                                    req.filed_at.as_ref(),
                                );
                            }
                            // TASK-269: `aida show` output runs 50-150 lines, so the
                            // top Status field scrolls off-screen. Reprint it after a
                            // rule — "what state is this in?" then sits at the cursor
                            // when the command returns. trace:TASK-269 | ai:claude
                            println!("\n{}", "─".repeat(40).dimmed());
                            // trace:BUG-1767 | ai:claude — reprint keeps the disclosure.
                            // trace:BUG-1687 | ai:claude — the reprint carries the same
                            // deferral override as the Status line at the top.
                            println!(
                                "{}: {}{}",
                                "Status".bold(),
                                status_display::spec_status_badge(&req, &status),
                                status_note
                            );
                            // STORY-727: the per-spec next-command block for HUMANS. The
                            // agent-mode TOON `next` block fires on its own branch above
                            // (and returns); STORY-723 added next-steps to the front door
                            // but not to per-spec inspection, so the human `show` never
                            // got a next command. Render it now, leading with `aida zen
                            // <id>` for an Approved/Planned spec. trace:STORY-727
                            // trace:BUG-1767 | ai:claude — stored-aware, matching the
                            // agent surfaces above.
                            let mut next = crate::help_next::show_spec_next(
                                &effective_status_str,
                                &stored_status_str,
                                &req.display_id(),
                            );
                            crate::help_next::push_serialize_cluster(
                                &mut next,
                                serialize_cluster_command,
                            );
                            if let Some(block) = crate::help_next::render_human(&next) {
                                println!("{block}");
                            }
                        }
                        None => {
                            // BUG-600: not-found is a failure — return the error so the
                            // process exits non-zero, matching edit/defer/archive and
                            // keeping script gating + usage telemetry honest (was a
                            // print-and-exit-0). trace:BUG-600 | ai:claude
                            return crate::command_registry::returned(Err(
                                not_found::requirement_not_found(id, Some(store_path)),
                            ));
                        }
                    }
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                ShowArgs { id, .. } => {
                    // Legacy SQLite show_requirement always prints comments inline,
                    // so the --comments flag is a no-op here. Git backend honors it.
                    show_requirement(&storage, id)?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static SHOW: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "show",
    help_after: "list",
    command: <ShowArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Read,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Store(run_show),
};

/// Edit an existing requirement
#[derive(clap::Parser, Debug)]
#[clap(name = "edit")]
pub(crate) struct EditArgs {
    /// The ID of the requirement to edit
    pub id: String,

    /// New title for the requirement
    // trace:BUG-1294 | ai:claude
    #[clap(long, allow_hyphen_values = true)]
    pub title: Option<String>,

    /// New description for the requirement
    // trace:BUG-1294 | ai:claude
    #[clap(long, conflicts_with_all = ["description_from_file", "description_stdin"], allow_hyphen_values = true)]
    pub description: Option<String>,

    /// Read the new description body from a file. Empty files are refused.
    // trace:BUG-1234 | ai:codex
    #[clap(long, conflicts_with_all = ["description", "description_stdin"])]
    pub description_from_file: Option<std::path::PathBuf>,

    /// Read the new description body from stdin. Empty input is refused.
    // trace:BUG-1234 | ai:codex
    #[clap(long, conflicts_with_all = ["description", "description_from_file"])]
    pub description_stdin: bool,

    /// Replace the data-only constraint verification block with JSON.
    /// Probes run only with `aida verify constraints`.
    // trace:STORY-1673 | ai:codex
    #[clap(long, value_name = "JSON", conflicts_with = "clear_verify")]
    pub verify: Option<String>,

    /// Remove a constraint's verification block.
    #[clap(long)]
    pub clear_verify: bool,

    /// New status (draft, approved, planned, in-progress, done,
    /// completed, rejected, superseded). Needs Attention is set via
    /// `aida punt`. `superseded` = adopted, then replaced by a later
    /// spec — pair it with --superseded-by.
    #[clap(long)]
    pub status: Option<String>,

    // trace:TASK-1176 | ai:claude — plain `//` keeps the id out of --help.
    /// Record the spec that REPLACES this one, as a typed superseded-by
    /// edge (plus the inverse supersedes edge on the successor). Implies
    /// `--status superseded` unless --status says otherwise, so
    /// `aida edit <old-spec> --superseded-by <new-spec>` is the whole
    /// move.
    #[clap(long = "superseded-by", value_name = "SPEC-ID")]
    pub superseded_by: Option<String>,

    // trace:STORY-1434 | ai:claude — plain `//` keeps the id out of --help.
    /// Carve a named criterion OUT of this spec's description in one
    /// step: the exact text is struck from the description (replaced
    /// with a pointer to the spec that now carries it), a typed
    /// carved-out-to edge is recorded (plus the inverse carved-from edge
    /// on the target — walkable by `aida graph`/`query_graph`), and the
    /// reason is logged as a comment that `aida show`'s DEFAULT view
    /// surfaces inline. Fixes the trap where a carve-out's correction
    /// lived only in a comment nobody reads and the description kept
    /// gating on text that had already moved. Pass the criterion text
    /// exactly as it appears in the description. Pair with
    /// --carve-into.
    #[clap(
        long = "carve-out",
        value_name = "TEXT",
        requires = "carve_into",
        allow_hyphen_values = true
    )]
    pub carve_out: Option<String>,

    /// The spec that now carries the criterion named by --carve-out.
    #[clap(long = "carve-into", value_name = "SPEC-ID", requires = "carve_out")]
    pub carve_into: Option<String>,

    /// Optional reason recorded in the carve-out comment (why the
    /// criterion moved). Defaults to a generic note when omitted.
    #[clap(
        long = "carve-reason",
        value_name = "TEXT",
        requires = "carve_out",
        allow_hyphen_values = true
    )]
    pub carve_reason: Option<String>,

    /// New priority (high, medium, low)
    #[clap(long)]
    pub priority: Option<String>,

    /// New type: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc, faq
    // trace:BUG-1920 | ai:claude
    #[clap(long)]
    pub r#type: Option<String>,

    /// New owner
    // trace:BUG-1294 | ai:claude
    #[clap(long, allow_hyphen_values = true)]
    pub owner: Option<String>,

    /// New feature
    // trace:BUG-1294 | ai:claude
    #[clap(long, allow_hyphen_values = true)]
    pub feature: Option<String>,

    /// New tags (comma-separated, replaces existing). A tag may not
    /// contain whitespace: `--tags "a b"` is refused, `--tags a,b` is two.
    /// Use --add-tag/--remove-tag for partial edits that don't clobber.
    #[clap(long)]
    pub tags: Option<String>,

    /// Add a tag without touching existing tags. Repeatable
    /// (--add-tag x --add-tag y). Adding a present tag is a no-op.
    /// Conflicts with --tags.
    // trace:TASK-351 | ai:claude
    #[clap(long = "add-tag", value_name = "TAG", conflicts_with = "tags")]
    pub add_tag: Vec<String>,

    /// Remove a tag without touching the rest. Repeatable
    /// (--remove-tag x --remove-tag y). Removing an absent tag is a no-op.
    /// Conflicts with --tags.
    // trace:TASK-351 | ai:claude
    #[clap(long = "remove-tag", value_name = "TAG", conflicts_with = "tags")]
    pub remove_tag: Vec<String>,

    // trace:STORY-446 | ai:claude
    /// Add a BlockedBy edge to this spec (repeatable; alias
    /// `--depends-on`). Creates the BlockedBy edge + inverse Blocks edge
    /// atomically; idempotent on a re-add.
    #[clap(
        long = "blocked-by",
        visible_alias = "depends-on",
        value_name = "SPEC-ID"
    )]
    pub blocked_by: Vec<String>,

    /// Remove a BlockedBy edge from this spec (repeatable). Removes both
    /// the BlockedBy edge and the inverse Blocks edge; no-op if absent.
    #[clap(long = "remove-blocked-by", value_name = "SPEC-ID")]
    pub remove_blocked_by: Vec<String>,

    // trace:STORY-476 | ai:claude
    /// Attach a one-way external issue reference (repeatable). Format
    /// `provider:id` where provider is linear, jira, or github — e.g.
    /// `--add-ref linear:LIN-123 --add-ref github:owner/repo#123`.
    /// Rendered as a link in `aida show` and searchable. AIDA stores the
    /// pointer; it does NOT sync state back to the external system.
    #[clap(long = "add-ref", value_name = "PROVIDER:ID")]
    pub add_ref: Vec<String>,

    /// Remove a previously-attached external issue reference (repeatable).
    /// Matches the stored `provider:id` form; removing an absent ref is a
    /// no-op.
    #[clap(long = "remove-ref", value_name = "PROVIDER:ID")]
    pub remove_ref: Vec<String>,

    // trace:TASK-1148 | ai:claude
    /// Set the narrative implementation summary — the "what shipped and why
    /// it was done this way" that a commit prefix / diff does not capture.
    /// Pass an empty string to clear.
    // trace:BUG-1294 | ai:claude
    #[clap(
        long = "implementation-summary",
        value_name = "TEXT",
        allow_hyphen_values = true
    )]
    pub implementation_summary: Option<String>,

    // trace:TASK-1148 | ai:claude
    /// Set the narrative risk notes — residual risk / blast-radius call not
    /// derivable from git, status, or trace. Pass an empty string to clear.
    // trace:BUG-1294 | ai:claude
    #[clap(long = "risk-notes", value_name = "TEXT", allow_hyphen_values = true)]
    pub risk_notes: Option<String>,

    // trace:TASK-1148 | ai:claude
    /// Set the narrative test-coverage notes — what was (and was not)
    /// covered, and why. Pass an empty string to clear.
    // trace:BUG-1294 | ai:claude
    #[clap(
        long = "test-coverage-notes",
        value_name = "TEXT",
        allow_hyphen_values = true
    )]
    pub test_coverage_notes: Option<String>,

    // trace:STORY-776 | ai:claude
    /// Set the execution mode — HOW this spec runs when dispatched by
    /// `aida do`: drain (full auto through merge), drive (auto through CI,
    /// you merge), guided (interactive keystone dialog), operator (you do
    /// the work), or decide (a pending decision blocks). Advisor-authority
    /// write. Pass an empty string to clear (back to ungroomed).
    #[clap(long = "mode", value_name = "MODE")]
    pub mode: Option<String>,

    // trace:STORY-634 | ai:claude
    /// Set where this spec's code lives in a multi-repo shared-store
    /// workspace: a repo slug from the workspace manifest, optionally
    /// with a component ("repo" or "repo/component"). Pass an empty
    /// string to clear (back to single-repo semantics).
    #[clap(long = "origin", value_name = "REPO[/COMPONENT]")]
    pub origin: Option<String>,

    /// Set the numeric weight/score (any finite number, e.g. 0.75 or 42)
    /// — a continuous ranking signal beyond the high/medium/low priority
    /// enum. Pass an empty string to clear.
    // trace:FR-283 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(long, value_name = "N", allow_hyphen_values = true)]
    pub weight: Option<String>,

    /// Use interactive mode (launches editor)
    #[clap(long, short = 'i')]
    pub interactive: bool,

    /// Treat session-lease conflicts as a hard error rather than the
    /// configured default. Equivalent to `[session] enforcement =
    /// "block"` for this single invocation.
    // trace:STORY-48 | ai:claude
    #[clap(long)]
    pub strict: bool,

    /// Bypass the guard that refuses re-opening a Completed or
    /// Rejected requirement (e.g. flipping `--status in-progress`
    /// on something already shipped). Use when intentionally
    /// re-opening: usually you should instead file a new req that
    /// supersedes the closed one.
    // trace:TASK-47 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(long, visible_alias = "replace-tags")]
    pub force: bool,

    /// Mark this spec as work no agent can do — a person-in-the-room
    /// task, sign-off, or physical activity. Pre-pickup gate skips
    /// human-only specs so the orchestrator never spawns a doomed
    /// phase-1 implementer on them.
    // trace:STORY-333 | ai:claude
    #[clap(long, conflicts_with = "no_human_only")]
    pub human_only: bool,

    /// Clear the human-only marker (inverse of `--human-only`). Use
    /// when a spec previously thought to be human-only turns out to
    /// be agent-doable.
    // trace:STORY-333 | ai:claude
    #[clap(long, conflicts_with = "human_only")]
    pub no_human_only: bool,
}

/// The refusal for a status edit on a deferred spec, or `None`. Every status
/// write path a caller drives (`aida edit` both paths, the interactive editor,
/// MCP `update_requirement`) asks this. A legacy `deferred:*` tag defers too:
/// the spec reads `deferred` either way ([`crate::status_display::is_deferred`]).
// trace:TASK-1955 | ai:claude
pub(crate) fn deferred_status_edit_refusal(req: &aida_core::Requirement) -> Option<String> {
    let id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
    if req.deferred {
        return Some(format!(
            "{id} is deferred, and a deferred spec's status reads `deferred` until it is \
             undeferred, so this edit would change nothing anyone sees. Run `aida undefer {id}` \
             first, then set the status."
        ));
    }
    // trace:TASK-1955 | ai:claude (slice 7 round-1 finding 5)
    let tag = req.tags.iter().find(|t| t.starts_with("deferred:"))?;
    Some(format!(
        "{id} is deferred by its legacy `{tag}` tag, and a deferred spec's status reads \
         `deferred`, so this edit would change nothing anyone sees. Remove the tag first \
         (`aida edit {id} --remove-tag {tag}`), then set the status."
    ))
}

#[cfg(test)]
#[path = "tests/task_2174_deferred_status_edit_tests.rs"]
mod task_2174_deferred_status_edit_tests;

fn run_edit(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <EditArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                EditArgs {
                    id,
                    title,
                    description,
                    description_from_file,
                    description_stdin,
                    verify,
                    clear_verify,
                    status,
                    priority,
                    r#type,
                    owner,
                    feature,
                    tags,
                    add_tag,
                    remove_tag,
                    blocked_by,
                    remove_blocked_by,
                    // trace:TASK-1176 | ai:claude
                    superseded_by,
                    // trace:STORY-1434 | ai:claude
                    carve_out,
                    carve_into,
                    carve_reason,
                    add_ref,
                    remove_ref,
                    strict,
                    force,
                    human_only,
                    no_human_only,
                    // trace:TASK-1148 | ai:claude
                    implementation_summary,
                    risk_notes,
                    test_coverage_notes,
                    // trace:STORY-776 | ai:claude
                    mode,
                    // trace:STORY-634 | ai:claude
                    origin,
                    // trace:FR-283 | ai:claude
                    weight,
                    // trace:TASK-1117 | ai:claude
                    interactive,
                    ..
                } => {
                    // trace:TASK-518 | ai:antigravity
                    let mut resolved_id = id.clone();
                    if let Some(pr) = parse_pr_arg(id) {
                        let store = backend.load()?;
                        let project_root = store_path.parent().ok_or_else(|| {
                            anyhow::anyhow!("cannot derive project root from store path")
                        })?;
                        let resolved = resolve_pr_to_spec(project_root, pr, &store)?;
                        println!("editing {} (backs {})", resolved, id);
                        resolved_id = resolved;
                    }
                    let id = &resolved_id;

                    // TASK-1176: naming the successor IS the supersede move — recording
                    // a superseded-by edge without also flipping the status would leave
                    // the two halves able to disagree (exactly the drift the string-tag
                    // workaround suffered). So `--superseded-by <ID>` implies
                    // `--status superseded`. An EXPLICIT `--status` still wins, so a
                    // recovery edit (`--status approved --superseded-by X`) stays
                    // expressible. trace:TASK-1176 | ai:claude
                    let implied_status: Option<String> =
                        if superseded_by.is_some() && status.is_none() {
                            Some("superseded".to_string())
                        } else {
                            None
                        };
                    let status: &Option<String> = if implied_status.is_some() {
                        &implied_status
                    } else {
                        status
                    };

                    // BUG-599: a malformed id is a typo, not an on-disk parse failure
                    // — friendly format hint instead of the bare "Invalid spec_id
                    // format" (or, via the lookup, the version-mismatch wall).
                    // trace:BUG-599 | ai:claude
                    if !is_spec_id_or_uuid(id) {
                        return crate::command_registry::returned(Err(
                            not_found::invalid_spec_id_format(id),
                        ));
                    }
                    // BUG-68: record activity AFTER successful lookup AND
                    // after the TASK-47 terminal-status guard, so a refused
                    // re-open doesn't leave a phantom edit entry.
                    // trace:BUG-68 | ai:claude
                    // BUG-1535: an ambiguous id REFUSES the edit (non-zero exit,
                    // every candidate + its unambiguous handle listed); `aida list`
                    // shows another spec under the same id, so a guess would silently
                    // modify the wrong one. trace:BUG-1535 | ai:claude
                    let mut req = backend
                        .get_requirement_unambiguous(id)?
                        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
                    // TASK-1506: the copy this edit started from. The write below
                    // re-reads the spec under the store lock and applies only what
                    // this edit changed (this copy → `req`) onto that fresh copy.
                    // trace:TASK-1506 | ai:claude
                    let read_copy = req.clone();

                    // TASK-47: refuse to re-open a Completed/Rejected req
                    // without --force. Closing or idempotent re-flips stay
                    // allowed; only Closed → Open transitions trip the guard.
                    // Computed BEFORE the field mutations below so we read the
                    // on-disk status, not the post-edit one.
                    // trace:TASK-47 | ai:claude
                    if let Some(new_status) = status {
                        if is_terminal_status(&req.status) && !*force {
                            // BUG-751: same type-aware alias as the main status path
                            // below, so a terminal decision spec probed with
                            // `accepted` resolves consistently. trace:BUG-751 | ai:claude
                            let canonical =
                                validate_status_input_for_type(new_status, &req.req_type)
                                    .map_err(|e| anyhow::anyhow!(e))?;
                            let mut probe = req.clone();
                            probe.set_status_from_str(canonical);
                            if !is_terminal_status(&probe.status) {
                                let spec_id = req.spec_id.as_deref().unwrap_or("?");
                                // BUG-671: keep the override flag on the FIRST line. In
                                // agent mode only the first line of an error survives as
                                // the TOON `error:` summary, so a `--force` hint on a
                                // continuation line is invisible to the agent that needs
                                // it. trace:BUG-671 | ai:claude
                                anyhow::bail!(
                                    "{} is currently {}. Re-opening a closed requirement is \
                                 usually a mistake — pass --force to override.\n  Otherwise, \
                                 file a new requirement that supersedes {}.",
                                    spec_id,
                                    req.status,
                                    spec_id
                                );
                            }
                        }
                    }

                    // STORY-717: focus-scope drift guard at the In-Progress work-start
                    // moment. Flipping a spec to In Progress in a focused worktree is
                    // "starting work"; if the spec is outside the focus subtree, apply
                    // the [focus] out_of_scope policy (warn nudges + proceeds, block
                    // refuses without --force, off is silent). Only the genuine
                    // transition INTO In Progress is a work-start — re-saving an
                    // already-In-Progress spec or any other status edit is not.
                    // trace:STORY-717 | ai:claude
                    if let Some(status_str) = status {
                        let starts_work = matches!(
                            status_str.to_lowercase().replace('-', "_").as_str(),
                            "in_progress" | "inprogress"
                        ) && req.status
                            != aida_core::RequirementStatus::InProgress;
                        if starts_work {
                            if let Ok(project_root) = find_project_root() {
                                focus_scope_guard(&project_root, backend, &req, *force)?;
                            }
                        }
                    }

                    // STORY-48: lease enforcement. Find leases relative to the git
                    // project root (parent of the orphan store), collect the target +
                    // its ancestor chain for ancestor walking (BUG-634: targeted, not a
                    // full-store load), and consult the [session].enforcement knob.
                    // Best-effort — if we can't even find a project root, skip
                    // enforcement rather than break edit.
                    // trace:STORY-48 | ai:claude
                    if let Ok(project_root) = find_project_root() {
                        if !list_leases(&project_root).is_empty() {
                            // BUG-634: build only the target + its ancestor chain via
                            // targeted lookups — the lease walk needs no more — instead
                            // of a full-store load (a scan of every YAML on the write
                            // path). trace:BUG-634 | ai:claude
                            let store = collect_ancestor_store(backend, &req);
                            // BUG-637: `--force` is the operator override that always
                            // permits an edit/reject of a live-claimed spec (the
                            // EPIC-54-reject escape hatch); `--strict` still escalates
                            // the live-claim warning into a hard block.
                            // trace:BUG-637 | ai:claude
                            enforce_session_lease(
                                &project_root,
                                &req,
                                &store,
                                "aida edit",
                                *strict,
                                *force,
                            )?;
                        }
                    }
                    // BUG-68: all validation gates passed → safe to record the
                    // edit. Uses the canonical spec_id from the resolved req
                    // rather than the user's input (which might be a UUID or
                    // an agreed_id). trace:BUG-68 | ai:claude
                    record_role_activity(req.spec_id.as_deref().unwrap_or(id), "edit");

                    // STORY-647 (team RBAC slice 2): a spec carrying any `[team]
                    // protected_tags` entry may only be edited/transitioned by the
                    // configured `protected_role` (advisor by default). Checked here,
                    // before any field mutation, against the spec's CURRENT tags so a
                    // non-advisor cannot edit OR un-protect a protected spec. `--force`
                    // and advisor authority (TTY / live drain / advisor role) bypass.
                    // Best-effort; no `[team] protected_tags` => no-op (slice-1 behavior).
                    // trace:STORY-647 | ai:claude
                    enforce_protected_spec_gate(req.tags.iter(), *force)?;

                    // trace:BUG-1234 | ai:codex — mirror `aida add`'s safe long-form
                    // sources after lookup/authorization, but before any mutation.
                    let resolved_description = resolve_edit_description(
                        description,
                        description_from_file,
                        *description_stdin,
                    )?;
                    let description = &resolved_description;

                    // TASK-1117: `aida edit <spec>` with NO field flags (or with
                    // `--interactive`) opens the user's editor (AIDA_EDITOR → VISUAL
                    // → EDITOR → platform default) on a git-commit-style markdown
                    // buffer of title + description, and writes the parsed result
                    // back through this same targeted single-spec path. Engages only
                    // at a TTY — a piped/scripted no-flag call keeps the "No changes
                    // specified" hint below. An aborted editor (nonzero exit), an
                    // emptied buffer, or an unchanged buffer leaves the spec
                    // untouched (no spurious commit).
                    // trace:TASK-1117 | ai:claude
                    let no_field_flags = title.is_none()
                    && description.is_none()
                    && verify.is_none()
                    && !*clear_verify
                    && status.is_none()
                    && priority.is_none()
                    && r#type.is_none()
                    && owner.is_none()
                    && feature.is_none()
                    && tags.is_none()
                    && add_tag.is_empty()
                    && remove_tag.is_empty()
                    && blocked_by.is_empty()
                    && remove_blocked_by.is_empty()
                    // trace:TASK-1176 | ai:claude
                    && superseded_by.is_none()
                    // trace:STORY-1434 | ai:claude
                    && carve_out.is_none()
                    && add_ref.is_empty()
                    && remove_ref.is_empty()
                    && !*human_only
                    && !*no_human_only
                    && implementation_summary.is_none()
                    && risk_notes.is_none()
                    && test_coverage_notes.is_none()
                    && mode.is_none()
                    // trace:STORY-634 | ai:claude
                    && origin.is_none()
                    // trace:FR-283 | ai:claude
                    && weight.is_none();
                    let mut editor_title: Option<String> = None;
                    let mut editor_description: Option<String> = None;
                    if (no_field_flags || *interactive)
                        && std::io::IsTerminal::is_terminal(&std::io::stdin())
                        && std::io::IsTerminal::is_terminal(&std::io::stdout())
                    {
                        let display_id = req.spec_id.as_deref().unwrap_or(id);
                        match edit_buffer::edit_spec_in_editor(
                            display_id,
                            &req.title,
                            &req.description,
                        )? {
                            Some((new_title, new_description)) => {
                                if new_title != req.title {
                                    editor_title = Some(new_title);
                                }
                                if new_description != req.description {
                                    editor_description = Some(new_description);
                                }
                                if no_field_flags
                                    && editor_title.is_none()
                                    && editor_description.is_none()
                                {
                                    println!("No changes — {} left untouched.", display_id);
                                    return crate::command_registry::returned(Ok(()));
                                }
                            }
                            None => {
                                println!("Empty buffer — {} left untouched.", display_id);
                                return crate::command_registry::returned(Ok(()));
                            }
                        }
                    }
                    let title = if editor_title.is_some() {
                        &editor_title
                    } else {
                        title
                    };
                    let description = if editor_description.is_some() {
                        &editor_description
                    } else {
                        description
                    };

                    let mut changed = false;
                    let mut force_dropped_structural_tags: Vec<String> = Vec::new();
                    // STORY-1434: (target uuid, target display id) once the
                    // carve-out target has been resolved+validated and the edge +
                    // comment folded into `req` below — carried past the single
                    // save so the inverse edge can be written on the target
                    // afterwards. trace:STORY-1434 | ai:claude
                    let mut carve_target: Option<(uuid::Uuid, String)> = None;
                    // TASK-1450: before/after pairs for the seat-tagged coordination
                    // events emitted once the edit is durably written — captured here
                    // (not reconstructed after the fact) so the recorded values are
                    // exactly what this invocation changed, not a re-derivation from
                    // the saved requirement. trace:TASK-1450 | ai:claude
                    let mut disposition_event: Option<(String, String)> = None;
                    let mut execution_mode_event: Option<(Option<String>, Option<String>)> = None;
                    if let Some(t) = title {
                        req.title = t.clone();
                        changed = true;
                    }
                    if let Some(d) = description {
                        req.description = d.clone();
                        changed = true;
                    }
                    // STORY-1434: carve a named criterion out of the description in
                    // the SAME edit that records where it now lives, so the stale
                    // text and its correction can never drift apart the way a
                    // hand-edit + a separate, easy-to-miss comment did. Applied to
                    // whatever description is current at this point, so `--carve-out`
                    // composes with a plain `--description` pass in one invocation.
                    // Refuses (instead of silently no-op'ing) when the text isn't
                    // found verbatim — a criterion carved from the wrong wording is
                    // worse than an explicit error.
                    //
                    // Atomicity: the target is resolved and validated FIRST — via
                    // `get_requirement_unambiguous` (BUG-1535), so a `--carve-into`
                    // id that names two specs refuses instead of guessing — and the
                    // edge + the CARVE-OUT comment are folded into THIS `req` before
                    // the single save below, alongside the struck description. One
                    // write, not a description change plus a separate follow-up
                    // write that could land only half-applied. trace:STORY-1434
                    if let Some(criterion) = carve_out {
                        let target_id_arg = carve_into
                            .as_deref()
                            .expect("clap requires --carve-into with --carve-out");
                        let target = backend
                            .get_requirement_unambiguous(target_id_arg)?
                            .ok_or_else(|| {
                                not_found::requirement_not_found(target_id_arg, Some(store_path))
                            })?;
                        if target.id == req.id {
                            anyhow::bail!("a spec cannot carve a criterion into itself ({})", id);
                        }
                        let target_display = target
                            .spec_id
                            .clone()
                            .unwrap_or_else(|| target_id_arg.to_string());
                        let source_display = req.spec_id.as_deref().unwrap_or(id).to_string();
                        match carve_out_description(&req.description, criterion, &target_display) {
                            Some(new_description) => {
                                req.description = new_description;
                                apply_carve_out(
                                    &mut req,
                                    target.id,
                                    &source_display,
                                    &target_display,
                                    criterion,
                                    carve_reason.as_deref(),
                                );
                                changed = true;
                                carve_target = Some((target.id, target_display));
                            }
                            None => anyhow::bail!(
                            "--carve-out text not found verbatim in {}'s description — copy the \
                             exact wording (including case and punctuation) from `aida show {}`.",
                            source_display,
                            source_display
                        ),
                        }
                    }
                    // TASK-1148: the three optional narrative fields. An empty string
                    // clears the field (stores None so it drops out of the YAML); any
                    // other value sets it. trace:TASK-1148 | ai:claude
                    let set_narrative = |slot: &mut Option<String>, arg: &Option<String>| -> bool {
                        match arg {
                            Some(text) if text.is_empty() => {
                                if slot.is_some() {
                                    *slot = None;
                                    return true;
                                }
                                false
                            }
                            Some(text) => {
                                *slot = Some(text.clone());
                                true
                            }
                            None => false,
                        }
                    };
                    changed |=
                        set_narrative(&mut req.implementation_summary, implementation_summary);
                    changed |= set_narrative(&mut req.risk_notes, risk_notes);
                    changed |= set_narrative(&mut req.test_coverage_notes, test_coverage_notes);
                    // FR-283: the numeric weight/score. An empty string clears it
                    // (drops out of the YAML); any other value must parse as a finite
                    // number. trace:FR-283 | ai:claude
                    if let Some(w) = weight {
                        if w.trim().is_empty() {
                            if req.weight.is_some() {
                                req.weight = None;
                                changed = true;
                            }
                        } else {
                            let parsed: f32 = w.trim().parse().map_err(|_| {
                                anyhow::anyhow!(
                                    "--weight expects a number (e.g. 0.75 or 42), got `{w}`; \
                                 pass an empty string to clear"
                                )
                            })?;
                            if !parsed.is_finite() {
                                anyhow::bail!("--weight must be a finite number (got {w})");
                            }
                            if req.weight != Some(parsed) {
                                req.weight = Some(parsed);
                                changed = true;
                            }
                        }
                    }
                    // STORY-776: execution_mode is the advisor's bless-time routing
                    // judgment — writing (or clearing) it is an advisor-authority act,
                    // same gate as a status promotion, so the audit trail (the store
                    // commit) records who classified it and when. An empty string
                    // clears back to ungroomed. trace:STORY-776 | ai:claude
                    if let Some(m) = mode {
                        if !has_advisor_authority() {
                            anyhow::bail!(
                            "--mode sets the advisor's routing classification and needs advisor \
                             authority (advisor role or an interactive session). Leave it for \
                             advisor grooming, or run as the advisor.{}",
                            team_role_refusal_clause()
                        );
                        }
                        let mode_before = req.execution_mode.map(|m| m.to_string());
                        if m.trim().is_empty() {
                            if req.execution_mode.is_some() {
                                req.execution_mode = None;
                                changed = true;
                                execution_mode_event = Some((mode_before, None));
                            }
                        } else {
                            let parsed: aida_core::ExecutionMode =
                                m.parse().map_err(|e: String| anyhow::anyhow!(e))?;
                            if req.execution_mode != Some(parsed) {
                                req.execution_mode = Some(parsed);
                                changed = true;
                                execution_mode_event =
                                    Some((mode_before, Some(parsed.to_string())));
                            }
                        }
                    }
                    // STORY-634 (ADR-12): the multi-repo origin dimension. An empty
                    // string clears it (drops out of the YAML); any other value must
                    // parse as "repo" or "repo/component" and the repo slug must name
                    // a repo in the workspace manifest — the canonical join-key
                    // vocabulary linkage resolves against.
                    if let Some(o) = origin {
                        if o.trim().is_empty() {
                            if req.origin.is_some() {
                                req.origin = None;
                                changed = true;
                            }
                        } else {
                            let parsed = aida_core::Origin::parse(o)
                                .map_err(|e: String| anyhow::anyhow!(e))?;
                            let project_root = store_path.parent().ok_or_else(|| {
                                anyhow::anyhow!("cannot derive project root from store path")
                            })?;
                            match aida_core::workspace::WorkspaceManifest::discover(project_root) {
                                Some((_, manifest)) => {
                                    let slugs = manifest.repo_slugs();
                                    if !slugs.iter().any(|s| s.eq_ignore_ascii_case(&parsed.repo)) {
                                        anyhow::bail!(
                                            "origin repo `{}` is not in the workspace manifest \
                                         (known repos: {})",
                                            parsed.repo,
                                            slugs.join(", ")
                                        );
                                    }
                                }
                                None => anyhow::bail!(
                                    "--origin needs a multi-repo workspace manifest \
                                 (.aida-workspace) to validate the repo slug against; \
                                 single-repo stores leave origin unset"
                                ),
                            }
                            if req.origin.as_ref() != Some(&parsed) {
                                req.origin = Some(parsed);
                                changed = true;
                            }
                        }
                    }
                    let mut new_status_for_manifest: Option<String> = None;
                    // STORY-738: did this edit transition the spec INTO Completed?
                    // Captured against the prior status before the set below so the
                    // human render path can fire the completion crescendo instead of
                    // the flat `Updated:` line. trace:STORY-738 | ai:claude
                    let mut into_completed = false;
                    // TASK-358: triage out of NeedsAttention — captured here, applied
                    // after the backend save below. trace:TASK-358 | ai:claude
                    let mut left_needs_attention = false;
                    // STORY-1429: an applied NeedsAttention exit, checked and reported
                    // around the targeted write below. trace:STORY-1429 | ai:claude
                    let mut pending_leave: Option<(
                        String,
                        std::path::PathBuf,
                        crate::requeue::ReturnCtx,
                        RequirementStatus,
                        crate::requeue::ReturnOutcome,
                    )> = None;
                    if let Some(s) = status {
                        // BUG-751: type-aware — a decision spec (ADR) may be moved to
                        // its accepted state with the ADR-native verb `accepted`,
                        // which records as `Approved`. trace:BUG-751 | ai:claude
                        let canonical = validate_status_input_for_type(s, &req.req_type)
                            .map_err(|e| anyhow::anyhow!(e))?;
                        // BUG-626: an EPIC's status is a read-only ROLLUP of its
                        // children, not a manually-set field. Reject ALL manual epic
                        // status edits (substrate-as-bouncer — this is exactly the
                        // hygiene drift a confident setter reintroduces: childless epics
                        // left In Progress, shipping-children epics left Draft). The
                        // displayed status is derived everywhere (list/show/why/status);
                        // `--force` is the recovery escape. This generalizes the
                        // pre-existing "Epic specs cannot be promoted to Approved" guard
                        // (TASK-761) to every transition. trace:BUG-626 | ai:claude
                        if manual_epic_status_edit_forbidden(&req.req_type, *force) {
                            anyhow::bail!(
                            "an epic's status is a read-only rollup of its children, not set by \
                             hand — it moves to In Progress when a child starts, to Done/Completed \
                             when all children finish, and back to Draft when it has no active \
                             children. Change the children's statuses instead (`aida graph tree {}` \
                             shows the rollup). Use --force only for recovery.",
                            req.agreed_id
                                .clone()
                                .or_else(|| req.spec_id.clone())
                                .unwrap_or_else(|| req.id.to_string())
                        );
                        }
                        // A deferred spec reads `deferred` until it is undeferred,
                        // so a status edit would print `Updated` and change nothing
                        // anyone sees. Refuse it, --force included, and name the
                        // undefer. trace:TASK-1955 | ai:claude
                        if let Some(msg) = deferred_status_edit_refusal(&req) {
                            anyhow::bail!(msg);
                        }
                        // STORY-332: enforce the NeedsAttention transition rules —
                        // into it only from In Progress (use `aida punt`), out of it
                        // only to Approved / In Progress / Rejected. Every other
                        // transition is unconstrained, so existing edits don't regress.
                        if let Some(msg) =
                            forbidden_attention_transition(&req.status, &parse_status(canonical)?)
                        {
                            anyhow::bail!(msg);
                        }
                        // TASK-647 (ADR-3): advisor-gate the draft→approved promotion.
                        // A non-advisor, non-TTY caller may not lift a draft into the
                        // approved+ pipeline — that's the intake-triage decision the
                        // advisor (or an interactive human) owns. Execution flips
                        // (Approved→InProgress→Done) are NOT gated, so drains are
                        // unaffected. trace:TASK-647 | ai:claude
                        //
                        // BUG-482: gate `NeedsAttention` as a source too. A punted spec
                        // sits in NeedsAttention awaiting advisor triage;
                        // `forbidden_attention_transition` permits NeedsAttention →
                        // Approved as a triage outcome, but *who* may make that triage
                        // call is still advisor authority. Without NeedsAttention in
                        // the source set, a non-advisor could self-re-approve a spec it
                        // (or the orchestrator) just punted — bypassing the human/advisor
                        // triage the punt exists to request. trace:BUG-482 | ai:claude
                        let new_status = parse_status(canonical)?;
                        if status_advance_requires_advisor_authority(&req.status, &new_status) {
                            // trace:TASK-761 | ai:codex
                            if matches!(new_status, RequirementStatus::Approved)
                                && approval_forbidden_for_type(&req.req_type)
                            {
                                anyhow::bail!(
                                "{} specs cannot be promoted to {}. Leave this class outside the \
                                 approved execution pipeline.",
                                req.req_type,
                                canonical
                            );
                            }
                            if !has_advisor_authority() {
                                anyhow::bail!(
                                "promoting a {} spec to {} needs advisor authority (advisor role or \
                                 an interactive session). Leave it for advisor triage, or run as \
                                 the advisor.{}{}",
                                req.status,
                                canonical,
                                team_role_refusal_clause(),
                                roleless_recovery_sentence()
                            );
                            }
                            // BUG-498: a gated promotion that went through is advisor
                            // work — nudge the operator to seat the advisor role if
                            // they're acting via an env prefix. trace:BUG-498 | ai:claude
                            maybe_hint_advisor_seat();
                        }
                        let was_needs_attention =
                            matches!(req.status, RequirementStatus::NeedsAttention);
                        // STORY-738: capture the into-Completed transition against the
                        // prior status (before the set). trace:STORY-738 | ai:claude
                        into_completed = is_into_completed_transition(&req.status, canonical);
                        // TASK-1450: disposition before/after for the coordination
                        // event, captured against the on-disk status before the set
                        // below (same "before the mutation" rule STORY-738 uses).
                        // trace:TASK-1450 | ai:claude
                        let status_before = req.status.to_string();
                        // TASK-1477: captured before the mutation so a reopen (this
                        // spec was Completed, canonical isn't) can clear the stale
                        // completed_at stamp below. trace:TASK-1477 | ai:claude
                        let prior_status_for_reopen = req.status.clone();
                        // STORY-1418: an into-Completed edit stamps through the seam;
                        // the ship record is emitted below once the write lands.
                        // trace:STORY-1418 | ai:claude
                        if canonical == "Completed" {
                            crate::completion::mark_completed(&mut req);
                        } else {
                            req.set_status_from_str(canonical);
                            // TASK-1477: reopening a Completed spec must clear the
                            // stale completed_at so the next completion stamps a
                            // fresh date instead of keeping the first one forever.
                            // trace:TASK-1477 | ai:claude
                            crate::completion::record_reopen(
                                &mut req,
                                &prior_status_for_reopen,
                                find_project_root().ok().as_deref(),
                            );
                        }
                        disposition_event = Some((status_before, req.status.to_string()));
                        // STORY-332 / EPIC-28: a spec triaged out of NeedsAttention is
                        // no longer paused — drop the now-stale punt metadata AND any
                        // orchestrator-shelving metadata. The punt ledger
                        // (`.aida/punts.jsonl`) keeps the durable history for both.
                        // trace:EPIC-28 | ai:claude
                        //
                        // TASK-1311: the clearing lives in `requeue::clear_shelve_markers`
                        // so `aida edit` and `aida queue rework` return a spec to flight
                        // identically. It also drops the `needs-human` tag an advisor
                        // escalation wrote, which is a burndown parking tag: without
                        // that, a triaged spec stayed out of the drain for good. The
                        // comment records why the spec came back (auditable re-entry).
                        // trace:TASK-1311 | ai:claude
                        //
                        // STORY-1429: the exit runs through the one owner,
                        // `requeue::return_to_flight`, on this one spec. The status is
                        // re-read and compared just before the single targeted write
                        // below, so a spec that moved in between (a concurrent requeue
                        // or a drain) is left alone. The requeue lease gate applies
                        // here too.
                        // trace:STORY-1429 | ai:claude
                        if was_needs_attention
                            && !matches!(req.status, RequirementStatus::NeedsAttention)
                        {
                            let spec_label = req.spec_id.clone().unwrap_or_else(|| id.to_string());
                            let lease_root = store_path
                                .parent()
                                .filter(|p| !p.as_os_str().is_empty())
                                .map(std::path::Path::to_path_buf)
                                .or_else(|| find_project_root().ok())
                                .unwrap_or_else(|| std::path::PathBuf::from("."));
                            let display = req.display_id();
                            let check = crate::requeue_lease_gate(
                                &lease_root,
                                &[spec_label.as_str(), display.as_str()],
                            );
                            if let Some(msg) = check.refusal(&spec_label) {
                                anyhow::bail!(msg);
                            }
                            // The escalation tag is a hand-off to a human; only a
                            // human at a terminal may clear it (not a non-TTY advisor
                            // agent, not an orchestrated phase).
                            let ctx = crate::requeue::ReturnCtx {
                                via: "`aida edit --status`".to_string(),
                                via_slug: "edit",
                                author: get_default_author(),
                                clear_escalation: crate::requeue::caller_may_clear_escalation(),
                                reason: None,
                                automated: false, // trace:BUG-1632 | ai:claude
                            };
                            let target = req.status.clone();
                            req.status = RequirementStatus::NeedsAttention;
                            let outcome = crate::requeue::return_to_flight(
                                &mut req,
                                &RequirementStatus::NeedsAttention,
                                &target,
                                &ctx,
                            );
                            pending_leave = Some((spec_label, lease_root, ctx, target, outcome));
                            left_needs_attention = true;
                        } else {
                            // BUG-1637: every other status edit is recorded under the
                            // caller through the one shared history helper (leaving
                            // NeedsAttention is recorded by `return_to_flight` above).
                            // trace:BUG-1637 | ai:claude
                            crate::record_caller_status_transition(
                                &mut req,
                                &prior_status_for_reopen,
                            );
                        }
                        changed = true;
                        new_status_for_manifest = Some(canonical.to_string());
                    }
                    if let Some(p) = priority {
                        let canonical =
                            validate_priority_input(p).map_err(|e| anyhow::anyhow!(e))?;
                        req.set_priority_from_str(canonical);
                        changed = true;
                    }
                    if let Some(t) = r#type {
                        // BUG-48: an invalid --type used to be silently dropped,
                        // making the user think they passed no flags ("No changes
                        // specified" — misleading). Propagate the error so they
                        // see "Unknown requirement type: <input>" with the same
                        // valid-list hint as --status / --priority.
                        let rt = parse_requirement_type(t).map_err(|e| {
                        anyhow::anyhow!(
                            "{} — expected one of: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc, faq",
                            e
                        )
                    })?;
                        // BUG-49: spec_id is stable; type changes don't auto-
                        // renumber. Warn when the new type implies a different
                        // prefix so the user isn't surprised by FR-4 still saying
                        // FR-4 after `--type principle`.
                        if let Some(ref sid) = req.spec_id {
                            let new_prefix = rt.default_prefix();
                            let current_prefix = sid.split('-').next().unwrap_or("");
                            if !current_prefix.is_empty() && current_prefix != new_prefix {
                                eprintln!(
                                    "{} type changed to `{}` but spec_id stays `{}` — \
                                 AIDA spec_ids are stable so existing trace comments \
                                 and commit refs keep working. To get a `{}-N` id, \
                                 recreate the requirement with the new type.",
                                    "Note:".dimmed(),
                                    t,
                                    sid,
                                    new_prefix
                                );
                            }
                        }
                        req.req_type = rt;
                        changed = true;
                    }
                    // After --type so `--type constraint --verify ...` works in one
                    // call. No probe runs while authoring the data-only block.
                    // trace:STORY-1673 | ai:claude
                    if let Some(change) =
                        verify_cmd::apply_verify_edit(&mut req, verify.as_deref(), *clear_verify)?
                    {
                        let author = get_default_author();
                        req.record_change(author, vec![change]);
                        changed = true;
                    }
                    if let Some(o) = owner {
                        req.owner = o.clone();
                        changed = true;
                    }
                    if let Some(f) = feature {
                        req.feature = f.clone();
                        changed = true;
                    }
                    if let Some(t) = tags {
                        // BUG-545: `--tags` REPLACES the whole set — a frequent footgun
                        // when someone means "add one tag". Warn loudly (stderr) showing
                        // old→new so a silent clobber of provenance/routing tags is
                        // visible. `--add-tag` / `--remove-tag` are the incremental forms
                        // (kept mutually exclusive with `--tags` by clap's conflicts_with).
                        // trace:BUG-545 | ai:claude
                        // BUG-1770: parse and validate the replacement set BEFORE
                        // clearing the old one — a refusal must leave the spec's tags
                        // exactly as they were. trace:BUG-1770 | ai:claude
                        let replacement = parse_tag_list(t)?;
                        let old_tags = req.tags.clone();
                        req.tags.clear();
                        req.tags.extend(replacement);
                        // trace:BUG-1252 | ai:codex
                        force_dropped_structural_tags =
                            enforce_structural_tag_replacement(&old_tags, &req.tags, *force)?;
                        if let Some(msg) = tags_replace_warning(&old_tags, &req.tags) {
                            eprintln!(
                                "  {} {}",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                                msg
                            );
                        }
                        changed = true;
                    }
                    // TASK-351: partial tag edits that don't clobber the rest.
                    // `--tags` is full-replace; `--add-tag` / `--remove-tag` are
                    // additive/subtractive. Clap's `conflicts_with` keeps the two
                    // forms mutually exclusive. Adding a present tag or removing
                    // an absent one is a graceful no-op.
                    // trace:TASK-351 | ai:claude
                    // BUG-1770: keep the REPORT, not just the bool. A `--remove-tag`
                    // that matched nothing used to fall through to the generic
                    // "No changes specified" line, which tells the caller to pass a
                    // flag they already passed. trace:BUG-1770 | ai:claude
                    let tag_report = apply_tag_deltas_report(&mut req.tags, add_tag, remove_tag)?;
                    if tag_report.changed() {
                        changed = true;
                    }
                    // TASK-524: typo guard — a `lifecycle:*` tag that isn't a recognized
                    // short-circuit tag silently no-ops at drain time (it's just an
                    // unread string). Warn on any misspelling the user passed via
                    // `--tags` or `--add-tag` so it's caught at edit time, not when a
                    // drain quietly fails to skip the phase. trace:TASK-524 | ai:claude
                    {
                        let passed: Vec<String> = tags
                            .iter()
                            .flat_map(|t| t.split(',').map(|s| s.trim().to_string()))
                            .chain(add_tag.iter().cloned())
                            .filter(|s| !s.is_empty())
                            .collect();
                        for tag in &passed {
                            if auto_complete::is_unrecognized_lifecycle_tag(tag) {
                                eprintln!(
                                "  {} unrecognized lifecycle tag `{}` — it will NOT short-circuit \
                                 any phase. Valid: {}",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                                tag,
                                auto_complete::RECOGNIZED_LIFECYCLE_TAGS.join(", ")
                            );
                            }
                        }
                    }
                    // STORY-333: `--human-only` / `--no-human-only` flip the typed
                    // marker that the pickability gate consults. Clap's
                    // `conflicts_with` keeps the two flags mutually exclusive, so
                    // at most one branch fires per invocation.
                    // trace:STORY-333 | ai:claude
                    if *human_only && !req.human_only {
                        req.human_only = true;
                        changed = true;
                    }
                    if *no_human_only && req.human_only {
                        req.human_only = false;
                        changed = true;
                    }

                    // STORY-476: one-way external issue refs. `--add-ref` validates
                    // each `provider:id` (linear/jira/github) before storing the
                    // canonical form; an invalid ref is a hard error so a typo'd ref
                    // never lands silently. `--remove-ref` drops a stored ref by its
                    // canonical form. Both are repeatable and idempotent.
                    // trace:STORY-476 | ai:claude
                    for raw in add_ref {
                        let parsed = aida_core::external_refs::parse_ref(raw)
                            .map_err(|e| anyhow::anyhow!(e))?;
                        let canonical = parsed.canonical();
                        if !req.external_refs.contains(&canonical) {
                            req.external_refs.push(canonical);
                            changed = true;
                        }
                    }
                    for raw in remove_ref {
                        // Accept either the canonical or a loosely-typed form by
                        // normalizing through the parser when possible; fall back to a
                        // trimmed literal match so a stored-but-now-unknown provider
                        // can still be removed.
                        let target = aida_core::external_refs::parse_ref(raw)
                            .map(|p| p.canonical())
                            .unwrap_or_else(|_| raw.trim().to_string());
                        let before = req.external_refs.len();
                        req.external_refs.retain(|r| r != &target);
                        if req.external_refs.len() != before {
                            changed = true;
                        }
                    }

                    if changed {
                        req.modified_at = chrono::Utc::now();
                        // STORY-1429 / TASK-1506: one targeted write through the
                        // per-spec compare-and-swap. Under the store lock the spec is
                        // re-read, a NeedsAttention exit is re-checked against that
                        // copy (nothing is written when the spec left NeedsAttention
                        // meanwhile), and this edit's field changes are applied to it,
                        // so a concurrent change to another field is kept and a
                        // same-field clash refuses instead of being overwritten.
                        // trace:STORY-1429 trace:TASK-1506 | ai:claude
                        let label = req.spec_id.clone().unwrap_or_else(|| id.to_string());
                        let leave_target =
                            pending_leave.as_ref().map(|(_, _, _, target, _)| target);
                        let subject = (!force_dropped_structural_tags.is_empty()).then(|| {
                            format!(
                                "update {}: replaced tags, dropped: {}",
                                label,
                                force_dropped_structural_tags.join(", ")
                            )
                        });
                        let leave_label = pending_leave
                            .as_ref()
                            .map(|(l, _, _, _, _)| l.as_str())
                            .unwrap_or(label.as_str());
                        req = crate::edit_rebase::write_edit_atomically(
                            backend,
                            leave_label,
                            &read_copy,
                            &req,
                            leave_target,
                            crate::edit_rebase::EditMerge {
                                // `--tags` replaces the set: refuse on a concurrent
                                // tag change instead of merging.
                                tags_replaced: tags.is_some(),
                                // `--status` refuses on a concurrent move to any
                                // other status, even a same-value request.
                                status_set: status.is_some(),
                                ..Default::default()
                            },
                            // STORY-647 re-run on the copy read under the lock: a
                            // protected tag added meanwhile blocks this edit.
                            // trace:TASK-1506 trace:STORY-647 | ai:claude
                            &|cur: &aida_core::Requirement| {
                                enforce_protected_spec_gate(cur.tags.iter(), *force)
                            },
                            subject.as_deref(),
                        )?;
                        // STORY-1429: the exit landed; report what it cleared and
                        // record the requeue. trace:STORY-1429 | ai:claude
                        if let Some((label, root, ctx, _, outcome)) = &pending_leave {
                            if let crate::requeue::ReturnOutcome::Returned { cleared, .. } = outcome
                            {
                                if !cleared.removed_tags.is_empty() {
                                    eprintln!(
                                    "  {} cleared stale parking tag(s) {} so the drain can pick it up again",
                                    "·".dimmed(),
                                    cleared.removed_tags.join(", ")
                                );
                                }
                                if let Some(w) =
                                    crate::requeue::kept_escalation_warning(label, cleared)
                                {
                                    eprintln!("  {} {w}", "Warning:".yellow().bold());
                                }
                            }
                            crate::requeue::emit_requeued(root, label, ctx, outcome);
                        }
                        // TASK-1450: a disposition (status) or execution_mode change
                        // made through `aida edit` is a coordination-seat decision —
                        // approve/reject/defer and groom's `--mode` writes all land
                        // here (groom/approve/reject shell out to `aida edit`
                        // themselves). Emit AFTER the write above lands, mirroring
                        // BUG-1423's PrMerged placement, so a failed write never
                        // produces a phantom event. Best-effort — `events::emit`
                        // itself never fails the edit. trace:TASK-1450 | ai:claude
                        if let Some(project_root) = store_path.parent() {
                            let display_id = req.spec_id.as_deref().unwrap_or(id);
                            if let Some((before, after)) = disposition_event.take() {
                                emit_edit_disposition_changed(
                                    project_root,
                                    display_id,
                                    before,
                                    after,
                                );
                            }
                            if let Some((before, after)) = execution_mode_event.take() {
                                emit_edit_execution_mode_changed(
                                    project_root,
                                    display_id,
                                    before,
                                    after,
                                );
                            }
                        }
                        // STORY-738: a transition INTO Completed is the payoff state —
                        // render the felt completion crescendo instead of the flat
                        // generic `Updated:` line (reused for any tag edit). Only the
                        // human surface gets it; the agent/TOON path and every
                        // non-completion edit keep `Updated:`. The workflow-hint line
                        // below is unaffected. trace:STORY-738 | ai:claude
                        // BUG-1286 F1: `aida edit --status completed` is the third
                        // into-Completed path and emitted nothing. `into_completed` was
                        // already computed here for the STORY-738 render against the
                        // PRIOR status, so the transition test is reused rather than
                        // recomputed. trace:BUG-1286 | ai:claude
                        if into_completed {
                            if let Some(project_root) = store_path.parent() {
                                crate::completion::emit_spec_completed(
                                    project_root,
                                    req.spec_id.as_deref().unwrap_or(id),
                                    "",
                                    None,
                                    "edit",
                                );
                            }
                        }
                        match edit_completion_render(into_completed, agent_output_mode()) {
                            EditCompletionRender::Crescendo => {
                                let display_id = req.spec_id.as_deref().unwrap_or(id);
                                render_completion_crescendo(display_id, &req.title);
                            }
                            EditCompletionRender::Updated => println!("Updated: {}", id),
                        }
                        // STORY-1649: a code spec completed with nothing shipped on the
                        // default branch (local or origin) gets a warning, never a
                        // refusal. trace:STORY-1649 | ai:claude
                        if into_completed {
                            if let Some(root) = store_path.parent() {
                                crate::shipped_linkage::warn_if_completed_without_shipped_commit(
                                    root, &req,
                                );
                            }
                        }

                        // BUG-1770: say what the tag flags actually did, on the success
                        // path too. A caller who removes two tags and mistypes one
                        // currently sees only "Updated" and cannot tell. The lines are
                        // emitted whenever tag flags were passed, including the partial
                        // case where some matched and some did not.
                        // trace:BUG-1770 | ai:claude
                        for line in tag_report.summary_lines() {
                            println!("  {line}");
                        }

                        // TASK-928 (SPIKE-71): a tag edit that introduces a
                        // `parent:<SPEC-ID>` tag must materialize the real bidirectional
                        // edge too (same gap as `aida add`). Only fires when tags were
                        // touched. Additive + lenient: tag stays, bad target = no-op.
                        // trace:TASK-928 | ai:claude
                        if tags.is_some() || !add_tag.is_empty() {
                            if let Some(sid) = req.spec_id.as_deref() {
                                match ensure_parent_edge_from_tag(backend, sid) {
                                    Ok(Some(pdisp)) => {
                                        println!(
                                            "  Linked: {} → parent of {} (from parent: tag)",
                                            pdisp, sid
                                        )
                                    }
                                    Ok(None) => {}
                                    Err(e) => eprintln!(
                                        "  {} could not link parent from tag: {}",
                                        "Warning:".yellow().bold(),
                                        e
                                    ),
                                }
                            }
                        }

                        // STORY-98: if this edit changed the status AND a session
                        // manifest covers the active session, flip the manifest
                        // row so `aida session show --plan` and the
                        // `[planned:by-X]` chip stay coherent with the store.
                        // Best-effort — manifest errors don't fail the edit.
                        // trace:STORY-98 | ai:claude
                        if let Some(status_str) = &new_status_for_manifest {
                            if let Some(spec_id) = req.spec_id.as_deref() {
                                update_manifest_for_status(spec_id, status_str);
                            }
                        }

                        // STORY-106: workflow hint when a status flip to Completed
                        // emptied the queue for the active role+scope. Mirrors the
                        // queue-done hint so direct `aida edit --status completed`
                        // surfaces the same Next-step. Best-effort.
                        // trace:STORY-106 | ai:claude
                        if new_status_for_manifest.as_deref() == Some("Completed") {
                            let storage = Storage::new(store_path);
                            let user_id = current_user_id(None);
                            maybe_hint_after_queue_drain(&storage, &user_id);
                        }

                        // BUG-378: substrate-as-bouncer — direct `aida edit --status
                        // done|completed` is the other path an agent uses to declare
                        // work shipped, so the pending-brief banner must fire here
                        // too. Mirrors the queue-done hookup. trace:BUG-378 | ai:claude
                        if matches!(
                            new_status_for_manifest.as_deref(),
                            Some("Done") | Some("Completed")
                        ) {
                            if let Ok(project_root) = find_project_root() {
                                warn_pending_briefs_for_running_agent(&project_root);
                            }
                        }

                        // BUG-238: file plan `## Followups` when `aida edit` directly
                        // transitions a spec into Done/Completed. `aida queue done`
                        // and the STORY-86 auto-bump already trigger the parse; the
                        // direct-edit path (used by `/aida-review` step 10) was the
                        // hole — followups were silently lost. Idempotent via the
                        // [aida:followups] marker, so whichever path runs first wins.
                        // Interactive only when stdin is a TTY; non-TTY (skill /
                        // script context) files all bullets so the silent-loss case
                        // that motivated this fix actually surfaces them.
                        // trace:BUG-238 | ai:claude
                        if matches!(
                            new_status_for_manifest.as_deref(),
                            Some("Done") | Some("Completed")
                        ) {
                            if let (Ok(project_root), Some(spec_id)) =
                                (find_project_root(), req.spec_id.as_deref())
                            {
                                let storage = Storage::new(store_path);
                                let interactive =
                                    std::io::IsTerminal::is_terminal(&std::io::stdin());
                                if let Err(e) = extract_plan_followups(
                                    &storage,
                                    &project_root,
                                    spec_id,
                                    spec_id,
                                    interactive,
                                ) {
                                    eprintln!(
                                        "{} followup extraction skipped: {}",
                                        "Warning:".yellow().bold(),
                                        e
                                    );
                                }
                            }
                        }

                        // TASK-358: triage out of NeedsAttention — clean up any
                        // orchestrator-escalated worktree for this spec. The lease's
                        // `escalated_to_human` marker (stamped by the
                        // `--escalate-blocks` path) is the safety gate: an
                        // interactive user session on the same spec, or an
                        // `--escalate-defaults` advisor-resume, has the marker
                        // absent and is left alone. trace:TASK-358 | ai:claude
                        if left_needs_attention {
                            if let Ok(project_root) = find_project_root() {
                                let spec_id = req.spec_id.as_deref().unwrap_or(id);
                                cleanup_escalated_leases_for_spec(&project_root, spec_id);
                                // BUG-674: a spec triaged out of NeedsAttention no
                                // longer has an open punt awaiting triage — close its
                                // ledger record so the punt ledger reflects reality
                                // instead of a permanent "awaiting triage" row.
                                // trace:BUG-674 | ai:claude
                                let _ = punt::close_open_records(
                                    &project_root,
                                    spec_id,
                                    punt::RESOLUTION_HUMAN_RESOLVED,
                                    "resolved",
                                    None,
                                    Some("resume"),
                                );
                            }
                        }
                    } else if blocked_by.is_empty()
                    && remove_blocked_by.is_empty()
                    // trace:TASK-1176 | ai:claude
                    && superseded_by.is_none()
                    // trace:STORY-1434 | ai:claude
                    && carve_out.is_none()
                    {
                        // BUG-1770: only say "no changes SPECIFIED" when none were.
                        // If tag flags were passed and matched nothing, say THAT —
                        // conflating the two is what made a no-op indistinguishable
                        // from success and hid this class long enough to be found by
                        // accident. trace:BUG-1770 | ai:claude
                        let tag_lines = tag_report.summary_lines();
                        if tag_lines.is_empty() {
                            println!(
                                "No changes specified. Use --title, --status, --priority, etc."
                            );
                        } else {
                            for line in tag_lines {
                                println!("{line}");
                            }
                        }
                    }

                    // TASK-1176: record the supersede lineage AFTER the scalar save,
                    // for the same reason the blocked-by block below does — the helper
                    // re-reads the persisted spec, so writing the edge first would be
                    // clobbered by the status save. trace:TASK-1176 | ai:claude
                    if let Some(successor) = superseded_by {
                        let edit_spec = req.spec_id.as_deref().unwrap_or(id).to_string();
                        match crate::add_superseded_by_edge(backend, &edit_spec, successor) {
                            Ok(disp) => println!("  Superseded by: {}", disp.cyan()),
                            Err(e) => eprintln!(
                                "  {} could not record superseded-by {}: {}",
                                "Warning:".yellow().bold(),
                                successor,
                                e
                            ),
                        }
                    } else if matches!(req.status, RequirementStatus::Superseded)
                        && !req.relationships.iter().any(|r| {
                            matches!(
                                r.rel_type,
                                aida_core::models::RelationshipType::SupersededBy
                            )
                        })
                    {
                        // TASK-1176: "superseded" without a named successor is a
                        // half-recorded move — the state says something replaced this,
                        // but nothing says WHAT. Nudge rather than refuse: the successor
                        // may not be filed yet. trace:TASK-1176 | ai:claude
                        eprintln!(
                        "{}",
                        format!(
                            "  Note: no successor recorded — add one with `aida edit {} --superseded-by <SPEC-ID>`.",
                            req.spec_id.as_deref().unwrap_or(id)
                        )
                        .dimmed()
                    );
                    }

                    // STORY-1434: the carved-out-to edge + CARVE-OUT comment already
                    // landed on `req` and were saved atomically above (with the
                    // struck description); only the INVERSE carved-from edge on the
                    // TARGET remains, written after the source's save the same way
                    // the blocked-by/supersede edges below are. `target` is
                    // re-fetched by UUID — never re-resolved from the user-typed
                    // `--carve-into` id — so this cannot rediscover an ambiguity;
                    // a failure here (backend error, target vanished) is a real
                    // error, not a warning: a one-sided carve-out (source struck, no
                    // reciprocal edge) is exactly the half-applied state this spec
                    // exists to prevent. trace:STORY-1434 | ai:claude
                    if let Some((target_id, target_display)) = carve_target {
                        let source_display = req.spec_id.as_deref().unwrap_or(id).to_string();
                        crate::add_carved_from_edge(
                            backend,
                            target_id,
                            &target_display,
                            req.id,
                            &source_display,
                        )?;
                        println!("  Carved out → {}", target_display.cyan());
                    }

                    // STORY-446: apply blocked-by edge add/remove AFTER any scalar edit
                    // has been saved, so the helper operates on the persisted spec
                    // (avoiding a save that would clobber the new edge). Runs whether or
                    // not a scalar field changed. trace:STORY-446 | ai:claude
                    if !blocked_by.is_empty() || !remove_blocked_by.is_empty() {
                        let edit_spec = req.spec_id.as_deref().unwrap_or(id).to_string();
                        for blocker in blocked_by {
                            match add_blocked_by_edge(backend, &edit_spec, blocker) {
                                Ok(disp) => println!("  Blocked by: {}", disp.cyan()),
                                Err(e) => eprintln!(
                                    "  {} could not add blocked-by {}: {}",
                                    "Warning:".yellow().bold(),
                                    blocker,
                                    e
                                ),
                            }
                        }
                        for blocker in remove_blocked_by {
                            match remove_blocked_by_edge(backend, &edit_spec, blocker) {
                                Ok(disp) => println!("  Removed blocked-by: {}", disp.cyan()),
                                Err(e) => eprintln!(
                                    "  {} could not remove blocked-by {}: {}",
                                    "Warning:".yellow().bold(),
                                    blocker,
                                    e
                                ),
                            }
                        }
                    }

                    // TASK-974 (AXI #9): agent-mode next-step block — the valid next
                    // transition(s) for the spec's (now-current) status, templated with
                    // its id. e.g. after `aida edit X --status approved` -> suggest
                    // `aida queue work X`. The human TTY path is unchanged. trace:TASK-974
                    if agent_output_mode() {
                        let sid = req.spec_id.as_deref().unwrap_or(id);
                        let next = crate::help_next::spec_next(&req.status.to_string(), sid);
                        if let Some(block) = crate::help_next::render(&next) {
                            println!("{block}");
                        }
                    }
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                EditArgs {
                    id,
                    title,
                    description,
                    description_from_file,
                    description_stdin,
                    verify,
                    clear_verify,
                    status,
                    priority,
                    r#type,
                    owner,
                    feature,
                    tags,
                    add_tag,
                    remove_tag,
                    blocked_by: _, // STORY-446: blocked-by edges land on the git-backend path
                    remove_blocked_by: _,
                    // TASK-1176: the supersede lineage edge is git-canonical only, same
                    // rule as blocked-by. trace:TASK-1176 | ai:claude
                    superseded_by: _,
                    // STORY-1434: the carve-out edge + comment are git-canonical
                    // only, same rule as superseded_by. trace:STORY-1434 | ai:claude
                    carve_out: _,
                    carve_into: _,
                    carve_reason: _,
                    // STORY-476: external refs land on the git-backend path only;
                    // the legacy SQLite path ignores them. trace:STORY-476 | ai:claude
                    add_ref: _,
                    remove_ref: _,
                    interactive,
                    strict: _, // legacy SQLite path ignores session leases
                    force: _,  // TASK-47 guard only applies to git-canonical Edit
                    // STORY-333: legacy centralized backend does not persist the
                    // human-only marker; ignore the flags here.
                    // trace:STORY-333 | ai:claude
                    human_only: _,
                    no_human_only: _,
                    // TASK-1148: the narrative fields are git-canonical only; the legacy
                    // centralized backend does not persist them. trace:TASK-1148 | ai:claude
                    implementation_summary: _,
                    risk_notes: _,
                    test_coverage_notes: _,
                    // STORY-776: execution_mode is git-canonical only, same rule.
                    mode: _,
                    // STORY-634: the multi-repo origin dimension is git-canonical
                    // only, same rule.
                    origin: _,
                    // FR-283: the numeric weight is git-canonical only, same rule.
                    weight: _,
                } => {
                    // trace:STORY-1673 | ai:codex
                    anyhow::ensure!(
                        verify.is_none() && !*clear_verify,
                        "--verify and --clear-verify require a git-canonical store"
                    );

                    // trace:BUG-1234 | ai:codex — keep legacy and git-canonical edit
                    // behavior aligned for file/stdin description replacement.
                    let resolved_description = resolve_edit_description(
                        description,
                        description_from_file,
                        *description_stdin,
                    )?;
                    let description = &resolved_description;
                    // If any flags provided, use non-interactive mode; otherwise interactive
                    // trace:TASK-351 | ai:claude — --add-tag / --remove-tag count too
                    let has_flags = title.is_some()
                        || description.is_some()
                        || status.is_some()
                        || priority.is_some()
                        || r#type.is_some()
                        || owner.is_some()
                        || feature.is_some()
                        || tags.is_some()
                        || !add_tag.is_empty()
                        || !remove_tag.is_empty();

                    if *interactive || !has_flags {
                        edit_requirement_interactive(&storage, id)?;
                    } else {
                        edit_requirement_cli(
                            &storage,
                            id,
                            title,
                            description,
                            status,
                            priority,
                            r#type,
                            owner,
                            feature,
                            tags,
                            add_tag,
                            remove_tag,
                        )?;
                    }
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static EDIT: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "edit",
    help_after: "done",
    command: <EditArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::EditRequirement,
        per_write_auto_push: true,
        skip_role_restore_prompt: false,
        team_identity_write: true,
    },
    run: crate::command_registry::Handler::Store(run_edit),
};

/// Delete a requirement
#[derive(clap::Parser, Debug)]
#[clap(name = "del")]
pub(crate) struct DelArgs {
    /// The ID (UUID or SPEC-ID) of the requirement to delete
    pub id: String,

    /// Skip confirmation prompt
    #[clap(long, short = 'y')]
    pub yes: bool,
}

fn run_del(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <DelArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                DelArgs { id, yes } => {
                    let req = backend
                        .get_requirement_unambiguous(id)?
                        .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;

                    if !yes {
                        println!("Delete {} — {}?", id, req.title);
                        print!("Confirm (y/N): ");
                        use std::io::Write;
                        std::io::stdout().flush()?;
                        let mut input = String::new();
                        std::io::stdin().read_line(&mut input)?;
                        if !input.trim().eq_ignore_ascii_case("y") {
                            println!("Cancelled.");
                            return crate::command_registry::returned(Ok(()));
                        }
                    }

                    backend.delete_requirement(&req.id)?;
                    println!("Deleted: {}", id);
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                DelArgs { id, yes } => {
                    delete_requirement(&storage, id, *yes)?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static DEL: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "del",
    help_after: "edit",
    command: <DelArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: true,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Store(run_del),
};

/// Search requirements for a pattern (like grep)
#[derive(clap::Parser, Debug)]
#[clap(name = "grep")]
#[clap(hide = true)]
pub(crate) struct GrepArgs {
    /// Pattern to search for (regex supported with -E)
    pub pattern: String,

    /// Case insensitive search
    #[clap(long, short = 'i')]
    pub ignore_case: bool,

    /// Use extended regex (ERE)
    #[clap(long, short = 'E')]
    pub extended_regex: bool,

    /// Show N lines of context after match
    #[clap(long, short = 'A', default_value = "0")]
    pub after_context: usize,

    /// Show N lines of context before match
    #[clap(long, short = 'B', default_value = "0")]
    pub before_context: usize,

    /// Show N lines of context before and after match
    #[clap(long, short = 'C')]
    pub context: Option<usize>,

    /// Search only in specific field(s): title, description, comments, tags, owner, feature
    #[clap(long, short = 'f')]
    pub field: Option<String>,

    /// Filter by status
    #[clap(long)]
    pub status: Option<String>,

    /// Filter by type
    #[clap(long)]
    pub r#type: Option<String>,

    /// Filter by feature
    #[clap(long)]
    pub feature: Option<String>,

    /// Only show matching SPEC-IDs (like grep -l)
    #[clap(long, short = 'l')]
    pub files_with_matches: bool,

    /// Show match count per requirement (like grep -c)
    #[clap(long, short = 'c')]
    pub count: bool,

    /// Invert match (show non-matching)
    #[clap(long, short = 'v')]
    pub invert_match: bool,
}

fn run_grep(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <GrepArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            let _ = (store_path, backend, &args);
            crate::git_backend_cmd::exit_unsupported_on_git_backend()
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                GrepArgs {
                    pattern,
                    ignore_case,
                    extended_regex,
                    after_context,
                    before_context,
                    context,
                    field,
                    status,
                    r#type,
                    feature,
                    files_with_matches,
                    count,
                    invert_match,
                } => {
                    grep_requirements(
                        &storage,
                        pattern,
                        *ignore_case,
                        *extended_regex,
                        *after_context,
                        *before_context,
                        *context,
                        field.as_deref(),
                        status.as_deref(),
                        r#type.as_deref(),
                        feature.as_deref(),
                        *files_with_matches,
                        *count,
                        *invert_match,
                    )?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static GREP: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "grep",
    help_after: "doc",
    command: <GrepArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Store(run_grep),
};

/// Simple search for requirements (case-insensitive by default)
#[derive(clap::Parser, Debug)]
#[clap(name = "search")]
pub(crate) struct SearchArgs {
    /// Search query (searches title, description, and comments)
    pub query: String,

    /// Make search case-sensitive (default is case-insensitive)
    #[clap(long, short = 's')]
    pub case_sensitive: bool,

    /// Filter by status
    #[clap(long)]
    pub status: Option<String>,

    /// Filter by feature
    #[clap(long)]
    pub feature: Option<String>,

    /// Maximum number of matches to return (default: 200)
    #[clap(long, short = 'n', default_value = "200")]
    pub limit: usize,

    /// Pull from `origin/aida-store` before searching. See
    /// `aida list --sync`.
    // trace:STORY-78 | ai:claude
    #[clap(long)]
    pub sync: bool,

    /// Include the seeded META AI-prompt specs in results. Default hides
    /// them, consistent with `aida list`.
    // trace:BUG-488 | ai:claude
    #[clap(long)]
    pub include_meta: bool,

    /// Include archived AND deferred requirements in the search
    /// (everything-escape-hatch). Default excludes both for consistency
    /// with `aida list`.
    // trace:STORY-441 | ai:claude
    // trace:STORY-584 | ai:claude — now widens the defer axis too.
    #[clap(long, conflicts_with_all = ["archived", "deferred"])]
    pub all: bool,

    /// Show only archived requirements.
    // trace:STORY-441 | ai:claude
    #[clap(long, conflicts_with_all = ["all", "deferred"])]
    pub archived: bool,

    /// Show only deferred requirements (the primed/conditional shelf).
    /// Honors both the deferred flag and legacy `deferred:*` tags.
    // trace:STORY-584 | ai:claude
    #[clap(long, conflicts_with_all = ["all", "archived"])]
    pub deferred: bool,

    /// Emit one bare canonical spec ID per line — no header, no count
    /// footer, no color. Directly usable in `$(...)` / xargs:
    /// `aida edit $(aida search "auth" --short) ...`. Composes with
    /// every filter and honors the same archive/defer default as plain
    /// `search`. Mutually exclusive with --json. Mirrors `aida list
    /// --short`.
    // trace:BUG-531 | ai:claude
    #[clap(
        long,
        visible_alias = "ids-only",
        visible_alias = "quiet",
        short = 'q',
        conflicts_with = "json"
    )]
    pub short: bool,

    /// Emit results as a JSON array. Mutually exclusive with --short.
    /// Mirrors `aida list --json`.
    // trace:BUG-531 | ai:claude
    #[clap(long, conflicts_with = "short")]
    pub json: bool,

    /// Select AND order the displayed columns, e.g.
    /// `--fields id,title,status,priority,tags`. Valid fields: id, title,
    /// status, type, priority, feature, owner, assignee, tags. Renders
    /// exactly those columns in that order; default keeps the standard
    /// layout. Mirrors `aida list --fields`.
    // trace:STORY-734 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(long, value_name = "CSV", conflicts_with_all = ["short", "json"])]
    pub fields: Option<String>,
}

fn run_search(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <SearchArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                SearchArgs {
                    query,
                    status,
                    limit,
                    sync,
                    all,
                    archived,
                    deferred,
                    include_meta,
                    short,
                    json,
                    fields,
                    ..
                } => {
                    // STORY-764: the global `--format json` pin reaches search the same
                    // way its own `--json` does. trace:STORY-764 | ai:claude
                    let effective_json = *json || output_format_is_json();
                    // STORY-78: opt-in sync-pull before search. trace:STORY-78 | ai:claude
                    if *sync {
                        maybe_sync_pull(store_path)?;
                    }
                    // STORY-441: same three-way archive axis as `aida list`.
                    // trace:STORY-441 | ai:claude
                    let archive = if *all {
                        aida_core::ArchiveFilter::Both
                    } else if *archived {
                        aida_core::ArchiveFilter::ArchivedOnly
                    } else {
                        aida_core::ArchiveFilter::NonArchivedOnly
                    };
                    // STORY-584: same three-way defer axis as `aida list`. `--archived`
                    // keeps the defer axis open so the archive audit is complete.
                    // trace:STORY-584 | ai:claude
                    let defer = if *all || *archived {
                        aida_core::DeferFilter::Both
                    } else if *deferred {
                        aida_core::DeferFilter::DeferredOnly
                    } else {
                        aida_core::DeferFilter::NonDeferredOnly
                    };
                    // Cache-backed FTS5 search (EPIC-1-001 Phase 2). Replaces a
                    // full-store load + in-memory substring scan.
                    // trace:EPIC-1-001 | ai:claude
                    let mut results = backend.search(query, *limit, archive, defer)?;
                    if let Some(s) = status {
                        let needle = s.clone();
                        results.retain(|r| r.status.eq_ignore_ascii_case(&needle));
                    }
                    // BUG-488: hide the seeded META AI-prompt specs by default, same as
                    // `aida list` (BUG-27) — a newcomer's search shouldn't surface
                    // internal prompt machinery like "Generate Children". Opt in with
                    // --include-meta. trace:BUG-488 | ai:claude
                    if !*include_meta {
                        results.retain(|r| !r.req_type.eq_ignore_ascii_case("meta"));
                    }

                    // BUG-531: mirror `aida list`'s output-mode flags onto search.
                    // `--short`/`-q`/`--ids-only`/`--quiet` emits one bare canonical
                    // spec ID per line — no header, no count footer, no color — so the
                    // output is directly pipeable into `$(...)` / xargs. Runs AFTER the
                    // shared filter + meta passes (same row set the human table shows)
                    // and returns early, before the table or JSON rendering. Mutually
                    // exclusive with --json (enforced by clap). Empty result = no
                    // output (and a zero exit), matching `aida list --short`.
                    // trace:BUG-531 | ai:claude
                    if *short {
                        for req in &results {
                            let id = req.agreed_id.as_deref().or(req.spec_id.as_deref());
                            if let Some(id) = id {
                                println!("{id}");
                            }
                        }
                        return crate::command_registry::returned(Ok(()));
                    }

                    // BUG-531: `--json` emits the row set as a JSON array without the
                    // human chrome (header / count footer / colour), mirroring
                    // `aida list --json`. trace:BUG-531 | ai:claude
                    if effective_json {
                        #[derive(serde::Serialize)]
                        struct SearchJsonRow<'a> {
                            spec_id: &'a str,
                            title: &'a str,
                            req_type: &'a str,
                            status: &'a str,
                            tags: &'a [String],
                        }
                        let out: Vec<SearchJsonRow> = results
                            .iter()
                            .map(|req| SearchJsonRow {
                                spec_id: req
                                    .agreed_id
                                    .as_deref()
                                    .or(req.spec_id.as_deref())
                                    .unwrap_or(""),
                                title: req.title.as_str(),
                                req_type: req.req_type.as_str(),
                                status: req.status.as_str(),
                                tags: &req.tags,
                            })
                            .collect();
                        println!("{}", crate::cache_output::json_pretty(&out)?);
                        return crate::command_registry::returned(Ok(()));
                    }

                    // STORY-734: `--fields <csv>` selects + orders the columns of the
                    // search table too, mirroring `aida list --fields`. Search rows carry
                    // no work-routing axis, so the queued/in_flight/blocked fields render
                    // false here. In AGENT mode the row set is emitted as the lean TOON
                    // `specs[N]{...}` projection (the ~2x token win the agent surface
                    // exists for) exactly like `aida list --fields`, not the verbose
                    // human box-table; `render_search_fields` branches on the same
                    // `agent_output_mode()` the list handler uses so the two surfaces
                    // stay identical. trace:STORY-734 trace:BUG-668 | ai:claude
                    if let Some(csv) = fields.as_deref() {
                        let selected = toon_list_fields(Some(csv))?;
                        if results.is_empty() {
                            println!("No results for: {}", query);
                        } else {
                            println!(
                                "{}",
                                render_search_fields(&results, &selected, agent_output_mode())
                            );
                        }
                        return crate::command_registry::returned(Ok(()));
                    }

                    // BUG-672: the DEFAULT search path (no `--fields`) was a hardcoded
                    // human box-table even in agent mode — the most-called discovery
                    // verb leaked human chrome (`─` rule, padded columns, count footer)
                    // to agents instead of the lean TOON projection STORY-734 gave the
                    // `--fields` path. Route the default through the SAME minimal
                    // `specs[N]{id,title,status,type}` schema `aida list` emits (reuse
                    // `toon_list_fields(None)` + `render_search_fields`, do NOT reinvent),
                    // so the no-`--fields` case finally reaches STORY-734 parity. The
                    // human TTY path keeps the box-table unchanged. trace:BUG-672
                    if agent_output_mode() {
                        let selected = toon_list_fields(None)?;
                        println!("{}", render_search_fields(&results, &selected, true));
                        // BUG-672 (Finding #4): trailing drill-in next-step block —
                        // `aida show <id>` is the move an agent reaching for a search
                        // result is missing. Placeholder id (multi-row surface), same
                        // rule `list` follows. trace:BUG-672
                        let next = crate::help_next::search_next(!results.is_empty());
                        if let Some(block) = crate::help_next::render(&next) {
                            println!("{block}");
                        }
                        return crate::command_registry::returned(Ok(()));
                    }

                    if results.is_empty() {
                        println!("No results for: {}", query);
                    } else {
                        println!("{:<14} {:<12} {:<10} Title", "ID", "Type", "Status");
                        println!("{}", "─".repeat(65));
                        for req in &results {
                            let display_id = req
                                .agreed_id
                                .as_deref()
                                .or(req.spec_id.as_deref())
                                .unwrap_or("?");
                            // BUG-781: same type-aware label the list table uses, so a
                            // ratified ADR reads `Accepted` here too. trace:BUG-781
                            println!(
                                "{:<14} {:<12} {:<10} {}",
                                display_id,
                                req.req_type,
                                status_display::display_status_for_type(&req.req_type, &req.status),
                                req.title,
                            );
                        }
                        println!("\n{} results", results.len());
                    }
                    // BUG-672 (Finding #4): the HUMAN search surface gets the same
                    // drill-in nudge via the `Next:` block (the idiom `aida show` uses),
                    // so the operator sees the next move too. trace:BUG-672
                    let next = crate::help_next::search_next(!results.is_empty());
                    if let Some(block) = crate::help_next::render_human(&next) {
                        println!("{block}");
                    }
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                SearchArgs {
                    query,
                    case_sensitive,
                    status,
                    feature,
                    ..
                } => {
                    // Search is a simplified version of grep with sensible defaults:
                    // - Case insensitive by default (unless -s/--case-sensitive)
                    // - Searches all text fields (title, description, comments)
                    // The --limit flag is honored by the git backend's FTS5 path; the
                    // legacy grep walks the in-memory store and ignores it.
                    grep_requirements(
                        &storage,
                        query,
                        !case_sensitive, // invert: case_sensitive=false means ignore_case=true
                        false,           // no extended regex
                        0,               // no after context
                        0,               // no before context
                        None,            // no context
                        None,            // search all fields
                        status.as_deref(),
                        None, // no type filter
                        feature.as_deref(),
                        false, // show full matches, not just IDs
                        false, // don't show count
                        false, // don't invert match
                    )?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static SEARCH: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "search",
        help_after: "rework",
        command: <SearchArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Read,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_search),
    };

#[cfg(test)]
#[path = "tests/task_1981_add_tests.rs"]
mod task_1981_add_tests;
