//! Init scaffolding: gitignore entries, user AIDA instructions block, OBE scaffold prune (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// `queue_promoted_finding`, returning the entry it wrote so a failed
/// promote can tell whether the queue still holds it (BUG-1651).
// trace:BUG-1651 | ai:claude
pub(crate) fn queue_promoted_finding_entry(
    store_path: &std::path::Path,
    requirement_id: Uuid,
    display_id: &str,
    for_override: Option<&str>,
) -> Result<aida_core::QueueEntry> {
    let role = for_override.unwrap_or("implementer");
    queue_spec_entry_for_role(
        store_path,
        requirement_id,
        role,
        format!("Promoted from finding {display_id} via `aida findings promote`"),
    )
    .with_context(|| {
        format!(
            "failed to add {display_id} to the {role} queue — \
             finding left at draft, not promoted"
        )
    })
}

/// Append a spec to a role's work queue for the current user; returns the
/// role. Shared by the work and gate promote routes, each of which adds its
/// own failure context.
// trace:STORY-1428 | ai:claude
pub(crate) fn queue_spec_for_role(
    store_path: &std::path::Path,
    requirement_id: Uuid,
    role: &str,
    note: String,
) -> Result<String> {
    queue_spec_entry_for_role(store_path, requirement_id, role, note)?;
    Ok(role.to_string())
}

/// [`queue_spec_for_role`], returning the entry as written (its `position`
/// still the append sentinel the backend resolves). BUG-1651: the promote
/// rollback matches the queue against it.
// trace:STORY-1428 trace:BUG-1651 | ai:claude
pub(crate) fn queue_spec_entry_for_role(
    store_path: &std::path::Path,
    requirement_id: Uuid,
    role: &str,
    note: String,
) -> Result<aida_core::QueueEntry> {
    let role = role.to_string();
    let user_id = current_user_id(None);
    let storage = Storage::new(store_path);
    let entry = aida_core::QueueEntry {
        user_id: user_id.clone(),
        requirement_id,
        // i64::MAX is the "append to bottom" sentinel the git backend
        // resolves to max_position + 1000 (STORY-72).
        position: i64::MAX,
        added_by: user_id,
        note: Some(note),
        added_at: chrono::Utc::now(),
        for_role: Some(role),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    storage.queue_add(entry.clone())?;
    Ok(entry)
}

// trace:TASK-0001 | ai:claude:high
/// Count requirements in a git-canonical store at `store_path`. Returns
/// None if the store doesn't exist or can't be opened — caller treats that
/// as "no data to lose, proceed".
/// trace:EPIC-1-001 | ai:claude
pub(crate) fn count_requirements_in_store(store_path: &std::path::Path) -> Option<usize> {
    if !store_path.is_dir() {
        return None;
    }
    let backend = aida_core::GitBackend::new(store_path).ok()?;
    let store = aida_core::DatabaseBackend::load(&backend).ok()?;
    Some(store.requirements.len())
}

/// Same as count_requirements_in_store but for a legacy SQLite-canonical
/// store at `db_path`.
pub(crate) fn count_requirements_in_sqlite(db_path: &std::path::Path) -> Result<usize> {
    let storage = Storage::new(db_path);
    Ok(storage.load()?.requirements.len())
}

/// Surface the data-loss risk of `aida init --force` on a populated store.
/// Returns true if the user confirmed (typed "reset"), false otherwise.
/// Bails the parent caller via Ok if the user cancels — caller pattern is
/// `if !confirm_destructive_reset(...)? { return Ok(()); }`.
/// trace:EPIC-1-001 | ai:claude
pub(crate) fn confirm_destructive_reset(
    count: usize,
    store_path: &std::path::Path,
) -> Result<bool> {
    eprintln!();
    eprintln!(
        "{} `aida init --force` will RESET the requirements store at {}.",
        "DANGER:".red().bold(),
        store_path.display()
    );
    eprintln!(
        "        {} existing requirement(s) will be lost.",
        count.to_string().red().bold()
    );
    eprintln!();
    eprintln!("If you only wanted to refresh scaffolding (CLAUDE.md, .claude/skills/, hooks),");
    eprintln!(
        "cancel here and run instead:  {}",
        "aida scaffold apply --force".cyan()
    );
    eprintln!();
    eprintln!(
        "Type `{}` (literally) to confirm the destructive reset, or anything else to cancel:",
        "reset".bold()
    );
    let mut answer = String::new();
    if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
        eprintln!("Cancelled.");
        return Ok(false);
    }
    if answer.trim() == "reset" {
        eprintln!("{} proceeding with reset.", "Confirmed:".yellow());
        Ok(true)
    } else {
        eprintln!("Cancelled. Store untouched.");
        Ok(false)
    }
}

/// STORY-757: the markdown-only first run. Scaffolds ONLY a `specs/` folder with
/// one example spec + a sample source file carrying a trace comment, then points
/// at `aida why`. No orphan branch, cache, MCP, skills, roles, or queue — the
/// 60-second magic with zero machine, so a first-user gets the whole idea in one
/// command and only reaches for `aida init` when a folder of markdown isn't
/// enough.
// trace:STORY-757 | ai:claude
/// Write the minimal scaffold under `root`. Returns the created paths, or an
/// empty vec when a target already exists and `force` is false (caller reports
/// it). Split out from [`handle_init_minimal`] so it's testable without a
/// process-global `set_current_dir` (the BUG-697 race).
// trace:STORY-757 | ai:claude
pub(crate) fn scaffold_minimal_specs(
    root: &std::path::Path,
    force: bool,
) -> Result<Vec<std::path::PathBuf>> {
    let specs = root.join("specs");
    let spec_file = specs.join("EXAMPLE-1.md");
    let demo = root.join("example.py");
    if (spec_file.exists() || demo.exists()) && !force {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(&specs).with_context(|| "creating specs/")?;
    std::fs::write(
        &spec_file,
        "---\nid: EXAMPLE-1\ntitle: Rate-limit the login endpoint\nstatus: draft\n---\n\
         We were seeing credential-stuffing attacks. Throttle login attempts to 5/min per IP.\n",
    )
    .with_context(|| "writing specs/EXAMPLE-1.md")?;
    std::fs::write(
        &demo,
        "def login(req):\n    # trace:EXAMPLE-1\n    if too_many_attempts(req.ip):\n        return deny()\n",
    )
    .with_context(|| "writing example.py")?;
    Ok(vec![spec_file, demo])
}

pub(crate) fn handle_init_minimal(force: bool) -> Result<()> {
    let root = std::env::current_dir()?;
    let created = scaffold_minimal_specs(&root, force)?;
    if created.is_empty() {
        println!(
            "  {} specs/EXAMPLE-1.md or example.py already exists — pass --force to overwrite.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
        return Ok(());
    }

    let check = crate::glyph(crate::glyphs::Glyph::Check).green();
    println!("{check} minimal AIDA — a folder of markdown, zero machine.");
    println!();
    println!("  created  specs/EXAMPLE-1.md   a spec: a title + one line of why");
    println!("  created  example.py           code with a `# trace:EXAMPLE-1` comment");
    println!();
    println!("  Now ask your code why it exists:");
    println!("    {}", "aida why example.py:2".cyan().bold());
    println!();
    println!(
        "  {}",
        "That's the whole idea — add `# trace:<ID>` comments to your own code and ask.".dimmed()
    );
    println!(
        "  {}",
        "When a folder of markdown isn't enough, `aida init` adds the graph, IDs, and MCP."
            .dimmed()
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/init_minimal_tests.rs"]
mod init_minimal_tests;

#[derive(Default)]
pub(crate) struct DisciplinePackScaffoldReport {
    pub(crate) written: usize,
    pub(crate) relocated: usize,
    pub(crate) written_paths: Vec<std::path::PathBuf>,
    pub(crate) relocated_paths: Vec<std::path::PathBuf>,
}

/// Scaffold the discipline pack — every embedded `.aida/discipline/*`
/// template — into `<root>/.aida/discipline/`. Idempotent: an existing
/// canonical file is left alone even under refresh.
///
/// Migration: a previous `<root>/docs/aida/discipline/` pack is moved into the
/// canonical directory first. Existing destination files are never overwritten,
/// so user edits survive and an old path can remain as the one-release
/// fallback when there is a destination conflict.
/// trace:STORY-255 | STORY-443 | ai:claude
// trace:STORY-829 | ai:codex
pub(crate) fn ensure_discipline_pack_scaffold(
    root: &std::path::Path,
    _force: bool,
) -> Result<DisciplinePackScaffoldReport> {
    use aida_core::templates::EMBEDDED_TEMPLATES;
    let mut pack: Vec<(&str, &str)> = EMBEDDED_TEMPLATES
        .iter()
        .filter_map(|(k, v)| k.strip_prefix(".aida/discipline/").map(|n| (n, *v)))
        .collect();
    if pack.is_empty() {
        return Ok(DisciplinePackScaffoldReport::default());
    }
    pack.sort_by(|a, b| a.0.cmp(b.0));

    let dir = root.join(".aida").join("discipline");
    std::fs::create_dir_all(&dir)?;
    let mut report = DisciplinePackScaffoldReport::default();
    let old_dir = root.join("docs").join("aida").join("discipline");
    if old_dir.is_dir() {
        let mut old_entries = Vec::new();
        for entry in std::fs::read_dir(&old_dir)? {
            old_entries.push(entry?.path());
        }
        old_entries.sort();
        for old_path in old_entries {
            if !old_path.is_file() {
                continue;
            }
            let Some(name) = old_path.file_name() else {
                continue;
            };
            let dest = dir.join(name);
            if dest.exists() {
                continue;
            }
            std::fs::rename(&old_path, &dest)?;
            report.relocated += 1;
            report
                .relocated_paths
                .push(dest.strip_prefix(root).unwrap_or(&dest).to_path_buf());
        }
        remove_empty_parent_dirs(root, &old_dir)?;
    }

    for (name, content) in pack {
        let dest = dir.join(name);
        if dest.exists() {
            continue;
        }
        std::fs::write(&dest, format!("{content}\n"))?;
        report.written += 1;
        report
            .written_paths
            .push(dest.strip_prefix(root).unwrap_or(&dest).to_path_buf());
    }
    Ok(report)
}

pub(crate) fn remove_empty_parent_dirs(
    root: &std::path::Path,
    start: &std::path::Path,
) -> Result<()> {
    let mut dir = start.to_path_buf();
    while dir.starts_with(root) && dir != root {
        match std::fs::remove_dir(&dir) {
            Ok(()) => {
                dir.pop();
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                dir.pop();
            }
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Scaffold a starter `docs/competitive-analysis/ecosystem-watch.md` with
/// today's local date stamped into the `Last updated` line. Idempotent:
/// leaves an existing file alone unless `force` is set. Returns `true` if
/// the file was written.
///
/// `scripts/release.sh` reads the `Last updated` line to verify the
/// ecosystem review is recent enough for a major/minor release; without
/// this scaffold, a fresh project's first `release.sh minor` would hit the
/// missing-file warning path (TASK-126 origin).
// trace:TASK-126 | ai:claude
/// Scaffold `.aida/project.toml` — the checked-in statement of what this
/// project IS — pre-filled from what is already knowable.
///
/// Idempotent in the same way the discipline pack and ecosystem-watch
/// scaffolds are: an existing file is left ALONE unless `force`. That is what
/// satisfies "`aida init --refresh` must not overwrite human edits" — the
/// manifest has no canonical master to overlay (unlike skills or memories,
/// whose refresh contract compares a checksum against an embedded template),
/// so the honest behaviour is simply never to rewrite it. Returns whether a
/// file was written.
///
/// PRE-FILLED, NEVER A BLANK FORM. Name, description and repository come from
/// the directory, the README and `origin`, so the file is worth something the
/// moment it exists even if nobody ever edits it. An empty form is exactly how
/// a metadata standard goes stale in a week.
// trace:STORY-781 | ai:claude
pub(crate) fn ensure_project_manifest_scaffold(
    root: &std::path::Path,
    force: bool,
) -> Result<bool> {
    use aida_core::project_manifest as pm;
    let dest = root.join(pm::MANIFEST_REL_PATH);
    if dest.exists() && !force {
        return Ok(false);
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let facts = pm::derive_facts(root);
    std::fs::write(&dest, pm::render(&facts))?;
    Ok(true)
}

pub(crate) const PROJECTS_REGISTRY_HEADER: &str = "\
# AIDA project registry
# Machine-global index of local projects. Hand-maintained today; EPIC-22 makes this canonical.

";

pub(crate) fn register_project_in_global_registry(root: &std::path::Path) -> Result<bool> {
    let home =
        aida_home_dir().context("cannot resolve home directory for ~/.aida/projects.toml")?;
    register_project_in_registry_at(&home.join(".aida/projects.toml"), root)
}

pub(crate) fn register_project_in_registry_at(
    registry_path: &std::path::Path,
    root: &std::path::Path,
) -> Result<bool> {
    use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

    let existing = match std::fs::read_to_string(registry_path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", registry_path.display())),
    };
    let before = existing.as_deref().unwrap_or("");
    let mut doc: DocumentMut = before
        .parse()
        .with_context(|| format!("parsing {}", registry_path.display()))?;

    if doc.get("project").is_none() {
        doc["project"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let projects = doc["project"]
        .as_array_of_tables_mut()
        .context("~/.aida/projects.toml key `project` must be an array of tables")?;

    let project_path = absolute_project_path(root)?;
    let project_path_s = project_path.to_string_lossy().to_string();
    let name = project_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "project".to_string());
    let repo = git_origin_url(root);

    let mut found = false;
    for table in projects.iter_mut() {
        if table
            .get("path")
            .and_then(|i| i.as_str())
            .map(|p| p == project_path_s)
            .unwrap_or(false)
        {
            write_project_registry_table(table, &name, &project_path_s, repo.as_deref());
            found = true;
            break;
        }
    }
    if !found {
        let mut table = Table::new();
        write_project_registry_table(&mut table, &name, &project_path_s, repo.as_deref());
        projects.push(table);
    }

    let mut after = doc.to_string();
    if existing.is_none() {
        after = format!("{PROJECTS_REGISTRY_HEADER}{after}");
    }
    if after == before {
        return Ok(false);
    }
    if let Some(parent) = registry_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(registry_path, after)?;
    Ok(true)
}

pub(crate) fn write_project_registry_table(
    table: &mut toml_edit::Table,
    name: &str,
    path: &str,
    repo: Option<&str>,
) {
    table["name"] = toml_edit::value(name);
    table["path"] = toml_edit::value(path);
    if let Some(repo) = repo.filter(|r| !r.trim().is_empty()) {
        table["repo"] = toml_edit::value(repo);
    } else {
        table.remove("repo");
    }
}

pub(crate) fn absolute_project_path(root: &std::path::Path) -> Result<std::path::PathBuf> {
    let abs = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()?.join(root)
    };
    Ok(abs.canonicalize().unwrap_or(abs))
}

pub(crate) fn git_origin_url(root: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
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

pub(crate) fn ensure_ecosystem_watch_scaffold(root: &std::path::Path, force: bool) -> Result<bool> {
    let dir = root.join("docs").join("competitive-analysis");
    let dest = dir.join("ecosystem-watch.md");
    if dest.exists() && !force {
        return Ok(false);
    }
    let Some(template) = aida_core::templates::EMBEDDED_TEMPLATES
        .get("docs/competitive-analysis/ecosystem-watch.md")
    else {
        return Ok(false);
    };
    std::fs::create_dir_all(&dir)?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let content = template.replace("{{LAST_UPDATED}}", &today);
    std::fs::write(&dest, content)?;
    Ok(true)
}

/// Detect whether the deny-by-default `.aida/*` line is present (ignoring
/// comments and surrounding whitespace). trace:BUG-73 | ai:claude
pub(crate) fn has_aida_runtime_deny_pattern(content: &str) -> bool {
    content.lines().any(|line| line.trim() == ".aida/*")
}

pub(crate) fn gitignore_has_discipline_pack_allow_list(content: &str) -> bool {
    content.lines().any(|line| {
        line.trim()
            .trim_start_matches('!')
            .trim_end_matches('/')
            .ends_with(".aida/discipline/**")
    })
}

pub(crate) fn ensure_discipline_pack_gitignore_allow_list(cwd: &std::path::Path) -> Result<bool> {
    use std::io::Write;

    let gitignore_path = cwd.join(".gitignore");
    let Ok(content) = std::fs::read_to_string(&gitignore_path) else {
        return Ok(false);
    };
    if gitignore_has_discipline_pack_allow_list(&content) {
        return Ok(false);
    }
    let discipline_pack_entry =
        "\n# AIDA-using discipline guides are checked in with the project.\n\
         # trace:STORY-829 | ai:codex\n\
         !.aida/discipline/\n\
         !.aida/discipline/**\n";
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&gitignore_path)?;
    file.write_all(discipline_pack_entry.as_bytes())?;
    Ok(true)
}

pub(crate) const USER_AIDA_INSTRUCTIONS_BEGIN: &str = "<!-- AIDA-USER-INSTRUCTIONS-BEGIN";

pub(crate) const USER_AIDA_INSTRUCTIONS_END: &str = "<!-- AIDA-USER-INSTRUCTIONS-END -->";

// trace:STORY-831 | ai:codex
pub(crate) fn user_aida_awareness_snippet() -> &'static str {
    "## AIDA Awareness\n\n\
     When working in a project, check whether AIDA is initialized: look for \
     `.aida/config.toml`, a `.aida-store/` worktree, or an `aida-store` branch.\n\n\
     If AIDA is present, requirements live in AIDA. Use `aida list`, `aida search`, \
     `aida show`, `aida add`, `aida edit`, and `aida comment` for requirement work; \
     add `trace:<SPEC-ID>` comments in relevant code; and include the `(SPEC-ID)` \
     trailer in commits.\n\n\
     If AIDA is not present, ignore this section.\n"
}

// trace:STORY-831 | ai:codex
pub(crate) fn user_aida_checksum(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())[..8].to_string()
}

// trace:STORY-831 | ai:codex
pub(crate) fn render_user_aida_instructions_block() -> String {
    let body = user_aida_awareness_snippet();
    format!(
        "{USER_AIDA_INSTRUCTIONS_BEGIN} checksum:{} -->\n{body}{USER_AIDA_INSTRUCTIONS_END}\n",
        user_aida_checksum(body),
    )
}

// trace:STORY-831 | ai:codex
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct UserAidaInstructionsReport {
    pub(crate) written: usize,
    pub(crate) unchanged: usize,
    pub(crate) kept_edited: usize,
}

// trace:STORY-831 | ai:codex
pub(crate) fn user_aida_block_bounds(content: &str) -> Option<(usize, usize, usize)> {
    let begin = content.find(USER_AIDA_INSTRUCTIONS_BEGIN)?;
    let after_begin_line = content[begin..].find('\n').map(|n| begin + n + 1)?;
    let end_rel = content[after_begin_line..].find(USER_AIDA_INSTRUCTIONS_END)?;
    let end_start = after_begin_line + end_rel;
    let mut end = end_start + USER_AIDA_INSTRUCTIONS_END.len();
    if content[end..].starts_with('\n') {
        end += 1;
    }
    Some((begin, after_begin_line, end))
}

// trace:STORY-831 | ai:codex
pub(crate) fn user_aida_block_checksum(header: &str) -> Option<&str> {
    let marker = "checksum:";
    let after = header.find(marker)? + marker.len();
    let tail = &header[after..];
    let end = tail
        .find(|c: char| c.is_whitespace() || c == '-')
        .unwrap_or(tail.len());
    Some(&tail[..end])
}

// trace:STORY-831 | ai:codex
pub(crate) fn merge_user_aida_instructions(
    existing: Option<&str>,
    refresh: bool,
) -> (String, UserAidaInstructionsReport) {
    let expected = render_user_aida_instructions_block();
    let Some(existing) = existing else {
        return (
            expected,
            UserAidaInstructionsReport {
                written: 1,
                ..Default::default()
            },
        );
    };

    let Some((begin, body_start, end)) = user_aida_block_bounds(existing) else {
        let separator = if existing.is_empty() || existing.ends_with("\n\n") {
            ""
        } else if existing.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        return (
            format!("{existing}{separator}{expected}"),
            UserAidaInstructionsReport {
                written: 1,
                ..Default::default()
            },
        );
    };

    let old_region = &existing[begin..end];
    if aida_core::scaffolding::generated_text_matches(old_region, &expected) {
        return (
            existing.to_string(),
            UserAidaInstructionsReport {
                unchanged: 1,
                ..Default::default()
            },
        );
    }

    let header = &existing[begin..body_start];
    let body_end =
        end - USER_AIDA_INSTRUCTIONS_END.len() - usize::from(existing[..end].ends_with('\n'));
    let body = &existing[body_start..body_end];
    let normalized_body = aida_core::scaffolding::normalize_lf(body);
    let pristine = user_aida_block_checksum(header)
        .map(|checksum| checksum == user_aida_checksum(&normalized_body))
        .unwrap_or(false);
    if refresh && pristine {
        let mut merged = String::new();
        merged.push_str(&existing[..begin]);
        merged.push_str(&expected);
        merged.push_str(&existing[end..]);
        (
            merged,
            UserAidaInstructionsReport {
                written: 1,
                ..Default::default()
            },
        )
    } else {
        (
            existing.to_string(),
            UserAidaInstructionsReport {
                kept_edited: 1,
                ..Default::default()
            },
        )
    }
}

// trace:STORY-831 | ai:codex
pub(crate) fn install_user_aida_instructions_at(
    home: &std::path::Path,
    refresh: bool,
) -> Result<UserAidaInstructionsReport> {
    let targets = [
        home.join(".claude").join("CLAUDE.md"),
        home.join(".codex").join("AGENTS.md"),
    ];
    let mut total = UserAidaInstructionsReport::default();
    for target in targets {
        let existing = std::fs::read_to_string(&target).ok();
        let (merged, report) = merge_user_aida_instructions(existing.as_deref(), refresh);
        total.written += report.written;
        total.unchanged += report.unchanged;
        total.kept_edited += report.kept_edited;
        if existing
            .as_deref()
            .is_some_and(|actual| aida_core::scaffolding::generated_text_matches(actual, &merged))
        {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&target, merged).with_context(|| format!("writing {}", target.display()))?;
    }
    Ok(total)
}

// trace:STORY-831 | ai:codex
pub(crate) fn scaffold_user_aida_instructions(refresh: bool) -> Result<()> {
    let Some(home) = aida_home_dir() else {
        anyhow::bail!("could not resolve home directory");
    };
    let report = install_user_aida_instructions_at(&home, refresh)?;
    println!(
        "{} user AIDA instructions: {} written, {} unchanged, {} kept edited",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        report.written,
        report.unchanged,
        report.kept_edited,
    );
    println!(
        "  {}",
        "~/.claude/CLAUDE.md and ~/.codex/AGENTS.md".dimmed()
    );
    Ok(())
}

// trace:STORY-831 | ai:codex
pub(crate) fn user_aida_instructions_already_installed(home: &std::path::Path) -> bool {
    [
        home.join(".claude/CLAUDE.md"),
        home.join(".codex/AGENTS.md"),
    ]
    .into_iter()
    .all(|path| {
        std::fs::read_to_string(path)
            .map(|content| user_aida_block_bounds(&content).is_some())
            .unwrap_or(false)
    })
}

// trace:STORY-831 | ai:codex
pub(crate) fn install_user_aida_instructions_if_accepted(
    home: &std::path::Path,
    accepted: bool,
) -> Result<Option<UserAidaInstructionsReport>> {
    if accepted {
        Ok(Some(install_user_aida_instructions_at(home, false)?))
    } else {
        Ok(None)
    }
}

// trace:STORY-831 | ai:codex
pub(crate) fn maybe_offer_user_aida_instructions() -> Result<()> {
    let Some(home) = aida_home_dir() else {
        return Ok(());
    };
    if user_aida_instructions_already_installed(&home) {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Ok(());
    }

    eprintln!();
    eprintln!("{}", "Machine setup — AIDA awareness for agents".bold());
    eprintln!("  Install a small detection-gated AIDA section in:");
    eprintln!("    ~/.claude/CLAUDE.md");
    eprintln!("    ~/.codex/AGENTS.md");
    eprintln!("  It only applies inside projects where AIDA is initialized.");
    let accepted = prompt_yes_no("  Install user-level AIDA instructions now? [Y/n]: ", true)?;
    let Some(report) = install_user_aida_instructions_if_accepted(&home, accepted)? else {
        eprintln!(
            "  {} user instruction files left untouched; run {} anytime.",
            "Note:".dimmed(),
            "aida scaffold user-instructions".cyan()
        );
        return Ok(());
    };
    eprintln!(
        "  {} user AIDA instructions installed ({} written, {} unchanged, {} kept edited).",
        "+".green(),
        report.written,
        report.unchanged,
        report.kept_edited,
    );
    Ok(())
}

/// TASK-859: after the core scaffold, prompt for a small curated set of
/// high-value config knobs that would otherwise sit at silent defaults, then
/// offer to open `aida config menu` for the full surface.
///
/// Deliberately tasteful — one knob (telemetry opt-out) plus the config-menu
/// offer, not an interrogation. The point is *discoverability* of config, not
/// friction. Mirrors `maybe_prompt_agent_posture`: TTY-gated (non-interactive
/// init writes nothing and keeps every default) and idempotent (a knob already
/// present in `.aida/config.toml` is detected and skipped, so re-running
/// `aida init --force` never re-prompts settled choices). Skips re-prompting
/// the things init already prompted for (agent posture, node name).
/// trace:TASK-859 | ai:claude
pub(crate) fn maybe_prompt_init_config(project_root: &std::path::Path) -> Result<()> {
    // TTY-gated: non-interactive init (--yes, CI, the test suite) writes
    // nothing and keeps the current defaults. This is the critical regression
    // guard — every default must survive a headless init untouched.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Ok(());
    }

    let config_path = project_root.join(".aida").join("config.toml");

    // Telemetry opt-out (STORY-122). Idempotent: if the project config already
    // carries a `[telemetry]` decision, leave it alone — never re-prompt a
    // settled choice on a re-run.
    let telemetry_already_set = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|c| crate::usage::parse_telemetry_enabled(&c))
        .is_some();
    if !telemetry_already_set {
        eprintln!();
        eprintln!("{}", "Project setup — usage telemetry".bold());
        eprintln!("  AIDA logs a one-line-per-command local usage trail (command shape,");
        eprintln!("  exit code, duration — never arguments, paths, or content). It is");
        eprintln!("  local-only and never phoned home. Keep it on?");
        let keep = prompt_yes_no("  Keep usage telemetry on? [Y/n]: ", true)?;
        if !keep {
            append_telemetry_disabled(&config_path)?;
            eprintln!(
                "  {} telemetry disabled ({} in .aida/config.toml).",
                "+".green(),
                "[telemetry] enabled = false".cyan()
            );
        } else {
            eprintln!(
                "  {} telemetry left on (default; opt out anytime with {}).",
                "Note:".dimmed(),
                "AIDA_TELEMETRY=0".cyan()
            );
        }
    }

    // Offer the full config surface via the new TUI (STORY-661). Declining (or
    // a build without the TUI feature) just prints the pointer and moves on.
    eprintln!();
    let configure_more = prompt_yes_no(
        "  Configure more settings now (opens the config menu)? [y/N]: ",
        false,
    )?;
    if configure_more {
        // `handle_config_menu_command` is itself TTY-gated and reads the cwd,
        // so it degrades gracefully if the terminal is gone by now.
        config_cmd::handle_config_menu_command()?;
    } else {
        eprintln!(
            "  {} run {} anytime to review or change settings.",
            "Note:".dimmed(),
            "aida config menu".cyan()
        );
    }
    Ok(())
}

/// TASK-859: write `[telemetry] enabled = false` to the project's
/// `.aida/config.toml`, opting out of the local usage trail. Use the shared
/// section-aware editor so existing sections are updated in place and duplicate
/// `[telemetry]` blocks are never raw-appended.
// trace:TASK-859 trace:BUG-1025 | ai:codex
pub(crate) fn append_telemetry_disabled(config_path: &std::path::Path) -> Result<()> {
    crate::config_edit::set_kv(
        config_path,
        "telemetry",
        "enabled",
        toml_edit::Value::from(false),
    )
}

/// Shared BUG-298 / BUG-719 handling: surface (and with `prune`, remove)
/// obsolete `aida-*` skills/commands/hooks this AIDA version no longer ships —
/// e.g. a skill deleted upstream but left behind, or *resurrected*, by an older
/// binary whose embedded template set still has it (BUG-719). Symlinks and
/// non-`aida-` files are never touched. Called by BOTH `scaffold apply` and
/// `scaffold upgrade` so whichever drift-fixing command the user runs cleans
/// the stray. `remove_hint` is the command to suggest when not pruning.
// trace:BUG-719 | ai:claude
pub(crate) fn report_and_prune_obe_scaffold(
    root: &std::path::Path,
    prune: bool,
    dry_run: bool,
    remove_hint: &str,
) {
    let obe = detect_obe_aida_scaffold_files(root);
    if obe.is_empty() {
        return;
    }
    println!();
    if prune && !dry_run {
        println!(
            "{} Pruning {} obsolete aida-* file(s):",
            "🧹".yellow(),
            obe.len()
        );
        for p in &obe {
            let rel = p.strip_prefix(root).unwrap_or(p);
            let removed = if p.is_dir() {
                std::fs::remove_dir_all(p)
            } else {
                std::fs::remove_file(p)
            };
            match removed {
                Ok(()) => println!("  {} {}", "-".red(), rel.display()),
                Err(e) => {
                    eprintln!(
                        "  {} {} (failed: {})",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        rel.display(),
                        e
                    )
                }
            }
        }
    } else {
        println!(
            "{} {} obsolete aida-* file(s) this AIDA version no longer ships:",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            obe.len()
        );
        for p in &obe {
            println!(
                "  {} {}",
                "·".dimmed(),
                p.strip_prefix(root).unwrap_or(p).display()
            );
        }
        println!(
            "  Remove with: {}{}",
            remove_hint.cyan(),
            if dry_run {
                " (dry-run: not pruned)".dimmed().to_string()
            } else {
                String::new()
            }
        );
    }
}

#[cfg(test)]
#[path = "tests/bug_298_prune_tests.rs"]
mod bug_298_prune_tests;
