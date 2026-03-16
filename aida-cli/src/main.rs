mod cli;
#[cfg(feature = "remote")]
mod client;
mod mcp;
mod prompts;

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use std::collections::HashSet;
use uuid::Uuid;

use aida_core::{
    check_migration_status,
    check_scaffold_status,
    determine_requirements_path,
    export,
    get_registry_path,
    seed_meta_requirements,
    ArtifactType,
    BackendType,
    Cardinality,
    Comment,
    DatabaseBackend,
    FieldChange,
    FileStatus,
    // GitLab integration
    GitLabClient,
    GitLabConfig,
    IdFormat,
    IssueFilter,
    IssueState,
    MigrationCheck,
    NumberingStrategy,
    Registry,
    RelationshipDefinition,
    RelationshipType,
    ReportFormat,
    ReportGenerator,
    Requirement,
    RequirementPriority,
    RequirementStatus,
    RequirementType,
    RequirementsStore,
    ScaffoldConfig,
    Scaffolder,
    Storage,
    TraceLink,
};

use crate::cli::{
    Cli, Command, CommentCommand, ConfigCommand, DbCommand, FeatureCommand, GitLabCommand,
    QueueCommand, RelDefCommand, RelationshipCommand, ReportCommand, ScaffoldCommand,
    ServerCommand, TraceCommand, TypeCommand,
};

/// Get the default author from AIDA_AUTHOR environment variable or fall back to system user.
/// Format recommendation: "ai:claude:username" for AI-assisted work
fn get_default_author() -> String {
    if let Ok(author) = std::env::var("AIDA_AUTHOR") {
        author
    } else {
        // Fall back to system username
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME")) // Windows fallback
            .unwrap_or_else(|_| "Unknown".to_string())
    }
}

fn main() -> Result<()> {
    let mut cli = Cli::parse();

    // Check for AIDA_SERVER environment variable if --server not specified
    if cli.server.is_none() {
        cli.server = std::env::var("AIDA_SERVER").ok();
    }

    // Handle init before path resolution (no DB exists yet)
    if let Command::Init {
        no_skills,
        agent,
        no_hooks,
        force,
        distributed,
        registry_remote,
    } = &cli.command
    {
        if *distributed {
            handle_init_distributed(registry_remote.as_deref(), *force)?;
        } else {
            handle_init_command(*no_skills, agent, *no_hooks, *force)?;
        }
        return Ok(());
    }

    // Determine which requirements file to use
    // trace:REQ-0231 | ai:claude:high
    let requirements_path = if let Some(ref explicit_file) = cli.file {
        let explicit_path = std::path::PathBuf::from(explicit_file);
        // If path is a directory, use GitBackend and route through the backend API
        if explicit_path.is_dir()
            || (!explicit_path.exists() && explicit_path.extension().is_none())
        {
            return handle_git_backend_command(&explicit_path, &cli.command);
        }
        // User explicitly specified a file path - use it directly
        explicit_path
    } else {
        // Auto-detect: first find the base path, then check migration status
        let initial_path = determine_requirements_path(cli.project.as_deref())?;

        // If path is already a .db file, skip migration check (no YAML to migrate from)
        if initial_path.extension().and_then(|e| e.to_str()) == Some("db") {
            initial_path
        } else {
            // Check for migration status (REQ-0231)
            // Storage class now auto-detects SQLite vs YAML by file extension
            match check_migration_status(&initial_path) {
                MigrationCheck::NoMigration(path) => path,
                MigrationCheck::MigratedToSqlite {
                    yaml_path: _,
                    sqlite_path,
                } => {
                    // YAML was officially migrated - use SQLite
                    eprintln!(
                        "{}: Using SQLite database: {}",
                        "INFO".blue(),
                        sqlite_path.display()
                    );
                    sqlite_path
                }
                MigrationCheck::PossibleStaleYaml {
                    yaml_path: _,
                    sqlite_path,
                } => {
                    // Both exist but no marker - default to SQLite as it's likely more current
                    eprintln!(
                        "{}: Both YAML and SQLite exist. Using SQLite.",
                        "INFO".blue()
                    );
                    eprintln!("Use --file requirements.yaml to use YAML instead.");
                    sqlite_path
                }
            }
        }
    };

    let storage = Storage::new(requirements_path.clone());

    match &cli.command {
        Command::Add {
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
            interactive,
        } => {
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
                )?;
            }
        }
        Command::List {
            status,
            priority,
            r#type,
            feature,
            tags,
        } => {
            list_requirements(&storage, status, priority, r#type, feature, tags)?;
        }
        Command::Show { id } => {
            show_requirement(&storage, id)?;
        }
        Command::Edit {
            id,
            title,
            description,
            status,
            priority,
            r#type,
            owner,
            feature,
            tags,
            interactive,
        } => {
            // If any flags provided, use non-interactive mode; otherwise interactive
            let has_flags = title.is_some()
                || description.is_some()
                || status.is_some()
                || priority.is_some()
                || r#type.is_some()
                || owner.is_some()
                || feature.is_some()
                || tags.is_some();

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
                )?;
            }
        }
        Command::Del { id, yes } => {
            delete_requirement(&storage, id, *yes)?;
        }
        Command::Feature(feature_cmd) => {
            handle_feature_command(feature_cmd, &storage)?;
        }
        Command::Db(db_cmd) => {
            handle_db_command(db_cmd, &requirements_path)?;
        }
        Command::Rel(rel_cmd) => {
            handle_relationship_command(rel_cmd, &storage)?;
        }
        Command::RelDef(rel_def_cmd) => {
            handle_rel_def_command(rel_def_cmd, &storage)?;
        }
        Command::Comment(comment_cmd) => {
            handle_comment_command(comment_cmd, &storage)?;
        }
        Command::Config(config_cmd) => {
            handle_config_command(config_cmd, &storage)?;
        }
        Command::Type(type_cmd) => {
            handle_type_command(type_cmd, &storage)?;
        }
        Command::Export { format, output, id } => {
            handle_export_command(&storage, format, output.as_deref(), id.as_deref())?;
        }
        Command::Import {
            file,
            parent,
            on_conflict,
        } => {
            handle_import_command(&storage, file, parent.as_deref(), on_conflict)?;
        }
        Command::UserGuide { dark } => {
            open_user_guide(*dark)?;
        }
        Command::Server(server_cmd) => {
            handle_server_command(server_cmd, cli.server.as_deref())?;
        }
        Command::Trace(trace_cmd) => {
            handle_trace_command(trace_cmd, &storage)?;
        }
        Command::Report(report_cmd) => {
            let db_path_str = requirements_path.display().to_string();
            handle_report_command(report_cmd, &storage, &db_path_str)?;
        }
        Command::Scaffold(scaffold_cmd) => {
            handle_scaffold_command(scaffold_cmd, &storage, &requirements_path)?;
        }
        Command::Gitlab(gitlab_cmd) => {
            handle_gitlab_command(gitlab_cmd, &storage)?;
        }
        Command::McpServe => {
            mcp::run_mcp_server(&storage)?;
        }
        Command::Grep {
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
        Command::Queue(queue_cmd) => {
            handle_queue_command(queue_cmd, &storage)?;
        }
        Command::Search {
            query,
            case_sensitive,
            status,
            feature,
        } => {
            // Search is a simplified version of grep with sensible defaults:
            // - Case insensitive by default (unless -s/--case-sensitive)
            // - Searches all text fields (title, description, comments)
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
        Command::Init { .. } => {
            // Handled before path resolution above; unreachable
            unreachable!("Init command should be handled before path resolution");
        }
    }

    Ok(())
}

// trace:TASK-0001 | ai:claude:high
fn handle_init_command(no_skills: bool, agent: &str, no_hooks: bool, force: bool) -> Result<()> {
    let db_path = std::path::PathBuf::from("requirements.db");

    // Check existing state
    if db_path.exists() && !force {
        eprintln!(
            "{} AIDA is already initialized in this directory (requirements.db exists).",
            "!".yellow()
        );
        eprintln!("  Use {} to reinitialize.", "--force".bold());
        return Ok(());
    }

    // Create the database
    let storage = Storage::new(db_path.clone());
    let mut store = if db_path.exists() && force {
        storage.load()?
    } else {
        RequirementsStore::default()
    };

    // Seed META requirements
    seed_meta_requirements(&mut store)?;
    storage.save(&store)?;

    // Create docs/plans/
    std::fs::create_dir_all("docs/plans")?;

    // Build ScaffoldConfig with escape hatches
    let mut config = ScaffoldConfig::default();
    match agent {
        "claude" => {
            config.generate_agents_md = false;
            config.generate_codex_skills = false;
        }
        "codex" => {
            config.generate_claude_md = false;
            config.generate_commands = false;
            config.generate_skills = false;
            config.generate_claude_code_hooks = false;
        }
        "both" => {}
        _ => {
            anyhow::bail!(
                "Invalid --agent value '{}'. Use claude, codex, or both.",
                agent
            );
        }
    }

    if no_skills {
        config.generate_skills = false;
        config.generate_commands = false;
        config.generate_codex_skills = false;
        config.include_aida_req_skill = false;
        config.include_aida_plan_skill = false;
        config.include_aida_implement_skill = false;
        config.include_aida_capture_skill = false;
        config.include_aida_docs_skill = false;
        config.include_aida_release_skill = false;
        config.include_aida_evaluate_skill = false;
        config.include_aida_commit_skill = false;
        config.include_aida_sync_skill = false;
        config.include_aida_test_skill = false;
        config.include_aida_review_skill = false;
        config.include_aida_onboard_skill = false;
        config.include_aida_sprint_skill = false;
        config.include_aida_search_skill = false;
        config.include_aida_standup_skill = false;
    }
    if no_hooks {
        config.generate_git_hooks = false;
        config.generate_claude_code_hooks = false;
        config.include_commit_msg_hook = false;
        config.include_pre_commit_hook = false;
        config.include_validate_commit_hook = false;
        config.include_track_commits_hook = false;
    }

    // Run scaffold
    let root = std::env::current_dir().unwrap_or_default();
    let config_for_output = config.clone();
    let mut scaffolder = Scaffolder::with_database(root.clone(), config, db_path);
    let preview = scaffolder.preview(&store);

    let mut _created_count = 0;
    let mut updated_count = 0;
    let mut skipped_count = 0;

    for artifact in &preview.artifacts {
        let full_path = root.join(&artifact.path);
        let exists = full_path.exists();

        if exists && !force {
            skipped_count += 1;
            continue;
        }

        // Create parent directories if needed
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full_path, &artifact.content)?;

        if exists {
            updated_count += 1;
        } else {
            _created_count += 1;
        }
    }

    // Print post-init message
    println!();
    println!("{}", "AIDA initialized ✓".green().bold());
    println!();
    println!("  {}:", "Created".bold());
    println!(
        "    {}{}Requirements database (SQLite)",
        "requirements.db".white().bold(),
        " ".repeat(24)
    );
    if config_for_output.generate_claude_md {
        println!(
            "    {}{}Claude Code MCP integration",
            ".mcp.json".white().bold(),
            " ".repeat(29)
        );
        println!(
            "    {}{}Project context for AI sessions",
            "CLAUDE.md".white().bold(),
            " ".repeat(29)
        );
    }
    if config_for_output.generate_agents_md {
        println!(
            "    {}{}Project context for Codex-compatible agents",
            "AGENTS.md".white().bold(),
            " ".repeat(29)
        );
    }
    if config_for_output.generate_skills
        || config_for_output.generate_commands
        || config_for_output.generate_codex_skills
    {
        if config_for_output.generate_skills {
            println!(
                "    {}{}Workflow skills",
                ".claude/skills/".white().bold(),
                " ".repeat(23)
            );
        }
        if config_for_output.generate_commands {
            println!(
                "    {}{}Slash commands",
                ".claude/commands/".white().bold(),
                " ".repeat(21)
            );
        }
        if config_for_output.generate_codex_skills {
            println!(
                "    {}{}Workflow skills (Codex-compatible)",
                ".codex/skills/".white().bold(),
                " ".repeat(24)
            );
        }
    }
    if !no_hooks && config_for_output.generate_claude_code_hooks {
        println!(
            "    {}{}Commit validation hooks",
            ".claude/hooks/".white().bold(),
            " ".repeat(24)
        );
    }
    if !no_hooks && config_for_output.generate_git_hooks {
        println!(
            "    {}{}Git commit-msg hook",
            ".git/hooks/commit-msg".white().bold(),
            " ".repeat(18)
        );
    }
    println!(
        "    {}{}Implementation plan archive",
        "docs/plans/".white().bold(),
        " ".repeat(27)
    );

    if skipped_count > 0 {
        println!();
        println!(
            "  {} files skipped (already exist, use --force to overwrite)",
            skipped_count.to_string().yellow(),
        );
    }
    if updated_count > 0 {
        println!("  {} files updated", updated_count.to_string().blue(),);
    }

    println!();
    println!("  {}:", "Quick start".bold());
    println!(
        "    {}",
        "aida add --title \"User auth\" --type story --status draft".cyan()
    );
    println!("    {}", "aida list".cyan());

    if config_for_output.generate_claude_md {
        println!();
        println!("  {}:", "In Claude Code".bold());
        println!(
            "    {}{}Interactive project walkthrough",
            "/aida-onboard".cyan(),
            " ".repeat(15)
        );
        println!(
            "    {}{}Add a requirement",
            "/aida-req".cyan(),
            " ".repeat(19)
        );
    }
    if config_for_output.generate_agents_md {
        println!();
        println!("  {}:", "In Codex CLI".bold());
        println!(
            "    {}{}Read project guidance",
            "AGENTS.md".cyan(),
            " ".repeat(23)
        );
        println!(
            "    {}{}Run AIDA commands from terminal",
            "aida show FR-0001".cyan(),
            " ".repeat(15)
        );
    }
    println!(
        "    {}{}See all available skills",
        "/aida-".cyan(),
        " ".repeat(22)
    );
    println!();

    Ok(())
}

/// Load or create the distributed dispenser for a git-backed store.
/// Reads node config from {store}/.aida/node.toml; defaults to node_id=1
/// if no node registration has happened yet.
fn load_dispenser(
    store_path: &std::path::Path,
) -> Result<aida_core::models::DispenserHandle> {
    use aida_core::dispenser::{FileDispenser, IdMode};
    use aida_core::node::NodeConfig;

    let aida_dir = store_path.join(".aida");
    std::fs::create_dir_all(&aida_dir)?;

    let node_config_path = aida_dir.join("node.toml");
    let dispenser_path = aida_dir.join("dispenser.toml");

    // Load node_id from config, or default to 1 for local-only
    let node_id = if node_config_path.exists() {
        NodeConfig::load(&node_config_path)?.node_id
    } else {
        1 // default for unregistered local node
    };

    let mode = IdMode::Distributed { node_id };
    let dispenser = FileDispenser::open(dispenser_path, mode)?;

    Ok(aida_core::models::DispenserHandle(std::sync::Arc::new(dispenser)))
}

/// Handle commands routed to the GitBackend (when --file points to a directory).
fn handle_git_backend_command(
    store_path: &std::path::Path,
    command: &Command,
) -> Result<()> {
    let dispenser = load_dispenser(store_path)?;
    let backend = aida_core::GitBackend::new(store_path)?
        .with_dispenser(dispenser);

    match command {
        Command::List { status, r#type, .. } => {
            let store = backend.load()?;
            let reqs: Vec<&Requirement> = store
                .requirements
                .iter()
                .filter(|r| !r.archived)
                .filter(|r| {
                    status
                        .as_ref()
                        .map(|s| r.effective_status().eq_ignore_ascii_case(s))
                        .unwrap_or(true)
                })
                .filter(|r| {
                    r#type
                        .as_ref()
                        .map(|t| format!("{:?}", r.req_type).eq_ignore_ascii_case(t))
                        .unwrap_or(true)
                })
                .collect();

            if reqs.is_empty() {
                println!("No requirements found.");
            } else {
                // Check if any have agreed IDs
                let has_agreed = reqs.iter().any(|r| r.agreed_id.is_some());

                if has_agreed {
                    println!(
                        "{:<12} {:<14} {:<12} {:<10} {}",
                        "ID", "Node ID", "Type", "Status", "Title"
                    );
                    println!("{}", "─".repeat(78));
                    for req in &reqs {
                        println!(
                            "{:<12} {:<14} {:<12} {:<10} {}",
                            req.display_id(),
                            req.spec_id.as_deref().unwrap_or("-"),
                            format!("{:?}", req.req_type),
                            req.effective_status(),
                            req.title,
                        );
                    }
                } else {
                    println!(
                        "{:<14} {:<12} {:<10} {:<10} {}",
                        "ID", "Type", "Status", "Priority", "Title"
                    );
                    println!("{}", "─".repeat(74));
                    for req in &reqs {
                        println!(
                            "{:<14} {:<12} {:<10} {:<10} {}",
                            req.display_id(),
                            format!("{:?}", req.req_type),
                            req.effective_status(),
                            req.effective_priority(),
                            req.title,
                        );
                    }
                }
                println!("\n{} requirements", reqs.len());
            }
        }
        Command::Add {
            title,
            description,
            status,
            priority,
            r#type,
            owner,
            tags,
            prefix,
            ..
        } => {
            let mut req = Requirement::new(
                title
                    .clone()
                    .unwrap_or_else(|| "Untitled".to_string()),
                description
                    .clone()
                    .unwrap_or_default(),
            );
            if let Some(s) = status {
                req.set_status_from_str(&capitalize(s));
            }
            if let Some(p) = priority {
                req.set_priority_from_str(&capitalize(p));
            }
            if let Some(t) = r#type {
                if let Ok(rt) = parse_requirement_type(t) {
                    req.req_type = rt;
                }
            }
            if let Some(o) = owner {
                req.owner = o.clone();
            }
            if let Some(t) = tags {
                for tag in t.split(',') {
                    req.tags.insert(tag.trim().to_string());
                }
            }
            if let Some(p) = prefix {
                req.prefix_override = Some(p.to_uppercase());
            }

            // Use update_atomically to generate the ID with store's config
            let store = backend.update_atomically(|store| {
                let type_prefix = store.get_type_prefix(&req.req_type);
                store.add_requirement_with_id(
                    req.clone(),
                    None,
                    type_prefix.as_deref(),
                );
            })?;

            if let Some(last) = store.requirements.last() {
                // Write the individual object file
                aida_core::object_store::write_object(
                    &store_path.join("objects"),
                    last,
                )?;
                println!(
                    "Added: {} - {}",
                    last.spec_id.as_deref().unwrap_or("?"),
                    last.title
                );
            }
        }
        Command::Show { id } => {
            match backend.get_requirement_by_spec_id(id)? {
                Some(req) => {
                    println!("{}: {}", "ID".bold(), req.display_id());
                    if let Some(ref agreed) = req.agreed_id {
                        if req.spec_id.as_deref() != Some(agreed.as_str()) {
                            println!("{}: {}", "Agreed ID".bold(), agreed);
                        }
                    }
                    if req.agreed_id.is_some() {
                        println!("{}: {}", "Node ID".bold(), req.spec_id.as_deref().unwrap_or("-"));
                    }
                    println!("{}: {}", "UUID".bold(), req.id);
                    println!("{}: {}", "Title".bold(), req.title);
                    println!("{}: {:?}", "Type".bold(), req.req_type);
                    println!("{}: {}", "Status".bold(), req.effective_status());
                    println!("{}: {}", "Priority".bold(), req.effective_priority());
                    if !req.owner.is_empty() {
                        println!("{}: {}", "Owner".bold(), req.owner);
                    }
                    if !req.tags.is_empty() {
                        println!("{}: {}", "Tags".bold(), req.tags.iter().cloned().collect::<Vec<_>>().join(", "));
                    }
                    if !req.relationships.is_empty() {
                        println!("{}: {} relationship(s)", "Relations".bold(), req.relationships.len());
                    }
                    if !req.comments.is_empty() {
                        println!("{}: {} comment(s)", "Comments".bold(), req.comments.len());
                    }
                    if !req.description.is_empty() {
                        println!("\n{}", req.description);
                    }
                }
                None => {
                    eprintln!("Requirement not found: {}", id);
                }
            }
        }
        Command::Edit {
            id,
            title,
            description,
            status,
            priority,
            r#type,
            owner,
            feature,
            tags,
            ..
        } => {
            let mut req = backend
                .get_requirement_by_spec_id(id)?
                .ok_or_else(|| anyhow::anyhow!("Requirement not found: {}", id))?;

            let mut changed = false;
            if let Some(t) = title {
                req.title = t.clone();
                changed = true;
            }
            if let Some(d) = description {
                req.description = d.clone();
                changed = true;
            }
            if let Some(s) = status {
                req.set_status_from_str(&capitalize(s));
                changed = true;
            }
            if let Some(p) = priority {
                req.set_priority_from_str(&capitalize(p));
                changed = true;
            }
            if let Some(t) = r#type {
                if let Ok(rt) = parse_requirement_type(t) {
                    req.req_type = rt;
                    changed = true;
                }
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
                req.tags.clear();
                for tag in t.split(',') {
                    req.tags.insert(tag.trim().to_string());
                }
                changed = true;
            }

            if changed {
                req.modified_at = chrono::Utc::now();
                backend.update_requirement(&req)?;
                println!("Updated: {}", id);
            } else {
                println!("No changes specified. Use --title, --status, --priority, etc.");
            }
        }
        Command::Del { id, yes } => {
            let req = backend
                .get_requirement_by_spec_id(id)?
                .ok_or_else(|| anyhow::anyhow!("Requirement not found: {}", id))?;

            if !yes {
                println!("Delete {} — {}?", id, req.title);
                print!("Confirm (y/N): ");
                use std::io::Write;
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Cancelled.");
                    return Ok(());
                }
            }

            backend.delete_requirement(&req.id)?;
            println!("Deleted: {}", id);
        }
        Command::Search { query, status, .. } => {
            let store = backend.load()?;
            let query_lower = query.to_lowercase();

            let results: Vec<&Requirement> = store
                .requirements
                .iter()
                .filter(|r| !r.archived)
                .filter(|r| {
                    r.title.to_lowercase().contains(&query_lower)
                        || r.description.to_lowercase().contains(&query_lower)
                        || r.spec_id.as_deref().unwrap_or("").to_lowercase().contains(&query_lower)
                })
                .filter(|r| {
                    status
                        .as_ref()
                        .map(|s| r.effective_status().eq_ignore_ascii_case(s))
                        .unwrap_or(true)
                })
                .collect();

            if results.is_empty() {
                println!("No results for: {}", query);
            } else {
                println!(
                    "{:<14} {:<12} {:<10} {}",
                    "ID", "Type", "Status", "Title"
                );
                println!("{}", "─".repeat(65));
                for req in &results {
                    println!(
                        "{:<14} {:<12} {:<10} {}",
                        req.display_id(),
                        format!("{:?}", req.req_type),
                        req.effective_status(),
                        req.title,
                    );
                }
                println!("\n{} results", results.len());
            }
        }
        Command::Comment(CommentCommand::Add {
            id: req_id,
            content,
            author,
            ..
        }) => {
            let mut req = backend
                .get_requirement_by_spec_id(req_id)?
                .ok_or_else(|| anyhow::anyhow!("Requirement not found: {}", req_id))?;

            let now = chrono::Utc::now();
            let comment = aida_core::Comment {
                id: Uuid::now_v7(),
                content: content.clone().unwrap_or_default(),
                author: author.clone().unwrap_or_else(|| get_default_author()),
                created_at: now,
                modified_at: now,
                parent_id: None,
                replies: Vec::new(),
                reactions: Vec::new(),
            };

            req.comments.push(comment);
            req.modified_at = now;
            backend.update_requirement(&req)?;
            println!("Comment added to {}", req_id);
        }
        Command::Db(DbCommand::Info) => {
            let store = backend.load()?;
            println!("{}: {}", "Backend".bold(), "Git (sharded YAML)");
            println!("{}: {}", "Path".bold(), store_path.display());
            println!("{}: {}", "Requirements".bold(), store.requirements.len());
            println!("{}: {}", "Users".bold(), store.users.len());

            let file_count = aida_core::object_store::list_objects(&store_path.join("objects"))
                .map(|l| l.len())
                .unwrap_or(0);
            println!("{}: {}", "Object files".bold(), file_count);

            // Show git status
            if aida_core::git_ops::is_git_repo(store_path) {
                let has_changes = aida_core::git_ops::has_changes(store_path).unwrap_or(false);
                let status_str = if has_changes { "uncommitted changes" } else { "clean" };
                println!("{}: {}", "Git".bold(), status_str);
                if let Ok(sha) = aida_core::git_ops::head_sha(store_path) {
                    println!("{}: {}", "HEAD".bold(), sha);
                }
            }
        }
        // Phase 2: Relationship commands
        Command::Rel(RelationshipCommand::Add {
            from,
            to,
            r#type,
            bidirectional,
        }) => {
            let mut from_req = backend
                .get_requirement_by_spec_id(from)?
                .ok_or_else(|| anyhow::anyhow!("Source not found: {}", from))?;

            let to_req = backend
                .get_requirement_by_spec_id(to)?
                .ok_or_else(|| anyhow::anyhow!("Target not found: {}", to))?;

            let rel_type = match r#type.to_lowercase().as_str() {
                "parent" => RelationshipType::Parent,
                "child" => RelationshipType::Child,
                "duplicate" => RelationshipType::Duplicate,
                "verifies" => RelationshipType::Verifies,
                "verified-by" | "verifiedby" => RelationshipType::VerifiedBy,
                "references" => RelationshipType::References,
                other => RelationshipType::Custom(other.to_string()),
            };

            let rel = aida_core::models::Relationship {
                rel_type: rel_type.clone(),
                target_id: to_req.id,
                created_at: Some(chrono::Utc::now()),
                created_by: Some(get_default_author()),
            };

            from_req.relationships.push(rel);
            from_req.modified_at = chrono::Utc::now();
            backend.update_requirement(&from_req)?;
            println!("Added relationship: {} --[{:?}]--> {}", from, rel_type, to);

            if *bidirectional {
                let mut to_req = backend.get_requirement_by_spec_id(to)?.unwrap();
                let inverse_type = match &rel_type {
                    RelationshipType::Parent => RelationshipType::Child,
                    RelationshipType::Child => RelationshipType::Parent,
                    RelationshipType::Verifies => RelationshipType::VerifiedBy,
                    RelationshipType::VerifiedBy => RelationshipType::Verifies,
                    other => other.clone(),
                };
                let inv_rel = aida_core::models::Relationship {
                    rel_type: inverse_type.clone(),
                    target_id: from_req.id,
                    created_at: Some(chrono::Utc::now()),
                    created_by: Some(get_default_author()),
                };
                to_req.relationships.push(inv_rel);
                to_req.modified_at = chrono::Utc::now();
                backend.update_requirement(&to_req)?;
                println!("Added inverse: {} --[{:?}]--> {}", to, inverse_type, from);
            }
        }
        Command::Rel(RelationshipCommand::Remove { from, to, .. }) => {
            let mut from_req = backend
                .get_requirement_by_spec_id(from)?
                .ok_or_else(|| anyhow::anyhow!("Source not found: {}", from))?;

            // Look up target UUID
            let to_req = backend
                .get_requirement_by_spec_id(to)?
                .ok_or_else(|| anyhow::anyhow!("Target not found: {}", to))?;

            let before = from_req.relationships.len();
            from_req.relationships.retain(|r| r.target_id != to_req.id);
            let removed = before - from_req.relationships.len();

            if removed > 0 {
                from_req.modified_at = chrono::Utc::now();
                backend.update_requirement(&from_req)?;
                println!("Removed {} relationship(s) from {} to {}", removed, from, to);
            } else {
                println!("No relationship found from {} to {}", from, to);
            }
        }

        // Phase 1: Sync command
        Command::Db(DbCommand::Sync { pull, push, message }) => {
            if !aida_core::git_ops::is_git_repo(store_path) {
                anyhow::bail!("Not a git repository: {}", store_path.display());
            }

            let branch = aida_core::git_ops::current_branch(store_path)
                .unwrap_or_else(|_| "main".to_string());

            if *pull {
                println!("Pulling from origin/{}...", branch);
                match aida_core::git_ops::pull(store_path, "origin", &branch) {
                    Ok(()) => println!("  Pull complete."),
                    Err(e) => eprintln!("  Pull failed: {}", e),
                }
            }

            // Stage and commit any pending changes
            let has_changes = aida_core::git_ops::has_changes(store_path)?;
            if has_changes {
                let msg = message.as_deref().unwrap_or("chore: sync pending changes");
                aida_core::git_ops::add_all(store_path, "objects")?;
                if store_path.join("metadata.yaml").exists() {
                    aida_core::git_ops::add(store_path, &["metadata.yaml"])?;
                }
                if store_path.join("registry").exists() {
                    aida_core::git_ops::add_all(store_path, "registry")?;
                }
                aida_core::git_ops::commit(store_path, msg)?;
                println!("Committed: {}", msg);
            } else {
                println!("Nothing to commit.");
            }

            if *push {
                println!("Pushing to origin/{}...", branch);
                match aida_core::git_ops::push(store_path, "origin", &branch) {
                    Ok(true) => println!("  Push complete."),
                    Ok(false) => {
                        println!("  Push rejected. Pulling and retrying...");
                        aida_core::git_ops::pull_rebase(store_path, "origin", &branch)?;
                        aida_core::git_ops::push(store_path, "origin", &branch)?;
                        println!("  Push complete after rebase.");
                    }
                    Err(e) => eprintln!("  Push failed: {}", e),
                }
            }

            if !pull && !push {
                println!("Use --pull and/or --push to sync with remote.");
                println!("  aida --file {} db sync --pull --push", store_path.display());
            }
        }

        // Merge gate: assign agreed IDs
        Command::Db(DbCommand::MergeGate) => {
            if !aida_core::git_ops::is_git_repo(store_path) {
                anyhow::bail!("Not a git repository: {}", store_path.display());
            }

            let assignments = aida_core::git_ops::merge_gate(store_path)?;

            if assignments.is_empty() {
                println!("All objects already have agreed IDs.");
            } else {
                println!("Assigned {} agreed ID(s):", assignments.len());
                for (node_id, agreed_id) in &assignments {
                    println!("  {} → {}", node_id, agreed_id.green().bold());
                }
            }
        }

        // Phase 3: Export to git backend
        Command::Db(DbCommand::ExportGit { output }) => {
            let output_path = std::path::PathBuf::from(output);

            // Load from whatever backend we're using (this handler is already git,
            // but the export command is also available from the main CLI path)
            let store = backend.load()?;

            // Create the target git backend
            let target = aida_core::GitBackend::new(&output_path)?;

            // Initialize git repo if not already
            if !aida_core::git_ops::is_git_repo(&output_path) {
                aida_core::git_ops::init(&output_path)?;
                let git_name = aida_core::git_ops::git_config_get("user.name")
                    .unwrap_or_else(|_| "AIDA".to_string());
                let git_email = aida_core::git_ops::git_config_get("user.email")
                    .unwrap_or_else(|_| "aida@localhost".to_string());
                aida_core::git_ops::configure_user(&output_path, &git_name, &git_email)?;
            }

            target.save(&store)?;
            println!(
                "Exported {} requirements to {}",
                store.requirements.len(),
                output_path.display()
            );
        }

        _ => {
            eprintln!(
                "Command not yet supported for git backend.\n\
                 Supported: list, add, show, edit, del, search, comment add,\n\
                 rel add/remove, db info, db sync, db export-git"
            );
            std::process::exit(1);
        }
    }

    Ok(())
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn parse_requirement_type(s: &str) -> Result<RequirementType> {
    match s.to_lowercase().as_str() {
        "functional" | "fr" => Ok(RequirementType::Functional),
        "non-functional" | "nonfunctional" | "nfr" => Ok(RequirementType::NonFunctional),
        "system" | "sr" => Ok(RequirementType::System),
        "user" | "ur" => Ok(RequirementType::User),
        "bug" => Ok(RequirementType::Bug),
        "epic" => Ok(RequirementType::Epic),
        "story" => Ok(RequirementType::Story),
        "task" => Ok(RequirementType::Task),
        "spike" => Ok(RequirementType::Spike),
        "sprint" => Ok(RequirementType::Sprint),
        "folder" => Ok(RequirementType::Folder),
        "meta" => Ok(RequirementType::Meta),
        _ => anyhow::bail!("Unknown requirement type: {}", s),
    }
}

fn handle_init_distributed(registry_remote: Option<&str>, force: bool) -> Result<()> {
    use aida_core::git_ops;

    let cwd = std::env::current_dir()?;
    let aida_dir = cwd.join(".aida");
    let store_dir = cwd.join("aida-store");

    // Check if already initialized
    if aida_dir.join("node.toml").exists() && !force {
        eprintln!(
            "{} AIDA distributed mode is already initialized (.aida/node.toml exists).",
            "!".yellow()
        );
        eprintln!("  Use {} to reinitialize.", "--force".bold());
        return Ok(());
    }

    println!("{}", "Initializing AIDA in distributed mode...".bold());
    println!();

    // Create the local git-backed store directory
    if !store_dir.exists() {
        std::fs::create_dir_all(&store_dir)?;
    }

    // Initialize git repo if not already
    if !git_ops::is_git_repo(&store_dir) {
        git_ops::init(&store_dir)?;
        println!(
            "  {} git repository in {}",
            "Created".green(),
            "aida-store/".white().bold()
        );
    }

    // Configure git user from global git config or defaults
    let git_name = git_ops::git_config_get("user.name")
        .unwrap_or_else(|_| "AIDA User".to_string());
    let git_email = git_ops::git_config_get("user.email")
        .unwrap_or_else(|_| "aida@localhost".to_string());
    git_ops::configure_user(&store_dir, &git_name, &git_email)?;

    // Add remote if specified
    if let Some(remote) = registry_remote {
        // Check if remote already exists
        let has_remote = std::process::Command::new("git")
            .current_dir(&store_dir)
            .args(["remote", "get-url", "origin"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !has_remote {
            std::process::Command::new("git")
                .current_dir(&store_dir)
                .args(["remote", "add", "origin", remote])
                .output()?;
            println!(
                "  {} remote: {}",
                "Added".green(),
                remote.white().bold()
            );
        }
    }

    // Initialize the git backend (creates objects/ and metadata.yaml)
    let backend = aida_core::GitBackend::new(&store_dir)?;
    let store = aida_core::models::RequirementsStore::new();
    backend.save(&store)?;
    println!(
        "  {} {}",
        "Created".green(),
        "aida-store/metadata.yaml".white().bold()
    );
    println!(
        "  {} {}",
        "Created".green(),
        "aida-store/objects/".white().bold()
    );

    // Create initial commit
    git_ops::add(&store_dir, &["metadata.yaml"])?;
    std::fs::create_dir_all(store_dir.join("objects"))?;
    // Create a .gitkeep so objects/ is tracked
    std::fs::write(store_dir.join("objects/.gitkeep"), "")?;
    git_ops::add(&store_dir, &["objects/.gitkeep"])?;

    // Create .gitignore for node-local files
    let gitignore_content = "# Node-local state (not shared)\n.aida/\n*.lock\n";
    std::fs::write(store_dir.join(".gitignore"), gitignore_content)?;
    git_ops::add(&store_dir, &[".gitignore"])?;

    git_ops::commit(&store_dir, "chore: initialize AIDA distributed store")?;

    // If we have a remote, push the initial commit and register the node
    if registry_remote.is_some() {
        let branch = git_ops::current_branch(&store_dir)
            .unwrap_or_else(|_| "main".to_string());

        // Push initial commit
        match git_ops::push(&store_dir, "origin", &branch) {
            Ok(true) => {
                println!(
                    "  {} initial commit to remote",
                    "Pushed".green(),
                );
            }
            Ok(false) => {
                // Remote has content — pull first then push
                git_ops::pull_rebase(&store_dir, "origin", &branch)?;
                git_ops::push(&store_dir, "origin", &branch)?;
                println!(
                    "  {} with remote and pushed",
                    "Synced".green(),
                );
            }
            Err(e) => {
                eprintln!(
                    "  {} Failed to push to remote: {}",
                    "Warning:".yellow(),
                    e
                );
                eprintln!("  You can push later with: cd aida-store && git push -u origin {}", branch);
            }
        }

        // Register this node
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        match git_ops::register_node(&store_dir, 1, &hostname) {
            Ok(node_id) => {
                println!(
                    "  {} node {} ({})",
                    "Registered".green(),
                    node_id.to_string().white().bold(),
                    hostname
                );
            }
            Err(e) => {
                eprintln!(
                    "  {} Node registration failed: {}",
                    "Warning:".yellow(),
                    e
                );
                eprintln!("  You can register later when the remote is available.");
            }
        }
    } else {
        println!();
        println!(
            "  {} No --registry-remote specified.",
            "Note:".yellow()
        );
        println!("  The store is local-only until you add a remote:");
        println!("    cd aida-store && git remote add origin <url>");
        println!("    aida init --distributed --registry-remote <url>");
    }

    // Create .aida/ config in the project root (not the store)
    std::fs::create_dir_all(&aida_dir)?;
    let config_content = format!(
        "# AIDA distributed mode configuration\n\
         [deployment]\n\
         mode = \"distributed\"\n\
         store_path = \"aida-store\"\n"
    );
    std::fs::write(aida_dir.join("config.toml"), &config_content)?;

    println!();
    println!("{}", "AIDA distributed mode initialized".green().bold());
    println!();
    println!("  {}:", "Directory layout".bold());
    println!("    {:<30} Project config", ".aida/config.toml".white().bold());
    println!("    {:<30} Git-backed object store", "aida-store/".white().bold());
    println!("    {:<30} Sharded requirement files", "aida-store/objects/".white().bold());
    println!("    {:<30} Store metadata & counters", "aida-store/metadata.yaml".white().bold());
    println!();
    println!("  {}:", "Quick start".bold());
    println!(
        "    {}",
        "aida --file aida-store add --title \"First req\" --type functional".cyan()
    );
    println!("    {}", "aida --file aida-store list".cyan());
    println!("    {}", "ls aida-store/objects/".cyan());
    println!();

    Ok(())
}

/// Get a git config value from the global config.
fn _git_config_get_global(key: &str) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["config", "--global", key])
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        anyhow::bail!("git config {} not set", key)
    }
}

fn handle_server_command(cmd: &ServerCommand, server_addr: Option<&str>) -> Result<()> {
    let server_addr = server_addr.ok_or_else(|| {
        anyhow::anyhow!(
            "Server address required. Use --server flag or set AIDA_SERVER environment variable."
        )
    })?;

    #[cfg(feature = "remote")]
    {
        // Create a tokio runtime for async operations
        let rt = tokio::runtime::Runtime::new()?;

        match cmd {
            ServerCommand::Status => {
                rt.block_on(client::get_server_status(server_addr))?;
            }
            ServerCommand::List {
                status,
                feature,
                limit,
            } => {
                rt.block_on(client::list_requirements(
                    server_addr,
                    status.as_deref(),
                    feature.as_deref(),
                    *limit,
                ))?;
            }
            ServerCommand::Get { id } => {
                rt.block_on(client::get_requirement(server_addr, id))?;
            }
            ServerCommand::Ping => {
                rt.block_on(client::ping_server(server_addr))?;
            }
        }
        Ok(())
    }

    #[cfg(not(feature = "remote"))]
    {
        let _ = cmd; // suppress unused warning
        let _ = server_addr;
        anyhow::bail!(
            "Remote server support is not enabled. \
            Build with: cargo build -p aida-cli --features remote"
        )
    }
}

fn add_requirement_interactive(storage: &Storage) -> Result<()> {
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

fn add_requirement_cli(
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
        Some(parse_requirement_id(parent_id, &store)?)
    } else {
        None
    };

    // Create a requirement with basic data
    let mut requirement = Requirement::new(title, description);

    // Set optional fields
    if let Some(status) = status_str {
        requirement.status = parse_status(status)?;
    }

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

    // Add parent relationship if specified
    if let Some(parent_id) = parent_uuid {
        store
            .add_relationship(&id, RelationshipType::Parent, &parent_id, false)
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

    // Show parent relationship if created
    if let Some(parent_id_str) = parent {
        println!("Parent: {}", parent_id_str.cyan());
    }

    Ok(())
}

fn list_requirements(
    storage: &Storage,
    status: &Option<String>,
    priority: &Option<String>,
    req_type: &Option<String>,
    feature: &Option<String>,
    tags: &Option<String>,
) -> Result<()> {
    // Load requirements
    let store = storage.load()?;
    let mut requirements = store.requirements;

    // Apply filters if provided
    if let Some(status_str) = status {
        let status_filter = parse_status(status_str)?;
        requirements.retain(|r| r.status == status_filter);
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
        requirements.retain(|r| tag_filters.iter().any(|tag| r.tags.contains(tag)));
    }

    // Display the requirements
    if requirements.is_empty() {
        println!("{}", "No requirements found.".yellow());
        return Ok(());
    }

    println!(
        "{:<10} | {:<36} | {:<30} | {:<10} | {:<10} | {:<15}",
        "SPEC-ID", "UUID", "Title", "Status", "Priority", "Feature"
    );
    println!("{}", "-".repeat(120));

    for req in requirements {
        let status_str = match req.status {
            RequirementStatus::Draft => "Draft".yellow(),
            RequirementStatus::Approved => "Approved".blue(),
            RequirementStatus::Planned => "Planned".cyan(),
            RequirementStatus::InProgress => "In Progress".magenta(),
            RequirementStatus::Completed => "Completed".green(),
            RequirementStatus::Rejected => "Rejected".red(),
        };

        let priority_str = match req.priority {
            RequirementPriority::High => "High".red(),
            RequirementPriority::Medium => "Medium".yellow(),
            RequirementPriority::Low => "Low".green(),
        };

        let spec_id_display = req.spec_id.as_ref().map(|s| s.as_str()).unwrap_or("-");

        println!(
            "{:<10} | {:<36} | {:<30} | {:<10} | {:<10} | {:<15}",
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

fn show_requirement(storage: &Storage, id_str: &str) -> Result<()> {
    // Load requirements first (needed for SPEC-ID lookup)
    let store = storage.load()?;

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

    let status_str = match req.status {
        RequirementStatus::Draft => "Draft".yellow(),
        RequirementStatus::Approved => "Approved".blue(),
        RequirementStatus::Planned => "Planned".cyan(),
        RequirementStatus::InProgress => "In Progress".magenta(),
        RequirementStatus::Completed => "Completed".green(),
        RequirementStatus::Rejected => "Rejected".red(),
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
    };
    println!("{}: {}", "Type".blue(), type_str);

    println!("{}: {}", "Owner".blue(), req.owner);
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

    if !req.relationships.is_empty() {
        println!("\n{}:", "Relationships".green());
        for relationship in &req.relationships {
            let target = store.get_requirement_by_id(&relationship.target_id);
            if let Some(target_req) = target {
                let target_spec = target_req.spec_id.as_deref().unwrap_or("N/A");

                // Format the relationship description based on type
                let description = match &relationship.rel_type {
                    RelationshipType::Parent => format!("is parent of"),
                    RelationshipType::Child => format!("is child of"),
                    RelationshipType::Duplicate => format!("is duplicate of"),
                    RelationshipType::Verifies => format!("verifies"),
                    RelationshipType::VerifiedBy => format!("is verified by"),
                    RelationshipType::References => format!("references"),
                    RelationshipType::Custom(name) => format!("{}", name),
                };

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

// trace:REQ-0232 | ai:claude:high
/// Edit a requirement non-interactively using CLI flags
fn edit_requirement_cli(
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
    if let Some(status_str) = status {
        let new_status = match status_str.to_lowercase().as_str() {
            "draft" => RequirementStatus::Draft,
            "approved" => RequirementStatus::Approved,
            "planned" => RequirementStatus::Planned,
            "in_progress" | "in-progress" | "inprogress" => RequirementStatus::InProgress,
            "completed" => RequirementStatus::Completed,
            "rejected" => RequirementStatus::Rejected,
            _ => anyhow::bail!("Invalid status '{}'. Use: draft, approved, planned, in_progress, completed, rejected", status_str),
        };
        if new_status != req.status {
            changes.push(Requirement::field_change(
                "status",
                format!("{:?}", req.status),
                format!("{:?}", new_status),
            ));
            req.status = new_status;
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
            _ => anyhow::bail!("Invalid type '{}'. Use: functional, non-functional, system, user, bug, epic, story, task, spike, sprint, folder, meta", type_str),
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
        let new_tags: HashSet<String> = tags_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let old_tags: String = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        let new_tags_str: String = new_tags.iter().cloned().collect::<Vec<_>>().join(", ");
        if new_tags != req.tags {
            changes.push(Requirement::field_change("tags", old_tags, new_tags_str));
            req.tags = new_tags;
        }
    }

    if changes.is_empty() {
        println!("{} No changes made to {}", "!".yellow(), spec_id);
        return Ok(());
    }

    // Record changes with CLI as author
    req.record_change("CLI".to_string(), changes.clone());

    // Save changes
    storage.save(&store)?;
    println!(
        "{} Updated {} ({} field(s) changed)",
        "✓".green(),
        spec_id,
        changes.len()
    );

    Ok(())
}

fn edit_requirement_interactive(storage: &Storage, id_str: &str) -> Result<()> {
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
    let old_req = req.clone();

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
        RequirementStatus::Completed,
        RequirementStatus::Rejected,
    ];
    if let Ok(new_status) = inquire::Select::new("Status:", status_options).prompt() {
        if new_status != req.status {
            changes.push(Requirement::field_change(
                "status",
                format!("{:?}", req.status),
                format!("{:?}", new_status),
            ));
            req.status = new_status;
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

    // Record changes
    req.record_change(author, changes);

    // Save changes
    storage.save(&store)?;
    println!("{}", "Requirement updated successfully!".green());

    Ok(())
}

fn delete_requirement(storage: &Storage, id_str: &str, skip_confirm: bool) -> Result<()> {
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
        let confirm = inquire::Confirm::new("Are you sure you want to delete this requirement?")
            .with_default(false)
            .prompt()?;

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

fn parse_uuid(id_str: &str) -> Result<Uuid> {
    Uuid::parse_str(id_str).with_context(|| format!("Invalid UUID: {}", id_str))
}

/// Parse requirement ID - accepts either UUID or SPEC-ID
fn parse_requirement_id(id_str: &str, store: &RequirementsStore) -> Result<Uuid> {
    // Try parsing as UUID first
    if let Ok(uuid) = Uuid::parse_str(id_str) {
        return Ok(uuid);
    }

    // Try as SPEC-ID
    if let Some(req) = store.get_requirement_by_spec_id(id_str) {
        return Ok(req.id);
    }

    anyhow::bail!(
        "Invalid requirement ID: '{}'. Must be either a UUID or SPEC-ID (e.g., SPEC-001)",
        id_str
    )
}

fn parse_status(status_str: &str) -> Result<RequirementStatus> {
    match status_str.to_lowercase().as_str() {
        "draft" => Ok(RequirementStatus::Draft),
        "approved" => Ok(RequirementStatus::Approved),
        "planned" => Ok(RequirementStatus::Planned),
        "in_progress" | "in-progress" | "inprogress" => Ok(RequirementStatus::InProgress),
        "completed" => Ok(RequirementStatus::Completed),
        "rejected" => Ok(RequirementStatus::Rejected),
        _ => anyhow::bail!("Invalid status: {}", status_str),
    }
}

fn parse_priority(priority_str: &str) -> Result<RequirementPriority> {
    match priority_str.to_lowercase().as_str() {
        "high" => Ok(RequirementPriority::High),
        "medium" => Ok(RequirementPriority::Medium),
        "low" => Ok(RequirementPriority::Low),
        _ => anyhow::bail!("Invalid priority: {}", priority_str),
    }
}

fn parse_type(type_str: &str) -> Result<RequirementType> {
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
        _ => anyhow::bail!("Invalid requirement type: {}", type_str),
    }
}

/// Handle feature management subcommands
fn handle_feature_command(cmd: &FeatureCommand, storage: &Storage) -> Result<()> {
    // Load existing requirements
    let mut store = storage.load()?;

    match cmd {
        FeatureCommand::Add {
            name,
            prefix,
            interactive,
        } => {
            let should_be_interactive = *interactive || name.is_none() || prefix.is_none();

            if should_be_interactive {
                // Use interactive prompting
                let feature_name = crate::prompts::prompt_new_feature(&mut store)?;
                println!(
                    "{} Feature '{}' created successfully.",
                    "✓".green(),
                    feature_name
                );
            } else {
                // Use command line arguments
                let name = name
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("Feature name is required"))?;
                let prefix = prefix
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("Feature prefix is required"))?;

                // Add feature with prefix to the new system
                let feature = store.add_feature(&name, &prefix)?;
                println!(
                    "{} Feature '{}' created with prefix '{}'.",
                    "✓".green(),
                    feature.name,
                    feature.prefix
                );
            }

            // Save the updated store
            storage.save(&store)?;
        }
        FeatureCommand::List => {
            // Show both legacy features and new feature definitions
            println!("{}", "Defined Features:".blue().bold());
            println!("{:<10} | {:<10} | {:<30}", "Number", "Prefix", "Name");
            println!("{}", "-".repeat(55));

            if store.features.is_empty() {
                println!("{}", "(No features defined yet)".dimmed());
            } else {
                for feature in &store.features {
                    println!(
                        "{:<10} | {:<10} | {:<30}",
                        feature.number, feature.prefix, feature.name
                    );
                }
            }

            // Also show legacy feature names from requirements
            let legacy_features = store.get_feature_names();
            if !legacy_features.is_empty() {
                println!("\n{}", "Legacy Features (from requirements):".yellow());
                for feature in legacy_features {
                    println!("  - {}", feature);
                }
            }
        }
        FeatureCommand::Show { name } => {
            // Try to find in new feature definitions first
            if let Some(feature) = store
                .get_feature_by_name(name)
                .or_else(|| store.get_feature_by_prefix(name))
            {
                println!("{}: {}", "Feature".blue(), feature.name);
                println!("{}: {}", "Prefix".blue(), feature.prefix);
                println!("{}: {}", "Number".blue(), feature.number);
                if !feature.description.is_empty() {
                    println!("{}: {}", "Description".blue(), feature.description);
                }
            } else {
                // Fall back to legacy feature search
                let features = store.get_feature_names();
                let mut found = false;

                for feature in features {
                    if feature.contains(name) {
                        println!("{}: {}", "Feature".blue(), feature);

                        // Find requirements with this feature
                        println!("\n{}", "Requirements:".blue());
                        let requirements: Vec<&Requirement> = store
                            .requirements
                            .iter()
                            .filter(|r| r.feature == feature)
                            .collect();

                        if requirements.is_empty() {
                            println!("No requirements found with this feature.");
                        } else {
                            println!(
                                "{:<12} | {:<30} | {:<10} | {:<10}",
                                "ID", "Title", "Status", "Priority"
                            );
                            println!("{}", "-".repeat(70));

                            for req in requirements {
                                let spec_id = req.spec_id.as_deref().unwrap_or("-");
                                let status_str = format!("{:?}", req.status);
                                let priority_str = format!("{:?}", req.priority);

                                println!(
                                    "{:<12} | {:<30} | {:<10} | {:<10}",
                                    spec_id,
                                    &req.title[..req.title.len().min(30)],
                                    status_str,
                                    priority_str
                                );
                            }
                        }

                        found = true;
                        break;
                    }
                }

                if !found {
                    println!("{} Feature '{}' not found.", "!".yellow(), name);
                }
            }
        }
        FeatureCommand::Edit {
            name,
            new_name,
            new_prefix,
            interactive,
        } => {
            // Try to find in new feature definitions first
            if let Some(idx) = store.features.iter().position(|f| {
                f.name.to_lowercase() == name.to_lowercase() || f.prefix == name.to_uppercase()
            }) {
                let old_name = store.features[idx].name.clone();
                let old_prefix = store.features[idx].prefix.clone();

                if *interactive || (new_name.is_none() && new_prefix.is_none()) {
                    // Interactive mode
                    let updated_name = inquire::Text::new("New name:")
                        .with_default(&old_name)
                        .prompt()?;
                    let updated_prefix = inquire::Text::new("New prefix:")
                        .with_default(&old_prefix)
                        .prompt()?;

                    store.features[idx].name = updated_name;
                    store.features[idx].prefix = updated_prefix.to_uppercase();
                } else {
                    if let Some(n) = new_name {
                        store.features[idx].name = n.clone();
                    }
                    if let Some(p) = new_prefix {
                        store.features[idx].prefix = p.to_uppercase();
                    }
                }

                storage.save(&store)?;
                println!("{} Feature updated successfully.", "✓".green());
            } else {
                // Fall back to legacy feature handling
                let features = store.get_feature_names();
                let mut found = false;

                for feature in features {
                    if feature.contains(name) {
                        let new_feature_name = if *interactive || new_name.is_none() {
                            crate::prompts::prompt_edit_feature(&feature)?
                        } else {
                            let new_name = new_name.clone().unwrap();
                            if let Some((prefix, _)) = feature.split_once('-') {
                                if prefix.parse::<u32>().is_ok() {
                                    format!("{}-{}", prefix, new_name)
                                } else {
                                    new_name
                                }
                            } else {
                                new_name
                            }
                        };

                        store.update_feature_name(&feature, &new_feature_name);
                        storage.save(&store)?;
                        println!(
                            "{} Feature '{}' renamed to '{}'.",
                            "✓".green(),
                            feature,
                            new_feature_name
                        );
                        found = true;
                        break;
                    }
                }

                if !found {
                    println!("{} Feature '{}' not found.", "!".yellow(), name);
                }
            }
        }
    }

    Ok(())
}

/// Handle ID configuration commands
fn handle_config_command(cmd: &ConfigCommand, storage: &Storage) -> Result<()> {
    let mut store = storage.load()?;

    match cmd {
        ConfigCommand::Show => {
            println!("{}", "ID Configuration:".blue().bold());
            println!();

            let format_str = match store.id_config.format {
                IdFormat::SingleLevel => "Single-level (PREFIX-NNN)",
                IdFormat::TwoLevel => "Two-level (FEATURE-TYPE-NNN)",
            };
            println!("{}: {}", "Format".cyan(), format_str);

            let numbering_str = match store.id_config.numbering {
                NumberingStrategy::Global => "Global (one counter for all)",
                NumberingStrategy::PerPrefix => "Per-prefix (separate counter per prefix)",
                NumberingStrategy::PerFeatureType => "Per feature+type combination",
            };
            println!("{}: {}", "Numbering".cyan(), numbering_str);

            println!("{}: {}", "Digits".cyan(), store.id_config.digits);
            println!(
                "{}: {}",
                "Next global number".cyan(),
                store.next_spec_number
            );

            if !store.prefix_counters.is_empty() {
                println!("\n{}", "Prefix Counters:".blue());
                for (prefix, counter) in &store.prefix_counters {
                    println!("  {}: {}", prefix, counter);
                }
            }
        }
        ConfigCommand::Format { format } => {
            store.id_config.format = match format.to_lowercase().as_str() {
                "single" | "single-level" | "1" => IdFormat::SingleLevel,
                "two" | "two-level" | "2" => IdFormat::TwoLevel,
                _ => anyhow::bail!("Invalid format. Use 'single' or 'two'."),
            };
            storage.save(&store)?;
            println!(
                "{} ID format set to {:?}",
                "✓".green(),
                store.id_config.format
            );
        }
        ConfigCommand::Numbering { strategy } => {
            store.id_config.numbering = match strategy.to_lowercase().as_str() {
                "global" => NumberingStrategy::Global,
                "per-prefix" | "prefix" => NumberingStrategy::PerPrefix,
                "per-feature-type" | "feature-type" => NumberingStrategy::PerFeatureType,
                _ => anyhow::bail!(
                    "Invalid strategy. Use 'global', 'per-prefix', or 'per-feature-type'."
                ),
            };
            storage.save(&store)?;
            println!(
                "{} Numbering strategy set to {:?}",
                "✓".green(),
                store.id_config.numbering
            );
        }
        ConfigCommand::Digits { digits } => {
            if *digits < 1 || *digits > 6 {
                anyhow::bail!("Digits must be between 1 and 6");
            }
            store.id_config.digits = *digits;
            storage.save(&store)?;
            println!("{} ID digits set to {}", "✓".green(), digits);
        }
        ConfigCommand::Migrate { yes } => {
            if !*yes {
                println!(
                    "{}",
                    "This will regenerate all requirement IDs based on current configuration."
                        .yellow()
                );
                println!("Current requirements: {}", store.requirements.len());
                let confirm = inquire::Confirm::new("Are you sure you want to migrate?")
                    .with_default(false)
                    .prompt()?;
                if !confirm {
                    println!("Migration cancelled.");
                    return Ok(());
                }
            }

            store.migrate_to_new_id_format();
            storage.save(&store)?;
            println!(
                "{} Successfully migrated {} requirements to new ID format.",
                "✓".green(),
                store.requirements.len()
            );
        }
    }

    Ok(())
}

/// Handle requirement type commands
fn handle_type_command(cmd: &TypeCommand, storage: &Storage) -> Result<()> {
    let mut store = storage.load()?;

    match cmd {
        TypeCommand::List => {
            println!("{}", "Requirement Types:".blue().bold());
            println!("{:<20} | {:<10} | {}", "Name", "Prefix", "Description");
            println!("{}", "-".repeat(60));

            for type_def in &store.id_config.requirement_types {
                println!(
                    "{:<20} | {:<10} | {}",
                    type_def.name, type_def.prefix, type_def.description
                );
            }
        }
        TypeCommand::Add {
            name,
            prefix,
            description,
        } => {
            let desc = description.clone().unwrap_or_default();
            store.add_requirement_type(name, prefix, &desc)?;
            storage.save(&store)?;
            println!(
                "{} Requirement type '{}' added with prefix '{}'.",
                "✓".green(),
                name,
                prefix.to_uppercase()
            );
        }
        TypeCommand::Remove { name, yes } => {
            // Find the type
            let idx = store.id_config.requirement_types.iter().position(|t| {
                t.name.to_lowercase() == name.to_lowercase() || t.prefix == name.to_uppercase()
            });

            if let Some(idx) = idx {
                let type_def = &store.id_config.requirement_types[idx];

                if !*yes {
                    println!(
                        "About to remove type '{}' (prefix: {})",
                        type_def.name, type_def.prefix
                    );
                    let confirm = inquire::Confirm::new("Are you sure?")
                        .with_default(false)
                        .prompt()?;
                    if !confirm {
                        println!("Removal cancelled.");
                        return Ok(());
                    }
                }

                let removed = store.id_config.requirement_types.remove(idx);
                storage.save(&store)?;
                println!(
                    "{} Requirement type '{}' removed.",
                    "✓".green(),
                    removed.name
                );
            } else {
                println!("{} Type '{}' not found.", "!".yellow(), name);
            }
        }
    }

    Ok(())
}

fn handle_db_command(cmd: &DbCommand, requirements_path: &std::path::PathBuf) -> Result<()> {
    match cmd {
        DbCommand::Register {
            name,
            path,
            description,
            default,
            interactive,
        } => {
            // Get registry path
            let registry_path = get_registry_path()?;

            // Ensure registry exists
            if !registry_path.exists() {
                Registry::create_default(&registry_path)?;
            }

            // Load registry
            let mut registry = Registry::load(&registry_path)?;

            // Default to interactive mode if no specific arguments are provided or interactive flag is set
            let should_be_interactive =
                *interactive || (name.is_none() && path.is_none() && description.is_none());

            // Project details to register
            let (project_name, project_path, project_description, is_default) =
                if should_be_interactive {
                    // Use interactive prompting
                    crate::prompts::prompt_register_project()?
                } else {
                    // Use command line arguments
                    let project_name = name
                        .clone()
                        .ok_or_else(|| anyhow::anyhow!("Project name is required"))?;

                    let project_path = path
                        .clone()
                        .ok_or_else(|| anyhow::anyhow!("Project path is required"))?;

                    let project_description = description.clone().unwrap_or_default();

                    (project_name, project_path, project_description, *default)
                };

            // Register project
            registry.register_project(
                project_name.clone(),
                project_path.to_string_lossy().to_string(),
                project_description,
            );

            // Set as default if requested
            if is_default {
                registry.set_default_project(&project_name)?;
            }

            // Save registry
            registry.save(&registry_path)?;

            println!(
                "{} Project '{}' registered successfully.",
                "✓".green(),
                project_name
            );
            if is_default {
                println!("{} Project '{}' set as default.", "✓".green(), project_name);
            }
        }
        DbCommand::Path { name } => {
            // If a name is provided, try to get that specific project
            if let Some(project_name) = name {
                // Get registry path
                let registry_path = get_registry_path()?;

                // Ensure registry exists
                if !registry_path.exists() {
                    Registry::create_default(&registry_path)?;
                }

                // Load registry
                let registry = Registry::load(&registry_path)?;

                // Find the project
                if let Some(project) = registry.get_project(project_name) {
                    println!("{}", project.path);
                } else {
                    println!(
                        "{} Project '{}' not found in registry. Use 'req db register' to add it.",
                        "!".yellow(),
                        project_name
                    );
                }
            } else {
                // Use the already determined path
                println!("{}", requirements_path.display());
            }
        }
        DbCommand::Migrate {
            from,
            to,
            output,
            force,
        } => {
            // trace:REQ-0231,FR-0316 | ai:claude:high
            use aida_core::create_backend;

            let source_format = match from.to_lowercase().as_str() {
                "yaml" | "yml" => "yaml",
                "sqlite" | "db" => "sqlite",
                "postgres" | "postgresql" | "pg" => "postgres",
                _ => {
                    println!(
                        "{} Invalid source format '{}'. Use 'yaml', 'sqlite', or 'postgres'.",
                        "!".red(),
                        from
                    );
                    return Ok(());
                }
            };

            let target_format = match to.to_lowercase().as_str() {
                "yaml" | "yml" => "yaml",
                "sqlite" | "db" => "sqlite",
                "postgres" | "postgresql" | "pg" => "postgres",
                _ => {
                    println!(
                        "{} Invalid target format '{}'. Use 'yaml', 'sqlite', or 'postgres'.",
                        "!".red(),
                        to
                    );
                    return Ok(());
                }
            };

            if source_format == target_format {
                println!("{} Source and target formats are the same.", "!".yellow());
                return Ok(());
            }

            // Handle PostgreSQL migrations
            if target_format == "postgres" {
                let conn_string = output.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("PostgreSQL migration requires --output with a connection string (e.g., postgres://user:pass@host:5432/db)")
                })?;

                println!(
                    "Migrating from {} to PostgreSQL...",
                    requirements_path.display()
                );

                let source_backend = create_backend(requirements_path, None)?;
                let count = aida_core::migrate_to_postgres(source_backend.as_ref(), conn_string)?;

                println!(
                    "{} Successfully migrated {} requirements to PostgreSQL",
                    "✓".green(),
                    count
                );
                return Ok(());
            }

            if source_format == "postgres" {
                let conn_string = requirements_path.to_string_lossy();
                if !conn_string.starts_with("postgres://")
                    && !conn_string.starts_with("postgresql://")
                {
                    println!("{} For PostgreSQL source, use --file with a connection string (e.g., postgres://user:pass@host:5432/db)", "!".red());
                    return Ok(());
                }

                let target_ext = if target_format == "yaml" {
                    "yaml"
                } else {
                    "db"
                };
                let target_path = output
                    .as_ref()
                    .map(|s| std::path::PathBuf::from(s))
                    .unwrap_or_else(|| {
                        std::path::PathBuf::from(format!("requirements.{}", target_ext))
                    });

                if target_path.exists() && !*force {
                    println!(
                        "{} Target file '{}' already exists. Use --force to overwrite.",
                        "!".yellow(),
                        target_path.display()
                    );
                    return Ok(());
                }

                println!("Migrating from PostgreSQL to {}...", target_path.display());

                let target_backend = create_backend(&target_path, None)?;
                let count =
                    aida_core::migrate_from_postgres(&conn_string, target_backend.as_ref())?;

                println!(
                    "{} Successfully migrated {} requirements to '{}'",
                    "✓".green(),
                    count,
                    target_path.display()
                );
                return Ok(());
            }

            // Standard YAML <-> SQLite migration
            let target_ext = if target_format == "yaml" {
                "yaml"
            } else {
                "db"
            };
            let target_path = output
                .as_ref()
                .map(|s| std::path::PathBuf::from(s))
                .unwrap_or_else(|| requirements_path.with_extension(target_ext));

            if target_path.exists() && !*force {
                println!(
                    "{} Target file '{}' already exists. Use --force to overwrite.",
                    "!".yellow(),
                    target_path.display()
                );
                return Ok(());
            }

            println!(
                "Migrating from {} to {}...",
                requirements_path.display(),
                target_path.display()
            );

            let count = if source_format == "yaml" {
                aida_core::migrate_yaml_to_sqlite(requirements_path, &target_path)?
            } else {
                aida_core::migrate_sqlite_to_yaml(requirements_path, &target_path)?
            };

            println!(
                "{} Successfully migrated {} requirements to '{}'",
                "✓".green(),
                count,
                target_path.display()
            );
        }
        DbCommand::Info => {
            // trace:REQ-0231,FR-0316 | ai:claude:high
            use aida_core::{create_backend, BackendType};

            let backend = create_backend(requirements_path, None)?;
            let store = backend.load()?;

            println!("{}", "Database Information".bold());
            println!("{}", "─".repeat(40));
            println!("Path:        {}", requirements_path.display());
            println!("Backend:     {}", backend.backend_type());
            println!("Name:        {}", store.name);
            println!("Title:       {}", store.title);
            println!("Description: {}", store.description);
            println!();
            println!("{}", "Statistics".bold());
            println!("{}", "─".repeat(40));
            println!("Requirements: {}", store.requirements.len());
            println!("Users:        {}", store.users.len());
            println!("Features:     {}", store.features.len());
            println!("Baselines:    {}", store.baselines.len());

            match backend.backend_type() {
                BackendType::Sqlite => {
                    println!();
                    println!("{}", "Concurrency Support".bold());
                    println!("{}", "─".repeat(40));
                    println!("Store Version:  {}", store.store_version);
                    println!("WAL Mode:       Enabled (recommended for concurrent access)");
                    println!("Optimistic Locking: Supported");
                }
                BackendType::Postgres => {
                    println!();
                    println!("{}", "Concurrency Support".bold());
                    println!("{}", "─".repeat(40));
                    println!("Store Version:  {}", store.store_version);
                    println!("Connection Pool: r2d2 (max 10 connections)");
                    println!("Optimistic Locking: Supported");
                    println!("JSONB:          Native PostgreSQL JSON storage");
                }
                BackendType::Yaml => {
                    println!();
                    println!("{}", "Note".bold());
                    println!("{}", "─".repeat(40));
                    println!("Consider migrating to SQLite or PostgreSQL for concurrent access.");
                }
                BackendType::Git => {
                    println!();
                    println!("{}", "Distributed Storage".bold());
                    println!("{}", "─".repeat(40));
                    println!("Backend:        Git-backed sharded YAML");
                    println!("Object files:   objects/TYPE/NNN/SPEC-ID.yaml");
                    println!("Sync:           git push/pull");
                }
            }
        }
        DbCommand::Sync { .. } => {
            println!(
                "{} Sync is only available for git-backed stores. Use: aida --file <dir> db sync --pull --push",
                "!".yellow()
            );
        }
        DbCommand::MergeGate => {
            println!(
                "{} Merge gate is only available for git-backed stores. Use: aida --file <dir> db merge-gate",
                "!".yellow()
            );
        }
        DbCommand::ExportGit { output } => {
            let output_path = std::path::PathBuf::from(output);

            // Load from current backend
            let source_backend = aida_core::create_backend(requirements_path, None)?;
            let store = source_backend.load()?;

            // Create target git backend
            let target = aida_core::GitBackend::new(&output_path)?;

            if !aida_core::git_ops::is_git_repo(&output_path) {
                aida_core::git_ops::init(&output_path)?;
                let git_name = aida_core::git_ops::git_config_get("user.name")
                    .unwrap_or_else(|_| "AIDA".to_string());
                let git_email = aida_core::git_ops::git_config_get("user.email")
                    .unwrap_or_else(|_| "aida@localhost".to_string());
                aida_core::git_ops::configure_user(&output_path, &git_name, &git_email)?;
            }

            target.save(&store)?;
            println!(
                "{} Exported {} requirements to git store at {}",
                "✓".green(),
                store.requirements.len(),
                output_path.display()
            );
        }
    }

    Ok(())
}

fn handle_export_command(
    storage: &Storage,
    format: &str,
    output: Option<&std::path::Path>,
    id: Option<&str>,
) -> Result<()> {
    // Load requirements
    let store = storage.load()?;

    match format {
        "mapping" => {
            let output_path = output
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from(".requirements-mapping.yaml"));
            export::generate_mapping_file(&store, &output_path)?;
        }
        "json" => {
            let output_path = output
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("requirements.json"));
            export::export_json(&store, &output_path)?;
        }
        "spec" | "requirements" => {
            let output_path = output
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("REQUIREMENTS.md"));
            export::export_requirements_spec(&store, &output_path)?;
        }
        "impl" | "implementation" => {
            let output_path = output
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("IMPLEMENTATION.md"));
            export::export_implementation_records(&store, &output_path)?;
        }
        "tree" => {
            let root_id = id.ok_or_else(|| {
                anyhow::anyhow!("Tree export requires --id to specify the root requirement")
            })?;
            let output_path = output
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("tree-export.json"));
            export::export_tree_to_file(&store, root_id, &output_path)?;
            println!(
                "{}: Exported requirement tree to {}",
                "Success".green(),
                output_path.display()
            );
        }
        _ => {
            anyhow::bail!(
                "Unknown export format: {}. Supported formats: mapping, json, spec, impl, tree",
                format
            );
        }
    }

    Ok(())
}

fn handle_import_command(
    storage: &Storage,
    file: &std::path::Path,
    parent_id: Option<&str>,
    on_conflict: &str,
) -> Result<()> {
    use export::{ConflictStrategy, TreeImportOptions};

    // Parse conflict strategy
    let conflict_strategy = match on_conflict.to_lowercase().as_str() {
        "skip" => ConflictStrategy::Skip,
        "rename" => ConflictStrategy::Rename,
        "replace" => ConflictStrategy::Replace,
        _ => {
            anyhow::bail!(
                "Unknown conflict strategy: {}. Supported: skip, rename, replace",
                on_conflict
            );
        }
    };

    // Load current store
    let mut store = storage.load()?;

    // Setup import options
    let options = TreeImportOptions {
        parent_id: parent_id.map(|s| s.to_string()),
        conflict_strategy,
        created_by: Some(get_default_author()),
    };

    // Perform import
    let result = export::import_tree_from_file(&mut store, file, options)?;

    // Save the updated store
    storage.save(&store)?;

    // Print results
    println!("{}: Import completed", "Success".green());
    println!("  Imported: {} requirements", result.imported_count);
    println!("  Skipped:  {} requirements", result.skipped_count);

    if !result.unresolved_refs.is_empty() {
        println!(
            "  {}",
            format!(
                "Unresolved external references: {}",
                result.unresolved_refs.len()
            )
            .yellow()
        );
        for ext_ref in &result.unresolved_refs {
            if let Some(ref spec_id) = ext_ref.original_target_spec_id {
                println!(
                    "    - {} -> {} ({})",
                    spec_id, ext_ref.original_target_uuid, ext_ref.rel_type
                );
            } else {
                println!(
                    "    - {} ({})",
                    ext_ref.original_target_uuid, ext_ref.rel_type
                );
            }
        }
    }

    Ok(())
}

fn handle_relationship_command(cmd: &RelationshipCommand, storage: &Storage) -> Result<()> {
    match cmd {
        RelationshipCommand::Add {
            from,
            to,
            r#type,
            bidirectional,
        } => {
            add_relationship(storage, from, to, r#type, *bidirectional)?;
        }
        RelationshipCommand::Remove {
            from,
            to,
            r#type,
            bidirectional,
        } => {
            remove_relationship(storage, from, to, r#type, *bidirectional)?;
        }
        RelationshipCommand::List { id } => {
            list_relationships(storage, id)?;
        }
    }
    Ok(())
}

fn add_relationship(
    storage: &Storage,
    from_str: &str,
    to_str: &str,
    rel_type_str: &str,
    bidirectional: bool,
) -> Result<()> {
    // Load requirements
    let mut store = storage.load()?;

    // Parse source and target IDs
    let from_id = parse_requirement_id(from_str, &store)?;
    let to_id = parse_requirement_id(to_str, &store)?;

    // Parse relationship type
    let rel_type = RelationshipType::from_str(rel_type_str);

    // Get requirement info for display (clone the data we need)
    let from_req = store
        .get_requirement_by_id(&from_id)
        .ok_or_else(|| anyhow::anyhow!("Source requirement not found"))?;
    let to_req = store
        .get_requirement_by_id(&to_id)
        .ok_or_else(|| anyhow::anyhow!("Target requirement not found"))?;

    let from_spec = from_req
        .spec_id
        .clone()
        .unwrap_or_else(|| "N/A".to_string());
    let from_title = from_req.title.clone();
    let to_spec = to_req.spec_id.clone().unwrap_or_else(|| "N/A".to_string());
    let to_title = to_req.title.clone();

    // Add the relationship
    store.add_relationship(&from_id, rel_type.clone(), &to_id, bidirectional)?;

    // Save
    storage.save(&store)?;

    println!("{}", "Relationship added successfully!".green());
    println!(
        "  {} ({}) {} {} ({})",
        from_spec,
        from_title,
        "->".blue(),
        to_spec,
        to_title
    );
    println!("  Relationship: {}", rel_type.to_string().cyan());

    if bidirectional {
        if let Some(inverse) = rel_type.inverse() {
            println!("  {} (bidirectional)", inverse.to_string().cyan());
        }
    }

    Ok(())
}

fn remove_relationship(
    storage: &Storage,
    from_str: &str,
    to_str: &str,
    rel_type_str: &str,
    bidirectional: bool,
) -> Result<()> {
    // Load requirements
    let mut store = storage.load()?;

    // Parse source and target IDs
    let from_id = parse_requirement_id(from_str, &store)?;
    let to_id = parse_requirement_id(to_str, &store)?;

    // Parse relationship type
    let rel_type = RelationshipType::from_str(rel_type_str);

    // Remove the relationship
    store.remove_relationship(&from_id, &rel_type, &to_id, bidirectional)?;

    // Save
    storage.save(&store)?;

    println!("{}", "Relationship removed successfully!".green());
    println!("  Relationship: {}", rel_type.to_string().cyan());

    if bidirectional {
        if let Some(inverse) = rel_type.inverse() {
            println!("  {} (bidirectional)", inverse.to_string().cyan());
        }
    }

    Ok(())
}

fn list_relationships(storage: &Storage, id_str: &str) -> Result<()> {
    // Load requirements
    let store = storage.load()?;

    // Parse ID
    let id = parse_requirement_id(id_str, &store)?;

    // Get requirement
    let req = store
        .get_requirement_by_id(&id)
        .ok_or_else(|| anyhow::anyhow!("Requirement not found"))?;

    println!("{}: {}", "Requirement".blue(), req.title);
    if let Some(spec_id) = &req.spec_id {
        println!("{}: {}", "SPEC-ID".blue(), spec_id);
    }
    println!("{}: {}", "UUID".blue(), req.id);
    println!();

    if req.relationships.is_empty() {
        println!("{}", "No relationships found.".yellow());
        return Ok(());
    }

    println!("{}:", "Relationships".green());
    for relationship in &req.relationships {
        let target = store.get_requirement_by_id(&relationship.target_id);
        if let Some(target_req) = target {
            let target_spec = target_req.spec_id.as_deref().unwrap_or("N/A");

            // Format the relationship description based on type
            let description = match &relationship.rel_type {
                RelationshipType::Parent => format!("is parent of"),
                RelationshipType::Child => format!("is child of"),
                RelationshipType::Duplicate => format!("is duplicate of"),
                RelationshipType::Verifies => format!("verifies"),
                RelationshipType::VerifiedBy => format!("is verified by"),
                RelationshipType::References => format!("references"),
                RelationshipType::Custom(name) => format!("{}", name),
            };

            println!(
                "  {} {} ({}) - {}",
                description.cyan(),
                target_spec.yellow(),
                target_req.id.to_string().dimmed(),
                target_req.title
            );
        } else {
            println!(
                "  {} {} {}",
                relationship.rel_type.to_string().cyan(),
                relationship.target_id.to_string().yellow(),
                "(requirement not found)".red()
            );
        }
    }

    Ok(())
}

fn handle_comment_command(cmd: &CommentCommand, storage: &Storage) -> Result<()> {
    match cmd {
        CommentCommand::Add {
            id,
            content,
            content_positional,
            author,
            parent,
            interactive,
        } => {
            // Use --content flag if provided, otherwise use positional argument
            let effective_content = content.as_ref().or(content_positional.as_ref());
            if *interactive || effective_content.is_none() {
                add_comment_interactive(storage, id, author.as_deref(), parent.as_deref())?;
            } else {
                add_comment_cli(
                    storage,
                    id,
                    effective_content.unwrap(),
                    author.as_deref(),
                    parent.as_deref(),
                )?;
            }
        }
        CommentCommand::List { id } => {
            list_comments(storage, id)?;
        }
        CommentCommand::Edit {
            req_id,
            comment_id,
            content,
            interactive,
        } => {
            if *interactive || content.is_none() {
                edit_comment_interactive(storage, req_id, comment_id)?;
            } else {
                edit_comment_cli(storage, req_id, comment_id, content.as_ref().unwrap())?;
            }
        }
        CommentCommand::Delete { req_id, comment_id } => {
            delete_comment(storage, req_id, comment_id)?;
        }
    }
    Ok(())
}

fn add_comment_interactive(
    storage: &Storage,
    req_id: &str,
    author: Option<&str>,
    parent_id: Option<&str>,
) -> Result<()> {
    let mut store = storage.load()?;
    let id = parse_requirement_id(req_id, &store)?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == id)
        .context("Requirement not found")?;

    let author = if let Some(a) = author {
        a.to_string()
    } else {
        let default_author = get_default_author();
        inquire::Text::new("Author:")
            .with_default(&default_author)
            .prompt()?
    };

    let content = inquire::Editor::new("Comment content:").prompt()?;

    let comment = if let Some(parent_str) = parent_id {
        let parent_uuid = Uuid::parse_str(parent_str).context("Invalid parent comment ID")?;
        Comment::new_reply(author, content, parent_uuid)
    } else {
        Comment::new(author, content)
    };

    if let Some(parent_str) = parent_id {
        let parent_uuid = Uuid::parse_str(parent_str)?;
        req.add_reply(parent_uuid, comment)?;
    } else {
        req.add_comment(comment);
    }

    storage.save(&store)?;
    println!("{}", "Comment added successfully".green());
    Ok(())
}

fn add_comment_cli(
    storage: &Storage,
    req_id: &str,
    content: &str,
    author: Option<&str>,
    parent_id: Option<&str>,
) -> Result<()> {
    let mut store = storage.load()?;
    let id = parse_requirement_id(req_id, &store)?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == id)
        .context("Requirement not found")?;

    let author = author
        .map(|a| a.to_string())
        .unwrap_or_else(get_default_author);

    let comment = if let Some(parent_str) = parent_id {
        let parent_uuid = Uuid::parse_str(parent_str).context("Invalid parent comment ID")?;
        Comment::new_reply(author, content.to_string(), parent_uuid)
    } else {
        Comment::new(author, content.to_string())
    };

    if let Some(parent_str) = parent_id {
        let parent_uuid = Uuid::parse_str(parent_str)?;
        req.add_reply(parent_uuid, comment)?;
    } else {
        req.add_comment(comment);
    }

    storage.save(&store)?;
    println!("{}", "Comment added successfully".green());
    Ok(())
}

fn list_comments(storage: &Storage, req_id: &str) -> Result<()> {
    let store = storage.load()?;
    let id = parse_requirement_id(req_id, &store)?;

    let req = store
        .requirements
        .iter()
        .find(|r| r.id == id)
        .context("Requirement not found")?;

    println!("{}: {}", "Requirement".cyan(), req.title);
    println!();

    if req.comments.is_empty() {
        println!("{}", "No comments yet".dimmed());
        return Ok(());
    }

    println!("{}:", "Comments".green().bold());
    for comment in &req.comments {
        print_comment(comment, 0);
    }

    Ok(())
}

fn print_comment(comment: &Comment, indent: usize) {
    let indent_str = "  ".repeat(indent);
    println!();
    println!("{}{}:", indent_str, comment.id.to_string().yellow());
    println!(
        "{}  {} {} at {}",
        indent_str,
        "By:".dimmed(),
        comment.author.cyan(),
        comment
            .created_at
            .format("%Y-%m-%d %H:%M")
            .to_string()
            .dimmed()
    );
    println!("{}  {}", indent_str, comment.content);

    if !comment.replies.is_empty() {
        for reply in &comment.replies {
            print_comment(reply, indent + 1);
        }
    }
}

fn edit_comment_interactive(storage: &Storage, req_id: &str, comment_id: &str) -> Result<()> {
    let mut store = storage.load()?;
    let req_uuid = parse_requirement_id(req_id, &store)?;
    let comment_uuid = Uuid::parse_str(comment_id).context("Invalid comment ID")?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == req_uuid)
        .context("Requirement not found")?;

    let comment = req
        .find_comment_mut(&comment_uuid)
        .context("Comment not found")?;

    let new_content = inquire::Editor::new("Comment content:")
        .with_predefined_text(&comment.content)
        .prompt()?;

    comment.content = new_content;
    comment.touch();

    storage.save(&store)?;
    println!("{}", "Comment updated successfully".green());
    Ok(())
}

fn edit_comment_cli(
    storage: &Storage,
    req_id: &str,
    comment_id: &str,
    content: &str,
) -> Result<()> {
    let mut store = storage.load()?;
    let req_uuid = parse_requirement_id(req_id, &store)?;
    let comment_uuid = Uuid::parse_str(comment_id).context("Invalid comment ID")?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == req_uuid)
        .context("Requirement not found")?;

    let comment = req
        .find_comment_mut(&comment_uuid)
        .context("Comment not found")?;

    comment.content = content.to_string();
    comment.touch();

    storage.save(&store)?;
    println!("{}", "Comment updated successfully".green());
    Ok(())
}

fn delete_comment(storage: &Storage, req_id: &str, comment_id: &str) -> Result<()> {
    let mut store = storage.load()?;
    let req_uuid = parse_requirement_id(req_id, &store)?;
    let comment_uuid = Uuid::parse_str(comment_id).context("Invalid comment ID")?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == req_uuid)
        .context("Requirement not found")?;

    req.delete_comment(&comment_uuid)?;

    storage.save(&store)?;
    println!("{}", "Comment deleted successfully".green());
    Ok(())
}

fn open_user_guide(dark_mode: bool) -> Result<()> {
    // Get the path to the docs directory relative to the executable
    let exe_path = std::env::current_exe().context("Failed to get executable path")?;

    // Try multiple possible locations for the docs
    let possible_paths = [
        // Relative to executable (for installed binaries)
        exe_path.parent().unwrap().join("../docs"),
        exe_path.parent().unwrap().join("../../docs"),
        // Development paths
        exe_path.parent().unwrap().join("../../../docs"),
        exe_path.parent().unwrap().join("../../../../docs"),
        // Current directory
        std::env::current_dir().unwrap_or_default().join("docs"),
        // Project root (when running from project directory)
        std::path::PathBuf::from("docs"),
    ];

    let filename = if dark_mode {
        "user-guide-dark.html"
    } else {
        "user-guide.html"
    };

    // Find the first path that exists
    let doc_path = possible_paths
        .iter()
        .map(|p| p.join(filename))
        .find(|p| p.exists());

    match doc_path {
        Some(path) => {
            let path_str = path
                .canonicalize()
                .unwrap_or(path.clone())
                .to_string_lossy()
                .to_string();

            // Convert to file:// URL
            let url = format!("file://{}", path_str);

            println!(
                "Opening user guide{}...",
                if dark_mode { " (dark mode)" } else { "" }
            );

            // Try to open in browser using platform-specific commands
            #[cfg(target_os = "linux")]
            {
                std::process::Command::new("xdg-open")
                    .arg(&url)
                    .spawn()
                    .context("Failed to open browser. Try opening manually: {}")?;
            }

            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("open")
                    .arg(&url)
                    .spawn()
                    .context("Failed to open browser")?;
            }

            #[cfg(target_os = "windows")]
            {
                std::process::Command::new("cmd")
                    .args(["/C", "start", &url])
                    .spawn()
                    .context("Failed to open browser")?;
            }

            println!("{}", "User guide opened in browser".green());
            Ok(())
        }
        None => {
            println!("{}", "User guide not found.".yellow());
            println!("Expected location: docs/{}", filename);
            println!("\nTo generate the documentation, run:");
            println!("  ./helper/generate-docs.sh");
            anyhow::bail!("User guide not found")
        }
    }
}

// ============================================================================
// Relationship Definition Command Handlers
// ============================================================================

fn handle_rel_def_command(cmd: &RelDefCommand, storage: &Storage) -> Result<()> {
    match cmd {
        RelDefCommand::List => {
            list_relationship_definitions(storage)?;
        }
        RelDefCommand::Show { name } => {
            show_relationship_definition(storage, name)?;
        }
        RelDefCommand::Add {
            name,
            display_name,
            description,
            inverse,
            symmetric,
            cardinality,
            source_types,
            target_types,
            color,
        } => {
            add_relationship_definition(
                storage,
                name,
                display_name.as_deref(),
                description.as_deref(),
                inverse.as_deref(),
                *symmetric,
                cardinality,
                source_types.as_deref(),
                target_types.as_deref(),
                color.as_deref(),
            )?;
        }
        RelDefCommand::Edit {
            name,
            display_name,
            description,
            source_types,
            target_types,
            color,
        } => {
            edit_relationship_definition(
                storage,
                name,
                display_name.as_deref(),
                description.as_deref(),
                source_types.as_deref(),
                target_types.as_deref(),
                color.as_deref(),
            )?;
        }
        RelDefCommand::Remove { name, yes } => {
            remove_relationship_definition(storage, name, *yes)?;
        }
    }
    Ok(())
}

fn list_relationship_definitions(storage: &Storage) -> Result<()> {
    let store = storage.load()?;

    println!("{}", "Relationship Definitions".cyan().bold());
    println!("{}", "=".repeat(60));

    for def in store.get_relationship_definitions() {
        let built_in_marker = if def.built_in {
            " [built-in]".dimmed()
        } else {
            "".normal()
        };
        println!(
            "\n{}{} ({})",
            def.display_name.green().bold(),
            built_in_marker,
            def.name.dimmed()
        );

        if !def.description.is_empty() {
            println!("  {}", def.description);
        }

        // Show inverse/symmetric
        if def.symmetric {
            println!("  {} symmetric", "↔".cyan());
        } else if let Some(ref inverse) = def.inverse {
            println!("  {} inverse: {}", "↔".cyan(), inverse.yellow());
        }

        // Show cardinality
        println!("  {} cardinality: {}", "⊛".cyan(), def.cardinality);

        // Show type constraints
        if !def.source_types.is_empty() {
            println!(
                "  {} source types: {}",
                "→".cyan(),
                def.source_types.join(", ")
            );
        }
        if !def.target_types.is_empty() {
            println!(
                "  {} target types: {}",
                "←".cyan(),
                def.target_types.join(", ")
            );
        }

        // Show color if set
        if let Some(ref color) = def.color {
            println!("  {} color: {}", "●".cyan(), color);
        }
    }

    println!(
        "\n{} relationship definitions total",
        store.get_relationship_definitions().len()
    );
    Ok(())
}

fn show_relationship_definition(storage: &Storage, name: &str) -> Result<()> {
    let store = storage.load()?;

    let def = store
        .get_relationship_definition(name)
        .ok_or_else(|| anyhow::anyhow!("Relationship definition '{}' not found", name))?;

    println!("{}", "Relationship Definition".cyan().bold());
    println!("{}", "=".repeat(40));

    println!("{}: {}", "Name".bold(), def.name);
    println!("{}: {}", "Display Name".bold(), def.display_name);
    println!(
        "{}: {}",
        "Description".bold(),
        if def.description.is_empty() {
            "(none)"
        } else {
            &def.description
        }
    );
    println!(
        "{}: {}",
        "Built-in".bold(),
        if def.built_in { "Yes" } else { "No" }
    );
    println!(
        "{}: {}",
        "Symmetric".bold(),
        if def.symmetric { "Yes" } else { "No" }
    );

    if let Some(ref inverse) = def.inverse {
        println!("{}: {}", "Inverse".bold(), inverse);
    }

    println!("{}: {}", "Cardinality".bold(), def.cardinality);

    if def.source_types.is_empty() {
        println!("{}: (all types)", "Source Types".bold());
    } else {
        println!("{}: {}", "Source Types".bold(), def.source_types.join(", "));
    }

    if def.target_types.is_empty() {
        println!("{}: (all types)", "Target Types".bold());
    } else {
        println!("{}: {}", "Target Types".bold(), def.target_types.join(", "));
    }

    if let Some(ref color) = def.color {
        println!("{}: {}", "Color".bold(), color);
    }

    Ok(())
}

fn add_relationship_definition(
    storage: &Storage,
    name: &str,
    display_name: Option<&str>,
    description: Option<&str>,
    inverse: Option<&str>,
    symmetric: bool,
    cardinality: &str,
    source_types: Option<&str>,
    target_types: Option<&str>,
    color: Option<&str>,
) -> Result<()> {
    let mut store = storage.load()?;

    // Parse source/target types
    let source_type_vec: Vec<String> = source_types
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let target_type_vec: Vec<String> = target_types
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Create the definition
    let mut def = RelationshipDefinition::new(name, display_name.unwrap_or(name));

    if let Some(desc) = description {
        def.description = desc.to_string();
    }

    if let Some(inv) = inverse {
        def.inverse = Some(inv.to_lowercase());
    }

    def.symmetric = symmetric;
    def.cardinality = Cardinality::from_str(cardinality);
    def.source_types = source_type_vec;
    def.target_types = target_type_vec;

    if let Some(c) = color {
        def.color = Some(c.to_string());
    }

    store.add_relationship_definition(def)?;
    storage.save(&store)?;

    println!("{} Added relationship definition '{}'", "✓".green(), name);
    Ok(())
}

fn edit_relationship_definition(
    storage: &Storage,
    name: &str,
    display_name: Option<&str>,
    description: Option<&str>,
    source_types: Option<&str>,
    target_types: Option<&str>,
    color: Option<&str>,
) -> Result<()> {
    let mut store = storage.load()?;

    // Get the existing definition
    let existing = store
        .get_relationship_definition(name)
        .ok_or_else(|| anyhow::anyhow!("Relationship definition '{}' not found", name))?
        .clone();

    // Build updated definition
    let mut updated = existing.clone();

    if let Some(dn) = display_name {
        updated.display_name = dn.to_string();
    }

    if let Some(desc) = description {
        updated.description = desc.to_string();
    }

    if let Some(st) = source_types {
        updated.source_types = st
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
    }

    if let Some(tt) = target_types {
        updated.target_types = tt
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
    }

    if let Some(c) = color {
        updated.color = if c.is_empty() {
            None
        } else {
            Some(c.to_string())
        };
    }

    store.update_relationship_definition(name, updated)?;
    storage.save(&store)?;

    if existing.built_in {
        println!(
            "{} Updated built-in relationship definition '{}' (limited fields)",
            "✓".green(),
            name
        );
    } else {
        println!("{} Updated relationship definition '{}'", "✓".green(), name);
    }
    Ok(())
}

fn remove_relationship_definition(
    storage: &Storage,
    name: &str,
    skip_confirmation: bool,
) -> Result<()> {
    let mut store = storage.load()?;

    // Check if it exists and is not built-in
    let def = store
        .get_relationship_definition(name)
        .ok_or_else(|| anyhow::anyhow!("Relationship definition '{}' not found", name))?;

    if def.built_in {
        anyhow::bail!("Cannot remove built-in relationship definition '{}'", name);
    }

    // Confirm deletion
    if !skip_confirmation {
        println!(
            "Are you sure you want to remove relationship definition '{}'?",
            name
        );
        println!(
            "This will not affect existing relationships, but they will become 'custom' type."
        );

        let confirm = inquire::Confirm::new("Delete?")
            .with_default(false)
            .prompt()?;

        if !confirm {
            println!("{}", "Cancelled".yellow());
            return Ok(());
        }
    }

    store.remove_relationship_definition(name)?;
    storage.save(&store)?;

    println!("{} Removed relationship definition '{}'", "✓".green(), name);
    Ok(())
}

// ============================================================================
// Trace Command Handlers
// ============================================================================

// trace:REQ-0243 | ai:claude:high
fn handle_trace_command(cmd: &TraceCommand, storage: &Storage) -> Result<()> {
    match cmd {
        TraceCommand::Add {
            req,
            file,
            symbol,
            line_start,
            line_end,
            r#type,
            notes,
            commit,
        } => {
            trace_add(
                storage,
                req,
                file,
                symbol.as_deref(),
                *line_start,
                *line_end,
                r#type,
                notes.as_deref(),
                commit.as_deref(),
            )?;
        }
        TraceCommand::List { req, file } => {
            trace_list(storage, req.as_deref(), file.as_deref())?;
        }
        TraceCommand::Remove { req, link_id } => {
            trace_remove(storage, req, link_id)?;
        }
        TraceCommand::Scan {
            path,
            extensions,
            update,
            verbose,
        } => {
            trace_scan(storage, path.as_deref(), extensions, *update, *verbose)?;
        }
        TraceCommand::Sweep {
            limit,
            branch,
            dry_run,
            verbose,
        } => {
            trace_sweep(storage, *limit, branch.as_deref(), *dry_run, *verbose)?;
        }
    }
    Ok(())
}

// trace:REQ-0243 | ai:claude:high
fn trace_add(
    storage: &Storage,
    req_id: &str,
    file_path: &str,
    symbol: Option<&str>,
    line_start: Option<u32>,
    line_end: Option<u32>,
    artifact_type: &str,
    notes: Option<&str>,
    commit: Option<&str>,
) -> Result<()> {
    let mut store = storage.load()?;
    let id = parse_requirement_id(req_id, &store)?;

    // Parse artifact type
    let art_type = match artifact_type.to_lowercase().as_str() {
        "source" | "sourcecode" | "src" => ArtifactType::SourceCode,
        "test" | "testcode" => ArtifactType::TestCode,
        "config" | "cfg" => ArtifactType::Config,
        "doc" | "documentation" => ArtifactType::Doc,
        _ => {
            anyhow::bail!(
                "Invalid artifact type '{}'. Use: source, test, config, doc",
                artifact_type
            )
        }
    };

    // Create trace link
    let mut trace = TraceLink::new(art_type, file_path.to_string());

    if let Some(sym) = symbol {
        trace.symbol = Some(sym.to_string());
    }

    trace.line_start = line_start;
    trace.line_end = line_end;

    if let Some(n) = notes {
        trace.notes = Some(n.to_string());
    }

    if let Some(c) = commit {
        trace.commit_hash = Some(c.to_string());
    }

    // Get current user
    trace.created_by = Some(
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "Unknown".to_string()),
    );

    // Find requirement and add trace link
    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == id)
        .context("Requirement not found")?;

    let spec_id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
    req.trace_links.push(trace.clone());

    storage.save(&store)?;

    println!("{} Added trace link to {}", "✓".green(), spec_id);
    println!("  File: {}", file_path.cyan());
    if let Some(sym) = symbol {
        println!("  Symbol: {}", sym.yellow());
    }
    if let Some(start) = line_start {
        if let Some(end) = line_end {
            println!("  Lines: {}-{}", start, end);
        } else {
            println!("  Line: {}", start);
        }
    }
    println!("  Type: {:?}", trace.artifact_type);

    Ok(())
}

// trace:REQ-0244 | ai:claude:high
fn trace_list(storage: &Storage, req_id: Option<&str>, file_path: Option<&str>) -> Result<()> {
    let store = storage.load()?;

    if req_id.is_none() && file_path.is_none() {
        // List all trace links across all requirements
        println!("{}", "All Trace Links".cyan().bold());
        println!("{}", "=".repeat(80));

        let mut total = 0;
        for req in &store.requirements {
            if !req.trace_links.is_empty() {
                let spec_id = req.spec_id.as_deref().unwrap_or("N/A");
                println!("\n{} - {}", spec_id.yellow(), req.title);

                for trace in &req.trace_links {
                    print_trace_link(trace, "  ");
                    total += 1;
                }
            }
        }

        if total == 0 {
            println!("{}", "No trace links found.".yellow());
        } else {
            println!("\n{} trace links total", total);
        }
        return Ok(());
    }

    if let Some(req_id_str) = req_id {
        // List trace links for a specific requirement
        let id = parse_requirement_id(req_id_str, &store)?;
        let req = store
            .get_requirement_by_id(&id)
            .context("Requirement not found")?;

        let spec_id = req.spec_id.as_deref().unwrap_or("N/A");
        println!("{}: {} - {}", "Requirement".blue(), spec_id, req.title);
        println!();

        if req.trace_links.is_empty() {
            println!("{}", "No trace links found.".yellow());
        } else {
            println!("{}:", "Trace Links".green());
            for trace in &req.trace_links {
                print_trace_link(trace, "  ");
            }
            println!("\n{} trace links total", req.trace_links.len());
        }
    }

    if let Some(file) = file_path {
        // List trace links for a specific file
        println!("{}: {}", "File".blue(), file);
        println!();

        let mut found = Vec::new();
        for req in &store.requirements {
            for trace in &req.trace_links {
                if trace.file_path.contains(file) {
                    found.push((req, trace));
                }
            }
        }

        if found.is_empty() {
            println!("{}", "No trace links found for this file.".yellow());
        } else {
            println!("{}:", "Trace Links".green());
            for (req, trace) in &found {
                let spec_id = req.spec_id.as_deref().unwrap_or("N/A");
                println!("  {} -> {}", spec_id.yellow(), trace.file_path.cyan());
                if let Some(sym) = &trace.symbol {
                    println!("    Symbol: {}", sym);
                }
                if let Some(start) = trace.line_start {
                    if let Some(end) = trace.line_end {
                        println!("    Lines: {}-{}", start, end);
                    } else {
                        println!("    Line: {}", start);
                    }
                }
            }
            println!("\n{} trace links total", found.len());
        }
    }

    Ok(())
}

fn print_trace_link(trace: &TraceLink, indent: &str) {
    println!("{}ID: {}", indent, trace.id.to_string().dimmed());
    println!("{}File: {}", indent, trace.file_path.cyan());
    if let Some(sym) = &trace.symbol {
        println!("{}Symbol: {}", indent, sym.yellow());
    }
    if let Some(start) = trace.line_start {
        if let Some(end) = trace.line_end {
            println!("{}Lines: {}-{}", indent, start, end);
        } else {
            println!("{}Line: {}", indent, start);
        }
    }
    println!("{}Type: {:?}", indent, trace.artifact_type);
    if let Some(notes) = &trace.notes {
        println!("{}Notes: {}", indent, notes);
    }
    if let Some(commit) = &trace.commit_hash {
        println!("{}Commit: {}", indent, commit);
    }
    if let Some(created_by) = &trace.created_by {
        println!(
            "{}Created: {} by {}",
            indent,
            trace.created_at.format("%Y-%m-%d %H:%M"),
            created_by
        );
    }
    println!();
}

fn trace_remove(storage: &Storage, req_id: &str, link_id: &str) -> Result<()> {
    let mut store = storage.load()?;
    let id = parse_requirement_id(req_id, &store)?;
    let link_uuid = Uuid::parse_str(link_id).context("Invalid trace link ID")?;

    let req = store
        .requirements
        .iter_mut()
        .find(|r| r.id == id)
        .context("Requirement not found")?;

    let initial_len = req.trace_links.len();
    req.trace_links.retain(|t| t.id != link_uuid);

    if req.trace_links.len() == initial_len {
        anyhow::bail!("Trace link not found");
    }

    storage.save(&store)?;
    println!("{} Trace link removed", "✓".green());
    Ok(())
}

// trace:REQ-0245 | ai:claude:high
fn trace_scan(
    storage: &Storage,
    path: Option<&str>,
    extensions: &str,
    update: bool,
    verbose: bool,
) -> Result<()> {
    use std::fs;
    use std::io::BufRead;

    let scan_path = path.unwrap_or(".");
    let ext_list: Vec<&str> = extensions.split(',').map(|s| s.trim()).collect();

    println!(
        "Scanning {} for trace comments (extensions: {})...",
        scan_path.cyan(),
        extensions
    );

    // Regex pattern for trace comments (supports both old and new formats):
    // Old: // trace:REQ-ID | ai:tool:confidence
    // New: // trace:REQ-ID - Title | ai:tool:confidence | impl:date | by:user
    let trace_pattern = regex::Regex::new(
        r"//\s*trace:([A-Z]+-\d+)(?:\s*-\s*([^|]+))?(?:\s*\|\s*ai:(\w+):(\w+))?(?:\s*\|\s*impl:(\S+))?(?:\s*\|\s*by:(\S+))?"
    ).unwrap();

    // (req_id, file_path, line_content, line_num, tool, confidence, title, impl_date, by_user)
    let mut found_traces: Vec<(
        String,
        String,
        String,
        u32,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = Vec::new();

    // Walk through files
    fn scan_dir(
        dir: &std::path::Path,
        ext_list: &[&str],
        pattern: &regex::Regex,
        found: &mut Vec<(
            String,
            String,
            String,
            u32,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        )>,
        verbose: bool,
    ) -> Result<()> {
        if dir.is_file() {
            if let Some(ext) = dir.extension() {
                if ext_list.contains(&ext.to_str().unwrap_or("")) {
                    scan_file(dir, pattern, found, verbose)?;
                }
            }
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            // Skip hidden directories and common non-source directories
            if let Some(name) = path.file_name() {
                let name_str = name.to_string_lossy();
                if name_str.starts_with('.')
                    || name_str == "target"
                    || name_str == "node_modules"
                    || name_str == "vendor"
                {
                    continue;
                }
            }

            if path.is_dir() {
                scan_dir(&path, ext_list, pattern, found, verbose)?;
            } else if let Some(ext) = path.extension() {
                if ext_list.contains(&ext.to_str().unwrap_or("")) {
                    scan_file(&path, pattern, found, verbose)?;
                }
            }
        }
        Ok(())
    }

    fn scan_file(
        path: &std::path::Path,
        pattern: &regex::Regex,
        found: &mut Vec<(
            String,
            String,
            String,
            u32,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        )>,
        verbose: bool,
    ) -> Result<()> {
        let file = fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);

        for (line_num, line) in reader.lines().enumerate() {
            let line = line?;
            if let Some(caps) = pattern.captures(&line) {
                let req_id = caps.get(1).map(|m| m.as_str().to_string()).unwrap();
                let title = caps.get(2).map(|m| m.as_str().trim().to_string());
                let tool = caps.get(3).map(|m| m.as_str().to_string());
                let confidence = caps.get(4).map(|m| m.as_str().to_string());
                let impl_date = caps.get(5).map(|m| m.as_str().to_string());
                let by_user = caps.get(6).map(|m| m.as_str().to_string());

                if verbose {
                    let title_str = title.as_deref().unwrap_or("");
                    println!(
                        "  Found: {}{} in {}:{}",
                        req_id.yellow(),
                        if title_str.is_empty() {
                            "".to_string()
                        } else {
                            format!(" - {}", title_str)
                        },
                        path.display(),
                        line_num + 1
                    );
                }

                found.push((
                    req_id,
                    path.to_string_lossy().to_string(),
                    line.clone(),
                    (line_num + 1) as u32,
                    tool,
                    confidence,
                    title,
                    impl_date,
                    by_user,
                ));
            }
        }
        Ok(())
    }

    scan_dir(
        std::path::Path::new(scan_path),
        &ext_list,
        &trace_pattern,
        &mut found_traces,
        verbose,
    )?;

    println!("\nFound {} trace comments:", found_traces.len());

    // Group by requirement
    let mut by_req: std::collections::HashMap<String, Vec<_>> = std::collections::HashMap::new();
    for trace in &found_traces {
        by_req
            .entry(trace.0.clone())
            .or_default()
            .push(trace.clone());
    }

    for (req_id, traces) in &by_req {
        println!("\n  {} ({} links):", req_id.yellow(), traces.len());
        for (_, file, _, line, tool, conf, title, impl_date, by_user) in traces {
            let title_info = title
                .as_ref()
                .map(|t| format!(" - {}", t))
                .unwrap_or_default();
            let ai_info = match (tool, conf) {
                (Some(t), Some(c)) => format!(" [ai:{t}:{c}]").dimmed().to_string(),
                _ => String::new(),
            };
            let impl_info = impl_date
                .as_ref()
                .map(|d| format!(" impl:{}", d))
                .unwrap_or_default();
            let by_info = by_user
                .as_ref()
                .map(|u| format!(" by:{}", u))
                .unwrap_or_default();
            println!(
                "    {}:{}{}{}{}{}",
                file.cyan(),
                line,
                title_info,
                ai_info,
                impl_info.dimmed(),
                by_info.dimmed()
            );
        }
    }

    if update {
        println!("\n{} Updating requirements database...", "→".blue());
        let mut store = storage.load()?;
        let mut added = 0;

        for (req_id, file_path, _, line_num, tool, confidence, _title, impl_date, by_user) in
            found_traces
        {
            // Find requirement by spec_id
            if let Some(req) = store
                .requirements
                .iter_mut()
                .find(|r| r.spec_id.as_deref() == Some(&req_id))
            {
                // Check if trace link already exists for this file and line
                let exists = req
                    .trace_links
                    .iter()
                    .any(|t| t.file_path == file_path && t.line_start == Some(line_num));

                if !exists {
                    let mut trace = TraceLink::new(ArtifactType::SourceCode, file_path.clone());
                    trace.line_start = Some(line_num);

                    // Use by_user from comment if present, otherwise default to "scan"
                    trace.created_by = by_user.or_else(|| Some("scan".to_string()));

                    // Build notes from available info
                    let mut notes_parts = Vec::new();
                    if let Some(t) = &tool {
                        let conf_str = confidence
                            .as_ref()
                            .map(|c| format!(":{}", c))
                            .unwrap_or_default();
                        notes_parts.push(format!("AI tool: {}{}", t, conf_str));
                    }
                    if let Some(date) = &impl_date {
                        notes_parts.push(format!("Implemented: {}", date));
                    }
                    if !notes_parts.is_empty() {
                        trace.notes = Some(notes_parts.join(", "));
                    }

                    req.trace_links.push(trace);
                    added += 1;
                }
            } else {
                println!(
                    "  {} Requirement {} not found in database",
                    "!".yellow(),
                    req_id
                );
            }
        }

        storage.save(&store)?;
        println!("{} Added {} new trace links", "✓".green(), added);
    }

    Ok(())
}

// trace:REQ-0258 | ai:claude:high
fn trace_sweep(
    storage: &Storage,
    limit: Option<u32>,
    branch: Option<&str>,
    dry_run: bool,
    verbose: bool,
) -> Result<()> {
    use std::process::Command;

    println!(
        "Sweeping git commits for requirement references{}...",
        if dry_run { " (dry run)" } else { "" }
    );

    // Get git log
    let mut cmd = Command::new("git");
    cmd.arg("log")
        .arg("--pretty=format:%H|%s|%b")
        .arg("--no-merges");

    if let Some(b) = branch {
        cmd.arg(b);
    }

    if let Some(n) = limit {
        cmd.arg(format!("-{}", n));
    }

    let output = cmd.output().context("Failed to run git log")?;

    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let log_output = String::from_utf8_lossy(&output.stdout);

    // Pattern to find requirement references in commit messages
    let req_pattern = regex::Regex::new(r"\b([A-Z]+-\d+)\b").unwrap();

    let mut found_refs: Vec<(String, String, String)> = Vec::new(); // (commit_hash, req_id, subject)

    for entry in log_output.split('\n') {
        if entry.is_empty() {
            continue;
        }

        let parts: Vec<&str> = entry.splitn(3, '|').collect();
        if parts.len() < 2 {
            continue;
        }

        let commit_hash = parts[0];
        let subject = parts[1];
        let body = parts.get(2).unwrap_or(&"");

        // Search for requirement IDs in subject and body
        let full_text = format!("{} {}", subject, body);
        for caps in req_pattern.captures_iter(&full_text) {
            let req_id = caps.get(1).map(|m| m.as_str().to_string()).unwrap();

            if verbose {
                println!(
                    "  Found: {} in commit {} - {}",
                    req_id.yellow(),
                    &commit_hash[..8].cyan(),
                    subject
                );
            }

            found_refs.push((commit_hash.to_string(), req_id, subject.to_string()));
        }
    }

    println!("\nFound {} requirement references:", found_refs.len());

    // Group by requirement
    let mut by_req: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();
    for (hash, req_id, subject) in &found_refs {
        by_req
            .entry(req_id.clone())
            .or_default()
            .push((hash.clone(), subject.clone()));
    }

    for (req_id, commits) in &by_req {
        println!("\n  {} ({} commits):", req_id.yellow(), commits.len());
        for (hash, subject) in commits {
            println!("    {} - {}", hash[..8].cyan(), subject);
        }
    }

    if !dry_run {
        println!("\n{} Updating requirements database...", "→".blue());
        let mut store = storage.load()?;
        let mut updated = 0;

        for (commit_hash, req_id, subject) in found_refs {
            // Find requirement by spec_id
            if let Some(req) = store
                .requirements
                .iter_mut()
                .find(|r| r.spec_id.as_deref() == Some(&req_id))
            {
                // Check if this commit is already linked
                let exists = req
                    .trace_links
                    .iter()
                    .any(|t| t.commit_hash.as_deref() == Some(&commit_hash));

                if !exists {
                    let mut trace = TraceLink::new(ArtifactType::SourceCode, "".to_string());
                    trace.commit_hash = Some(commit_hash.clone());
                    trace.notes = Some(format!("Git commit: {}", subject));
                    trace.created_by = Some("sweep".to_string());
                    req.trace_links.push(trace);
                    updated += 1;
                }
            }
        }

        storage.save(&store)?;
        println!("{} Added {} new commit trace links", "✓".green(), updated);
    }

    Ok(())
}

// trace:FR-0259 | ai:claude:high
fn handle_report_command(cmd: &ReportCommand, storage: &Storage, storage_path: &str) -> Result<()> {
    match cmd {
        ReportCommand::AiIntegration {
            format,
            output,
            project_root,
            include_scaffold,
        } => {
            let store = storage.load()?;

            // Parse format
            let report_format = match format.to_lowercase().as_str() {
                "markdown" | "md" => ReportFormat::Markdown,
                "html" | "htm" => ReportFormat::Html,
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unknown format '{}'. Use 'markdown' or 'html'.",
                        format
                    ))
                }
            };

            // Create report generator
            let mut generator = ReportGenerator::new(store, storage_path.to_string());

            // Set project root if provided or use current directory for scaffold status
            let root = if let Some(ref root) = project_root {
                root.clone()
            } else if *include_scaffold {
                std::env::current_dir()?
            } else {
                // No root needed if not checking scaffold
                std::path::PathBuf::new()
            };

            if *include_scaffold || project_root.is_some() {
                if root.exists() {
                    generator = generator.with_project_root(root.clone());
                }
            }

            // Generate report
            let report = generator.generate();

            // Render based on format
            let content = match report_format {
                ReportFormat::Markdown => generator.render_markdown(&report),
                ReportFormat::Html => generator.render_html(&report),
            };

            // Output
            if let Some(ref output_path) = output {
                std::fs::write(output_path, &content)?;
                println!(
                    "{} Report generated: {}",
                    "✓".green(),
                    output_path.display()
                );
            } else {
                println!("{}", content);
            }
        }
    }

    Ok(())
}

// trace:FR-0260 | ai:claude:high
fn handle_scaffold_command(
    cmd: &ScaffoldCommand,
    storage: &Storage,
    db_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        ScaffoldCommand::Status {
            project_root,
            verbose,
            report,
            output,
        } => {
            let store = storage.load()?;
            let root = project_root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

            if !root.exists() {
                return Err(anyhow::anyhow!(
                    "Project root does not exist: {}",
                    root.display()
                ));
            }

            let config = ScaffoldConfig::default();
            let status = check_scaffold_status(&store, &root, &config, db_path);

            // Generate HTML report if requested
            if *report {
                let html = generate_scaffold_html_report(&store, &root, &config, db_path, &status)?;
                if let Some(output_path) = output {
                    std::fs::write(output_path, &html)?;
                    println!(
                        "{} Scaffold report generated: {}",
                        "✓".green(),
                        output_path.display()
                    );
                } else {
                    println!("{}", html);
                }
                return Ok(());
            }

            if status.is_current {
                println!("{} Scaffold is up to date", "✓".green());
            } else {
                println!("{} Scaffold drift detected", "⚠".yellow());
            }

            println!();
            println!(
                "  {} matching, {} modified, {} missing, {} extra",
                status.matching.len().to_string().green(),
                status.modified.len().to_string().yellow(),
                status.missing.len().to_string().red(),
                status.extra.len().to_string().blue()
            );

            if *verbose {
                if !status.matching.is_empty() {
                    println!();
                    println!("{}:", "Matching".green());
                    for path in &status.matching {
                        println!("  ✓ {}", path.display());
                    }
                }

                if !status.modified.is_empty() {
                    println!();
                    println!("{}:", "Modified".yellow());
                    for (path, file_status) in &status.modified {
                        match file_status {
                            FileStatus::Modified {
                                expected_lines,
                                actual_lines,
                            } => {
                                println!(
                                    "  ~ {} (expected {} lines, found {})",
                                    path.display(),
                                    expected_lines,
                                    actual_lines
                                );
                            }
                            _ => {
                                println!("  ~ {}", path.display());
                            }
                        }
                    }
                }

                if !status.missing.is_empty() {
                    println!();
                    println!("{}:", "Missing".red());
                    for path in &status.missing {
                        println!("  ✗ {}", path.display());
                    }
                }

                if !status.extra.is_empty() {
                    println!();
                    println!("{} (not from scaffold):", "Extra".blue());
                    for path in &status.extra {
                        println!("  + {}", path.display());
                    }
                }
            }
        }

        ScaffoldCommand::Preview { project_root } => {
            let store = storage.load()?;
            let root = project_root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

            let config = ScaffoldConfig::default();
            let mut scaffolder =
                Scaffolder::with_database(root.clone(), config, db_path.to_path_buf());
            let preview = scaffolder.preview(&store);

            println!("{} Scaffold preview for: {}", "📁".blue(), root.display());
            println!();

            for artifact in &preview.artifacts {
                let exists = root.join(&artifact.path).exists();
                let status = if exists { "exists" } else { "new" };
                println!(
                    "  {} {} ({})",
                    if exists { "~" } else { "+" },
                    artifact.path.display(),
                    status
                );
            }

            println!();
            println!(
                "Total: {} files ({} new, {} existing)",
                preview.artifacts.len(),
                preview
                    .artifacts
                    .iter()
                    .filter(|a| !root.join(&a.path).exists())
                    .count(),
                preview
                    .artifacts
                    .iter()
                    .filter(|a| root.join(&a.path).exists())
                    .count()
            );
        }

        ScaffoldCommand::Apply {
            project_root,
            force,
            dry_run,
        } => {
            let store = storage.load()?;
            let root = project_root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

            let config = ScaffoldConfig::default();
            let mut scaffolder =
                Scaffolder::with_database(root.clone(), config, db_path.to_path_buf());

            if *dry_run {
                println!("{} Dry run - no files will be modified", "ℹ".blue());
                println!();
            }

            let preview = scaffolder.preview(&store);

            for artifact in &preview.artifacts {
                let full_path = root.join(&artifact.path);
                let exists = full_path.exists();

                if exists && !force && !dry_run {
                    println!(
                        "  {} {} (skipped - exists, use --force to overwrite)",
                        "~".yellow(),
                        artifact.path.display()
                    );
                    continue;
                }

                if !*dry_run {
                    // Create parent directories if needed
                    if let Some(parent) = full_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&full_path, &artifact.content)?;
                }

                let action = if exists { "updated" } else { "created" };
                println!(
                    "  {} {} ({})",
                    if exists { "~" } else { "+" },
                    artifact.path.display(),
                    action
                );
            }

            if !*dry_run {
                println!();
                println!("{} Scaffold applied successfully", "✓".green());
            }
        }

        // trace:FR-0269 - Template extraction command | ai:claude:high
        ScaffoldCommand::Extract { output, force } => {
            use aida_core::templates::TemplateLoader;

            let dest = output.clone().unwrap_or_else(|| {
                dirs::config_dir()
                    .map(|p| p.join("aida/templates"))
                    .unwrap_or_else(|| std::path::PathBuf::from("templates"))
            });

            println!(
                "{} Extracting embedded templates to: {}",
                "📦".blue(),
                dest.display()
            );

            // Create the destination directory if it doesn't exist
            if !dest.exists() {
                std::fs::create_dir_all(&dest)?;
            }

            let loader = TemplateLoader::new();
            let templates = loader.list_templates();

            let mut extracted = 0;
            let mut skipped = 0;

            for key in &templates {
                let full_path = dest.join(key);

                // Check if file exists and skip unless force
                if full_path.exists() && !force {
                    println!("  {} {} (skipped - exists)", "~".yellow(), key);
                    skipped += 1;
                    continue;
                }

                // Create parent directories if needed
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                // Load from embedded and write to disk
                let mut temp_loader = TemplateLoader::new();
                if let Some(content) = temp_loader.load(key) {
                    std::fs::write(&full_path, &content)?;
                    println!("  {} {} (extracted)", "+".green(), key);
                    extracted += 1;
                }
            }

            println!();
            println!(
                "{} Extracted {} templates ({} skipped)",
                "✓".green(),
                extracted,
                skipped
            );

            if skipped > 0 && !force {
                println!("  Use --force to overwrite existing files");
            }
        }
    }

    Ok(())
}

/// Search requirements for a pattern
#[allow(clippy::too_many_arguments)]
fn grep_requirements(
    storage: &Storage,
    pattern: &str,
    ignore_case: bool,
    extended_regex: bool,
    after_context: usize,
    before_context: usize,
    context: Option<usize>,
    field_filter: Option<&str>,
    status_filter: Option<&str>,
    type_filter: Option<&str>,
    feature_filter: Option<&str>,
    files_with_matches: bool,
    count_only: bool,
    invert_match: bool,
) -> Result<()> {
    use regex::RegexBuilder;

    let store = storage.load()?;

    // Build the regex pattern
    let regex = if extended_regex {
        RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()
            .context("Invalid regex pattern")?
    } else {
        // Escape special regex characters for literal search
        let escaped = regex::escape(pattern);
        RegexBuilder::new(&escaped)
            .case_insensitive(ignore_case)
            .build()
            .context("Invalid pattern")?
    };

    // Parse field filter
    let fields: HashSet<&str> = if let Some(f) = field_filter {
        f.split(',').map(|s| s.trim()).collect()
    } else {
        [
            "title",
            "description",
            "comments",
            "tags",
            "owner",
            "feature",
            "spec_id",
        ]
        .iter()
        .copied()
        .collect()
    };

    // Context lines (C overrides A and B)
    let ctx_before = context.unwrap_or(before_context);
    let ctx_after = context.unwrap_or(after_context);

    let mut total_matches = 0;
    let mut matching_reqs = 0;

    for req in &store.requirements {
        // Apply filters
        if let Some(status_str) = status_filter {
            let req_status = req.status.to_string().to_lowercase();
            if !req_status.contains(&status_str.to_lowercase()) {
                continue;
            }
        }

        if let Some(type_str) = type_filter {
            let req_type = req.req_type.to_string().to_lowercase();
            if !req_type.contains(&type_str.to_lowercase()) {
                continue;
            }
        }

        if let Some(feature_str) = feature_filter {
            if !req
                .feature
                .to_lowercase()
                .contains(&feature_str.to_lowercase())
            {
                continue;
            }
        }

        // Collect matches from all fields
        let mut matches: Vec<GrepMatch> = Vec::new();

        // Search title
        if fields.contains("title") {
            if let Some(m) = search_field(&regex, "title", &req.title, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search description
        if fields.contains("description") {
            for m in search_multiline_field(
                &regex,
                "description",
                &req.description,
                ctx_before,
                ctx_after,
            ) {
                matches.push(m);
            }
        }

        // Search spec_id
        if fields.contains("spec_id") {
            if let Some(spec_id) = &req.spec_id {
                if let Some(m) = search_field(&regex, "spec_id", spec_id, ctx_before, ctx_after) {
                    matches.push(m);
                }
            }
        }

        // Search owner
        if fields.contains("owner") {
            if let Some(m) = search_field(&regex, "owner", &req.owner, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search feature
        if fields.contains("feature") {
            if let Some(m) = search_field(&regex, "feature", &req.feature, ctx_before, ctx_after) {
                matches.push(m);
            }
        }

        // Search tags
        if fields.contains("tags") {
            for tag in &req.tags {
                if let Some(m) = search_field(&regex, "tags", tag, ctx_before, ctx_after) {
                    matches.push(m);
                }
            }
        }

        // Search comments
        if fields.contains("comments") {
            for comment in &req.comments {
                for m in search_multiline_field(
                    &regex,
                    &format!("comment:{}", comment.author),
                    &comment.content,
                    ctx_before,
                    ctx_after,
                ) {
                    matches.push(m);
                }
            }
        }

        let has_matches = !matches.is_empty();
        let should_show = if invert_match {
            !has_matches
        } else {
            has_matches
        };

        if should_show {
            matching_reqs += 1;
            let match_count = matches.len();
            total_matches += match_count;

            let id_string = req.id.to_string();
            let spec_id = req.spec_id.as_deref().unwrap_or(&id_string);

            if files_with_matches {
                // Just print the SPEC-ID
                println!("{}", spec_id.cyan());
            } else if count_only {
                // Print count for this requirement
                println!("{}: {}", spec_id.cyan(), match_count);
            } else if invert_match {
                // For invert match, just show SPEC-ID and title
                println!("{}: {}", spec_id.cyan(), req.title);
            } else {
                // Full output with matches
                println!("{}: {}", spec_id.cyan().bold(), req.title);
                for m in matches {
                    print_grep_match(&m);
                }
                println!();
            }
        }
    }

    // Summary
    if !files_with_matches && !count_only {
        if matching_reqs == 0 {
            if invert_match {
                println!("{}", "All requirements matched the pattern.".yellow());
            } else {
                println!("{}", "No matches found.".yellow());
            }
        } else {
            println!(
                "{} match(es) in {} requirement(s)",
                total_matches.to_string().green(),
                matching_reqs.to_string().green()
            );
        }
    }

    Ok(())
}

/// A single grep match result
struct GrepMatch {
    field: String,
    line_num: Option<usize>,
    line: String,
    match_start: usize,
    match_end: usize,
    context_before: Vec<String>,
    context_after: Vec<String>,
}

/// Search a single-line field for matches
fn search_field(
    regex: &regex::Regex,
    field: &str,
    content: &str,
    _ctx_before: usize,
    _ctx_after: usize,
) -> Option<GrepMatch> {
    if let Some(m) = regex.find(content) {
        Some(GrepMatch {
            field: field.to_string(),
            line_num: None,
            line: content.to_string(),
            match_start: m.start(),
            match_end: m.end(),
            context_before: vec![],
            context_after: vec![],
        })
    } else {
        None
    }
}

/// Search a multiline field for matches
fn search_multiline_field(
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
fn print_grep_match(m: &GrepMatch) {
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

// trace:FR-0315 | ai:claude:high
/// Generate HTML report for scaffold status with diffs
fn generate_scaffold_html_report(
    store: &RequirementsStore,
    root: &std::path::Path,
    config: &ScaffoldConfig,
    db_path: &std::path::Path,
    status: &aida_core::ScaffoldStatus,
) -> Result<String> {
    use std::fmt::Write;

    let mut scaffolder =
        Scaffolder::with_database(root.to_path_buf(), config.clone(), db_path.to_path_buf());
    let preview = scaffolder.preview(store);

    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");

    let mut html = String::new();

    // HTML header with inline styles
    writeln!(
        html,
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>AIDA Scaffold Status Report</title>
    <style>
        :root {{
            --bg-primary: #1a1a2e;
            --bg-secondary: #16213e;
            --bg-tertiary: #0f3460;
            --text-primary: #e4e4e7;
            --text-secondary: #a1a1aa;
            --accent-green: #22c55e;
            --accent-yellow: #eab308;
            --accent-red: #ef4444;
            --accent-blue: #3b82f6;
            --border-color: #27272a;
        }}
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: 'Segoe UI', system-ui, -apple-system, sans-serif;
            background: var(--bg-primary);
            color: var(--text-primary);
            line-height: 1.6;
            padding: 2rem;
        }}
        .container {{ max-width: 1200px; margin: 0 auto; }}
        header {{
            background: linear-gradient(135deg, var(--bg-secondary), var(--bg-tertiary));
            padding: 2rem;
            border-radius: 12px;
            margin-bottom: 2rem;
            border: 1px solid var(--border-color);
        }}
        h1 {{ color: var(--accent-blue); font-size: 1.75rem; margin-bottom: 0.5rem; }}
        .meta {{ color: var(--text-secondary); font-size: 0.875rem; }}
        .summary {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 1rem;
            margin-bottom: 2rem;
        }}
        .summary-card {{
            background: var(--bg-secondary);
            padding: 1.5rem;
            border-radius: 8px;
            border: 1px solid var(--border-color);
            text-align: center;
        }}
        .summary-card .count {{ font-size: 2.5rem; font-weight: bold; }}
        .summary-card .label {{ color: var(--text-secondary); font-size: 0.875rem; }}
        .count.green {{ color: var(--accent-green); }}
        .count.yellow {{ color: var(--accent-yellow); }}
        .count.red {{ color: var(--accent-red); }}
        .count.blue {{ color: var(--accent-blue); }}
        section {{
            background: var(--bg-secondary);
            border-radius: 12px;
            border: 1px solid var(--border-color);
            margin-bottom: 1.5rem;
            overflow: hidden;
        }}
        section h2 {{
            padding: 1rem 1.5rem;
            background: var(--bg-tertiary);
            font-size: 1.1rem;
            border-bottom: 1px solid var(--border-color);
        }}
        .file-list {{ list-style: none; }}
        .file-list li {{
            padding: 0.75rem 1.5rem;
            border-bottom: 1px solid var(--border-color);
            font-family: 'Consolas', 'Monaco', monospace;
            font-size: 0.875rem;
        }}
        .file-list li:last-child {{ border-bottom: none; }}
        .file-list .icon {{ margin-right: 0.5rem; }}
        details {{
            border-bottom: 1px solid var(--border-color);
        }}
        details:last-child {{ border-bottom: none; }}
        summary {{
            padding: 0.75rem 1.5rem;
            cursor: pointer;
            font-family: 'Consolas', 'Monaco', monospace;
            font-size: 0.875rem;
            background: var(--bg-secondary);
            transition: background 0.2s;
        }}
        summary:hover {{ background: var(--bg-tertiary); }}
        summary .icon {{ margin-right: 0.5rem; }}
        .diff {{
            padding: 1rem 1.5rem;
            background: #0d1117;
            overflow-x: auto;
            font-family: 'Consolas', 'Monaco', monospace;
            font-size: 0.8rem;
            line-height: 1.4;
        }}
        .diff-line {{ white-space: pre; }}
        .diff-line.add {{ color: #3fb950; background: rgba(46, 160, 67, 0.15); }}
        .diff-line.remove {{ color: #f85149; background: rgba(248, 81, 73, 0.15); }}
        .diff-line.context {{ color: #8b949e; }}
        .diff-line.header {{ color: #79c0ff; font-weight: bold; }}
        .status-ok {{ color: var(--accent-green); }}
        .status-warn {{ color: var(--accent-yellow); }}
        .status-error {{ color: var(--accent-red); }}
        .status-info {{ color: var(--accent-blue); }}
        .empty {{ padding: 2rem; text-align: center; color: var(--text-secondary); }}
    </style>
</head>
<body>
<div class="container">"#
    )?;

    // Header
    writeln!(
        html,
        r#"<header>
    <h1>📊 AIDA Scaffold Status Report</h1>
    <p class="meta">Project: {} • Generated: {}</p>
</header>"#,
        root.display(),
        timestamp
    )?;

    // Summary cards
    writeln!(
        html,
        r#"<div class="summary">
    <div class="summary-card">
        <div class="count green">{}</div>
        <div class="label">Matching</div>
    </div>
    <div class="summary-card">
        <div class="count yellow">{}</div>
        <div class="label">Modified</div>
    </div>
    <div class="summary-card">
        <div class="count red">{}</div>
        <div class="label">Missing</div>
    </div>
    <div class="summary-card">
        <div class="count blue">{}</div>
        <div class="label">Extra</div>
    </div>
</div>"#,
        status.matching.len(),
        status.modified.len(),
        status.missing.len(),
        status.extra.len()
    )?;

    // Overall status
    let overall_status = if status.is_current {
        r#"<p class="status-ok">✓ Scaffold is up to date</p>"#
    } else {
        r#"<p class="status-warn">⚠ Scaffold drift detected</p>"#
    };
    writeln!(
        html,
        r#"<section><h2>Status</h2><div style="padding: 1rem 1.5rem;">{}</div></section>"#,
        overall_status
    )?;

    // Matching files section
    if !status.matching.is_empty() {
        writeln!(
            html,
            r#"<section>
    <h2 class="status-ok">✓ Matching Files ({})</h2>
    <ul class="file-list">"#,
            status.matching.len()
        )?;
        for path in &status.matching {
            writeln!(
                html,
                r#"        <li><span class="icon">✓</span>{}</li>"#,
                path.display()
            )?;
        }
        writeln!(html, "    </ul>\n</section>")?;
    }

    // Modified files section with diffs
    if !status.modified.is_empty() {
        writeln!(
            html,
            r#"<section>
    <h2 class="status-warn">~ Modified Files ({})</h2>"#,
            status.modified.len()
        )?;

        for (path, file_status) in &status.modified {
            // Get expected content from scaffold preview
            let expected_content = preview
                .artifacts
                .iter()
                .find(|a| &a.path == path)
                .map(|a| a.content.as_str())
                .unwrap_or("");

            // Get actual content from disk
            let full_path = root.join(path);
            let actual_content = std::fs::read_to_string(&full_path).unwrap_or_default();

            // Generate diff
            let diff = generate_unified_diff(
                path.to_string_lossy().as_ref(),
                expected_content,
                &actual_content,
            );

            let status_info = match file_status {
                FileStatus::Modified {
                    expected_lines,
                    actual_lines,
                } => {
                    format!(
                        " (expected {} lines, found {})",
                        expected_lines, actual_lines
                    )
                }
                _ => String::new(),
            };

            writeln!(
                html,
                r#"    <details>
        <summary><span class="icon status-warn">~</span>{}{}</summary>
        <div class="diff">"#,
                path.display(),
                status_info
            )?;

            for line in diff.lines() {
                let class = if line.starts_with('+') && !line.starts_with("+++") {
                    "add"
                } else if line.starts_with('-') && !line.starts_with("---") {
                    "remove"
                } else if line.starts_with("@@")
                    || line.starts_with("---")
                    || line.starts_with("+++")
                {
                    "header"
                } else {
                    "context"
                };
                writeln!(
                    html,
                    r#"<div class="diff-line {}">{}</div>"#,
                    class,
                    html_escape(line)
                )?;
            }

            writeln!(html, "        </div>\n    </details>")?;
        }
        writeln!(html, "</section>")?;
    }

    // Missing files section
    if !status.missing.is_empty() {
        writeln!(
            html,
            r#"<section>
    <h2 class="status-error">✗ Missing Files ({})</h2>
    <ul class="file-list">"#,
            status.missing.len()
        )?;
        for path in &status.missing {
            writeln!(
                html,
                r#"        <li><span class="icon status-error">✗</span>{}</li>"#,
                path.display()
            )?;
        }
        writeln!(html, "    </ul>\n</section>")?;
    }

    // Extra files section
    if !status.extra.is_empty() {
        writeln!(
            html,
            r#"<section>
    <h2 class="status-info">+ Extra Files ({})</h2>
    <ul class="file-list">"#,
            status.extra.len()
        )?;
        for path in &status.extra {
            writeln!(
                html,
                r#"        <li><span class="icon status-info">+</span>{}</li>"#,
                path.display()
            )?;
        }
        writeln!(html, "    </ul>\n</section>")?;
    }

    // Footer
    writeln!(
        html,
        r#"</div>
</body>
</html>"#
    )?;

    Ok(html)
}

/// Generate a unified diff between expected and actual content
fn generate_unified_diff(filename: &str, expected: &str, actual: &str) -> String {
    use std::fmt::Write;

    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();

    let mut diff = String::new();
    writeln!(diff, "--- expected/{}", filename).ok();
    writeln!(diff, "+++ actual/{}", filename).ok();

    // Simple line-by-line diff with context
    let max_len = expected_lines.len().max(actual_lines.len());
    let context_size = 3;
    let mut in_hunk = false;
    let mut hunk_start_expected = 0;
    let mut hunk_start_actual = 0;
    let mut hunk_lines: Vec<String> = Vec::new();
    let mut last_change = 0;

    for i in 0..max_len {
        let exp_line = expected_lines.get(i).copied();
        let act_line = actual_lines.get(i).copied();

        let is_same = exp_line == act_line;

        if !is_same {
            // Start a new hunk if needed
            if !in_hunk {
                in_hunk = true;
                hunk_start_expected = i.saturating_sub(context_size);
                hunk_start_actual = i.saturating_sub(context_size);
                // Add context before
                for j in hunk_start_expected..i {
                    if let Some(line) = expected_lines.get(j) {
                        hunk_lines.push(format!(" {}", line));
                    }
                }
            }
            last_change = i;

            // Add the diff lines
            if let Some(line) = exp_line {
                hunk_lines.push(format!("-{}", line));
            }
            if let Some(line) = act_line {
                hunk_lines.push(format!("+{}", line));
            }
        } else if in_hunk {
            // We have a matching line in a hunk
            if i <= last_change + context_size {
                // Still within context after
                if let Some(line) = exp_line {
                    hunk_lines.push(format!(" {}", line));
                }
            } else {
                // End the hunk
                let exp_count = hunk_lines.iter().filter(|l| !l.starts_with('+')).count();
                let act_count = hunk_lines.iter().filter(|l| !l.starts_with('-')).count();
                writeln!(
                    diff,
                    "@@ -{},{} +{},{} @@",
                    hunk_start_expected + 1,
                    exp_count,
                    hunk_start_actual + 1,
                    act_count
                )
                .ok();
                for line in &hunk_lines {
                    writeln!(diff, "{}", line).ok();
                }
                hunk_lines.clear();
                in_hunk = false;
            }
        }
    }

    // Flush any remaining hunk
    if !hunk_lines.is_empty() {
        let exp_count = hunk_lines.iter().filter(|l| !l.starts_with('+')).count();
        let act_count = hunk_lines.iter().filter(|l| !l.starts_with('-')).count();
        writeln!(
            diff,
            "@@ -{},{} +{},{} @@",
            hunk_start_expected + 1,
            exp_count,
            hunk_start_actual + 1,
            act_count
        )
        .ok();
        for line in &hunk_lines {
            writeln!(diff, "{}", line).ok();
        }
    }

    if diff.lines().count() <= 2 {
        // No actual differences found, show a note
        diff.push_str("@@ -1,1 +1,1 @@\n");
        diff.push_str(" (Files appear identical or differ only in whitespace)\n");
    }

    diff
}

/// Escape HTML special characters
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// trace:STORY-0368 | ai:claude
/// Handle queue commands
fn handle_queue_command(cmd: &QueueCommand, storage: &Storage) -> Result<()> {
    let get_user = |user: &Option<String>| -> String {
        user.clone().unwrap_or_else(|| {
            std::env::var("AIDA_USER")
                .or_else(|_| std::env::var("USER"))
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "default".to_string())
        })
    };

    match cmd {
        QueueCommand::List {
            user,
            include_completed,
        } => {
            let user_id = get_user(user);
            let entries = storage.queue_list(&user_id, *include_completed)?;
            let store = storage.load()?;

            if entries.is_empty() {
                println!("{}", "Your queue is empty.".dimmed());
                return Ok(());
            }

            println!("{}", format!("My Queue ({} items)", entries.len()).bold());
            println!("{}", "─".repeat(80));

            for (i, entry) in entries.iter().enumerate() {
                let req = store
                    .requirements
                    .iter()
                    .find(|r| r.id == entry.requirement_id);
                let spec_id = req.and_then(|r| r.spec_id.as_deref()).unwrap_or("???");
                let title = req.map(|r| r.title.as_str()).unwrap_or("(deleted)");
                let status = req
                    .map(|r| format!("{}", r.status))
                    .unwrap_or_else(|| "Unknown".to_string());

                let status_colored = match status.as_str() {
                    "Draft" => status.dimmed(),
                    "Approved" => status.blue(),
                    "Planned" => status.cyan(),
                    "In Progress" => status.yellow(),
                    "Completed" => status.green(),
                    "Rejected" => status.red(),
                    _ => status.normal(),
                };

                print!(
                    "  {}. {} {}",
                    (i + 1).to_string().dimmed(),
                    spec_id.bold(),
                    title
                );
                print!("  [{}]", status_colored);
                if entry.added_by != user_id {
                    print!("  {}", format!("(from @{})", entry.added_by).dimmed());
                }
                if let Some(ref note) = entry.note {
                    print!("  {}", format!("\"{}\"", note).dimmed().italic());
                }
                println!();
            }
        }
        QueueCommand::Add {
            id,
            top,
            bottom: _,
            user,
            note,
        } => {
            let user_id = get_user(user);
            let store = storage.load()?;

            // Resolve requirement ID
            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store
                    .requirements
                    .iter()
                    .find(|r| r.spec_id.as_deref() == Some(id.as_str()))
            }
            .ok_or_else(|| anyhow::anyhow!("Requirement not found: {}", id))?;

            let position = if *top {
                let entries = storage.queue_list(&user_id, true)?;
                entries.first().map(|e| e.position - 1000).unwrap_or(1000)
            } else {
                i64::MAX // sentinel: queue_add auto-assigns max+1000
            };

            let entry = aida_core::QueueEntry {
                user_id: user_id.clone(),
                requirement_id: req.id,
                position,
                added_by: user_id.clone(),
                note: note.clone(),
                added_at: chrono::Utc::now(),
            };
            storage.queue_add(entry)?;

            let spec_id = req.spec_id.as_deref().unwrap_or("???");
            println!(
                "{} Added {} ({}) to queue",
                "✓".green(),
                spec_id.bold(),
                req.title
            );
        }
        QueueCommand::Remove { id, user } => {
            let user_id = get_user(user);
            let store = storage.load()?;

            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store
                    .requirements
                    .iter()
                    .find(|r| r.spec_id.as_deref() == Some(id.as_str()))
            }
            .ok_or_else(|| anyhow::anyhow!("Requirement not found: {}", id))?;

            storage.queue_remove(&user_id, &req.id)?;
            let spec_id = req.spec_id.as_deref().unwrap_or("???");
            println!("{} Removed {} from queue", "✓".green(), spec_id.bold());
        }
        QueueCommand::Move {
            id,
            top,
            bottom,
            before,
        } => {
            let user_id = std::env::var("AIDA_USER")
                .or_else(|_| std::env::var("USER"))
                .unwrap_or_else(|_| "default".to_string());
            let store = storage.load()?;

            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store
                    .requirements
                    .iter()
                    .find(|r| r.spec_id.as_deref() == Some(id.as_str()))
            }
            .ok_or_else(|| anyhow::anyhow!("Requirement not found: {}", id))?;

            let entries = storage.queue_list(&user_id, true)?;
            let new_position = if *top {
                entries.first().map(|e| e.position - 1000).unwrap_or(0)
            } else if *bottom {
                entries.last().map(|e| e.position + 1000).unwrap_or(1000)
            } else if let Some(ref before_id) = before {
                let before_req = if let Ok(uuid) = uuid::Uuid::parse_str(before_id) {
                    store.requirements.iter().find(|r| r.id == uuid)
                } else {
                    store
                        .requirements
                        .iter()
                        .find(|r| r.spec_id.as_deref() == Some(before_id.as_str()))
                }
                .ok_or_else(|| anyhow::anyhow!("Requirement not found: {}", before_id))?;

                entries
                    .iter()
                    .find(|e| e.requirement_id == before_req.id)
                    .map(|e| e.position - 1)
                    .unwrap_or(0)
            } else {
                anyhow::bail!("Specify --top, --bottom, or --before <ID>");
            };

            storage.queue_reorder(&user_id, &[(req.id, new_position)])?;
            let spec_id = req.spec_id.as_deref().unwrap_or("???");
            println!("{} Moved {} in queue", "✓".green(), spec_id.bold());
        }
        QueueCommand::Clear { user, completed } => {
            let user_id = get_user(user);
            storage.queue_clear(&user_id, *completed)?;
            if *completed {
                println!("{} Cleared completed items from queue", "✓".green());
            } else {
                println!("{} Cleared all items from queue", "✓".green());
            }
        }
    }
    Ok(())
}

// trace:STORY-0321 | ai:claude
/// Handle GitLab integration commands
fn handle_gitlab_command(cmd: &GitLabCommand, storage: &Storage) -> Result<()> {
    // Create tokio runtime for async operations
    let rt = tokio::runtime::Runtime::new()?;

    match cmd {
        GitLabCommand::Config {
            url,
            project,
            token,
            show,
        } => {
            if *show {
                // Show current configuration
                match GitLabConfig::load() {
                    Ok(Some(config)) => {
                        println!("{}", "GitLab Configuration:".bold());
                        println!("  URL:        {}", config.url);
                        println!("  Project ID: {}", config.project_id);
                        println!(
                            "  Enabled:    {}",
                            if config.enabled { "yes" } else { "no" }
                        );
                        if config.effective_token().is_some() {
                            println!("  Token:      {}", "(configured)".green());
                        } else {
                            println!("  Token:      {}", "(not set)".yellow());
                        }
                        println!("\nLabel prefix: {}", config.labels.prefix);
                        println!("Sync mode:    {:?}", config.sync.mode);
                    }
                    Ok(None) => {
                        println!("{}", "GitLab is not configured.".yellow());
                        println!("Use 'aida gitlab config --url <URL> --project <ID> --token <TOKEN>' to configure.");
                    }
                    Err(e) => {
                        println!("{}: {}", "Error loading config".red(), e);
                    }
                }
                return Ok(());
            }

            // Update configuration
            let mut config = GitLabConfig::load()?.unwrap_or_default();

            if let Some(u) = url {
                config.url = u.clone();
                println!("Set URL: {}", u);
            }
            if let Some(p) = project {
                config.project_id = *p;
                println!("Set project ID: {}", p);
            }
            if let Some(t) = token {
                // Store token in environment for this session
                // In production, would use keyring
                std::env::set_var("AIDA_GITLAB_TOKEN", t);
                config.token = Some(t.clone());
                println!("Set token: {}", "(hidden)");
            }

            // Save config (token excluded from file)
            config.save()?;
            println!("{}", "Configuration saved.".green());

            // Validate if we have enough config
            if let Err(e) = config.validate() {
                println!("{}: {}", "Warning".yellow(), e);
                println!("Run 'aida gitlab test' to verify connection.");
            }
        }

        GitLabCommand::Test => {
            // Test connection to GitLab
            let config = GitLabConfig::load()?.ok_or_else(|| {
                anyhow::anyhow!("GitLab not configured. Run 'aida gitlab config' first.")
            })?;

            println!("Testing connection to {}...", config.url);

            let client = GitLabClient::new(config)?;
            let project = rt.block_on(client.test_connection())?;

            println!("{}", "Connection successful!".green());
            println!("  Project: {}", project.name_with_namespace);
            println!("  URL:     {}", project.web_url);
            if let Some(desc) = &project.description {
                if !desc.is_empty() {
                    println!("  Desc:    {}", desc);
                }
            }
        }

        GitLabCommand::List {
            state,
            labels,
            search,
            limit,
        } => {
            let config = GitLabConfig::load()?.ok_or_else(|| {
                anyhow::anyhow!("GitLab not configured. Run 'aida gitlab config' first.")
            })?;

            let client = GitLabClient::new(config)?;

            // Build filter
            let mut filter = match state.as_str() {
                "opened" => IssueFilter::open(),
                "closed" => IssueFilter {
                    state: Some(IssueState::Closed),
                    ..Default::default()
                },
                _ => IssueFilter::default(),
            };

            if let Some(l) = labels {
                filter = filter.with_labels(l.split(',').map(|s| s.trim().to_string()).collect());
            }

            if let Some(s) = search {
                filter.search = Some(s.clone());
            }

            filter.per_page = Some(*limit);

            let issues = rt.block_on(client.list_issues(Some(filter)))?;

            if issues.is_empty() {
                println!("{}", "No issues found.".yellow());
                return Ok(());
            }

            println!("{}", format!("Found {} issues:", issues.len()).bold());
            println!();

            for issue in &issues {
                let state_indicator = if issue.is_open() {
                    "●".green()
                } else {
                    "○".bright_black()
                };

                println!(
                    "{} {} {}",
                    state_indicator,
                    format!("GL-{}", issue.iid).cyan(),
                    issue.title
                );

                if !issue.labels.is_empty() {
                    println!("    Labels: {}", issue.labels.join(", ").bright_black());
                }
            }
        }

        GitLabCommand::Show { iid } => {
            let config = GitLabConfig::load()?.ok_or_else(|| {
                anyhow::anyhow!("GitLab not configured. Run 'aida gitlab config' first.")
            })?;

            let client = GitLabClient::new(config)?;

            // Parse IID (handle "GL-123" or "123" format)
            let iid_num: u64 = iid
                .strip_prefix("GL-")
                .unwrap_or(iid)
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid issue IID: {}", iid))?;

            let issue = rt.block_on(client.get_issue(iid_num))?;

            println!("{}", format!("GL-{}: {}", issue.iid, issue.title).bold());
            println!();

            let state_str = if issue.is_open() {
                "Open".green()
            } else {
                "Closed".bright_black()
            };
            println!("State:    {}", state_str);
            println!("Author:   {}", issue.author.username);
            if let Some(assignee) = issue.assignee_username() {
                println!("Assignee: {}", assignee);
            }
            if !issue.labels.is_empty() {
                println!("Labels:   {}", issue.labels.join(", "));
            }
            if let Some(milestone) = &issue.milestone {
                println!("Milestone: {}", milestone.title);
            }
            println!("URL:      {}", issue.web_url);
            println!();

            if let Some(desc) = &issue.description {
                if !desc.is_empty() {
                    println!("{}", "Description:".bold());
                    println!("{}", desc);
                }
            }
        }

        // trace:STORY-0325 | ai:claude
        GitLabCommand::Status { id, diverged } => {
            use aida_core::{GitLabSyncState, LinkOrigin, SyncStatus};

            // Check if storage is SQLite (sync state only works with SQLite)
            if !storage.is_sqlite() {
                println!(
                    "{}",
                    "GitLab sync status is only available for SQLite databases.".yellow()
                );
                return Ok(());
            }

            let store = storage.load()?;

            // Load sync states based on filter
            let sync_states = if let Some(req_id) = id {
                // Find the requirement by spec_id or UUID
                let requirement = store.requirements.iter().find(|r| {
                    r.spec_id.as_deref() == Some(req_id.as_str()) || r.id.to_string() == *req_id
                });

                if let Some(req) = requirement {
                    storage.load_sync_states_for_requirement(req.id)?
                } else {
                    println!("{} {}", "Requirement not found:".red(), req_id);
                    return Ok(());
                }
            } else {
                storage.load_all_sync_states()?
            };

            // Filter by diverged if requested
            let sync_states: Vec<_> = if *diverged {
                sync_states
                    .into_iter()
                    .filter(|s| !matches!(s.sync_status, SyncStatus::InSync))
                    .collect()
            } else {
                sync_states
            };

            if sync_states.is_empty() {
                if *diverged {
                    println!("{}", "No diverged GitLab links found.".green());
                } else {
                    println!("{}", "No GitLab sync states found.".yellow());
                    println!("Link requirements to GitLab issues using the GUI or create issues from requirements.");
                }
                return Ok(());
            }

            // Display sync states
            println!("{}", "GitLab Sync Status".bold());
            println!("{}", "─".repeat(60));

            for state in &sync_states {
                // Find the requirement for this sync state
                let req = store
                    .requirements
                    .iter()
                    .find(|r| r.id == state.requirement_id);

                let req_display = if let Some(r) = req {
                    r.spec_id.clone().unwrap_or_else(|| r.id.to_string())
                } else {
                    state.spec_id.clone()
                };

                // Status icon and color
                let (status_icon, status_color) = match state.sync_status {
                    SyncStatus::InSync => ("✓", "green"),
                    SyncStatus::AidaModified => ("△", "yellow"),
                    SyncStatus::GitLabModified => ("▽", "cyan"),
                    SyncStatus::Conflict => ("⚠", "red"),
                    SyncStatus::Error => ("✗", "red"),
                    SyncStatus::Untracked => ("?", "dimmed"),
                };

                let status_text = match state.sync_status {
                    SyncStatus::InSync => "In Sync".green(),
                    SyncStatus::AidaModified => "AIDA Modified".yellow(),
                    SyncStatus::GitLabModified => "GitLab Modified".cyan(),
                    SyncStatus::Conflict => "Conflict".red(),
                    SyncStatus::Error => "Error".red(),
                    SyncStatus::Untracked => "Untracked".dimmed(),
                };

                let origin_text = match state.link_origin {
                    LinkOrigin::CreatedFromAida => "→GL",
                    LinkOrigin::ImportedFromGitLab => "←GL",
                    LinkOrigin::ManualLink => "↔GL",
                };

                println!(
                    "{} {} {} GL-{} [{}] {}",
                    status_icon,
                    req_display.bold(),
                    origin_text.dimmed(),
                    state.gitlab_issue_iid,
                    status_text,
                    state
                        .last_sync
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                        .dimmed()
                );

                if let Some(error) = &state.last_error {
                    println!("    {} {}", "Error:".red(), error);
                }
            }

            println!("{}", "─".repeat(60));
            println!(
                "Total: {} links ({} in sync, {} diverged)",
                sync_states.len(),
                sync_states
                    .iter()
                    .filter(|s| matches!(s.sync_status, SyncStatus::InSync))
                    .count(),
                sync_states
                    .iter()
                    .filter(|s| !matches!(s.sync_status, SyncStatus::InSync))
                    .count()
            );
        }

        // trace:STORY-0326 | ai:claude
        GitLabCommand::Labels {
            validate,
            create_missing,
            init,
        } => {
            // Load or create config
            let mut config = GitLabConfig::load()?.unwrap_or_default();

            // Initialize with defaults if requested
            if *init {
                config.labels = config.labels.with_defaults();
                config.save()?;
                println!("{}", "Label mappings initialized with defaults.".green());
            }

            // Show current label configuration
            println!("{}", "GitLab Label Mappings".bold());
            println!("{}", "─".repeat(50));

            if !config.labels.prefix.is_empty() {
                println!("Prefix: {}", config.labels.prefix.cyan());
            }

            println!("\n{}", "Type Mappings:".bold());
            if config.labels.types.is_empty() {
                println!(
                    "  {} (use --init to set defaults)",
                    "(none configured)".dimmed()
                );
            } else {
                for (aida_type, gitlab_label) in &config.labels.types {
                    println!("  {} → {}", aida_type, gitlab_label.cyan());
                }
            }

            println!("\n{}", "Priority Mappings:".bold());
            if config.labels.priorities.is_empty() {
                println!(
                    "  {} (use --init to set defaults)",
                    "(none configured)".dimmed()
                );
            } else {
                for (priority, gitlab_label) in &config.labels.priorities {
                    println!("  {} → {}", priority, gitlab_label.cyan());
                }
            }

            println!("\n{}", "Status Mappings:".bold());
            if config.labels.statuses.is_empty() {
                println!(
                    "  {} (use --init to set defaults)",
                    "(none configured)".dimmed()
                );
            } else {
                for (status, gitlab_label) in &config.labels.statuses {
                    println!("  {} → {}", status, gitlab_label.cyan());
                }
            }

            println!(
                "\nAuto-create labels: {}",
                if config.labels.auto_create_labels {
                    "yes".green()
                } else {
                    "no".dimmed()
                }
            );

            // Validate labels if requested
            if *validate || *create_missing {
                let Some(token) = config.effective_token() else {
                    return Err(anyhow::anyhow!("GitLab token required. Set AIDA_GITLAB_TOKEN or run 'aida gitlab config --token <TOKEN>'"));
                };

                let mut config_with_token = config.clone();
                config_with_token.token = Some(token);
                let client = GitLabClient::new(config_with_token)?;

                println!("\n{}", "Validating labels in GitLab...".dimmed());

                // Get all labels from GitLab project
                let gitlab_labels = rt.block_on(client.list_labels())?;
                let gitlab_label_names: std::collections::HashSet<_> =
                    gitlab_labels.iter().map(|l| l.name.clone()).collect();

                // Get all mapped labels
                let mapped_labels = config.labels.all_labels();
                let mut missing_labels = Vec::new();
                let mut found_labels = Vec::new();

                for label in &mapped_labels {
                    if gitlab_label_names.contains(label) {
                        found_labels.push(label.clone());
                    } else {
                        missing_labels.push(label.clone());
                    }
                }

                println!("\n{}", "Validation Results:".bold());
                println!(
                    "  {} labels found in GitLab",
                    found_labels.len().to_string().green()
                );
                if !missing_labels.is_empty() {
                    println!(
                        "  {} labels missing:",
                        missing_labels.len().to_string().yellow()
                    );
                    for label in &missing_labels {
                        println!("    - {}", label.yellow());
                    }
                }

                // Create missing labels if requested
                if *create_missing && !missing_labels.is_empty() {
                    println!("\n{}", "Creating missing labels...".dimmed());
                    for label in &missing_labels {
                        // Determine label color based on type
                        let color = if label.starts_with("type::") {
                            "#428BCA" // Blue for types
                        } else if label.starts_with("priority::") {
                            if label.contains("high") {
                                "#DC3545"
                            } else if label.contains("low") {
                                "#28A745"
                            } else {
                                "#FFC107"
                            }
                        } else if label.starts_with("status::") {
                            "#6C757D" // Gray for status
                        } else {
                            "#7950F2" // Purple default
                        };

                        match rt.block_on(client.create_label(label, color, None)) {
                            Ok(_) => println!("  {} Created: {}", "✓".green(), label),
                            Err(e) => println!("  {} Failed to create {}: {}", "✗".red(), label, e),
                        }
                    }
                }
            }
        }

        // trace:STORY-0327 | ai:claude
        GitLabCommand::Refresh { id, force } => {
            use aida_core::{GitLabSyncState, IssueFilter, SyncStatus};

            // Check if storage is SQLite (sync state only works with SQLite)
            if !storage.is_sqlite() {
                println!(
                    "{}",
                    "GitLab refresh is only available for SQLite databases.".yellow()
                );
                return Ok(());
            }

            // Load GitLab config
            let config = GitLabConfig::load()?.ok_or_else(|| {
                anyhow::anyhow!("GitLab not configured. Run 'aida gitlab config' first.")
            })?;

            let Some(token) = config.effective_token() else {
                return Err(anyhow::anyhow!("GitLab token required. Set AIDA_GITLAB_TOKEN or run 'aida gitlab config --token <TOKEN>'"));
            };

            let mut config_with_token = config.clone();
            config_with_token.token = Some(token);
            let client = GitLabClient::new(config_with_token)?;

            let store = storage.load()?;

            // Get sync states to refresh
            let sync_states = if let Some(req_id) = id {
                // Find the requirement by spec_id or UUID
                let requirement = store.requirements.iter().find(|r| {
                    r.spec_id.as_deref() == Some(req_id.as_str()) || r.id.to_string() == *req_id
                });

                if let Some(req) = requirement {
                    storage.load_sync_states_for_requirement(req.id)?
                } else {
                    println!("{} {}", "Requirement not found:".red(), req_id);
                    return Ok(());
                }
            } else {
                storage.load_all_sync_states()?
            };

            if sync_states.is_empty() {
                println!("{}", "No GitLab sync states found to refresh.".yellow());
                println!("Link requirements to GitLab issues first.");
                return Ok(());
            }

            println!("{}", "Refreshing GitLab sync states...".dimmed());
            println!("{}", "─".repeat(60));

            // Collect all issue IIDs to fetch
            let iids: Vec<u64> = sync_states.iter().map(|s| s.gitlab_issue_iid).collect();

            // Fetch issues from GitLab
            let filter = IssueFilter::default().with_iids(iids);
            let issues = rt.block_on(client.list_issues(Some(filter)))?;

            // Create a map of IID -> Issue for quick lookup
            let issue_map: std::collections::HashMap<u64, _> =
                issues.into_iter().map(|i| (i.iid, i)).collect();

            let mut updated_count = 0;
            let mut error_count = 0;

            for mut state in sync_states {
                // Find the requirement
                let req = store
                    .requirements
                    .iter()
                    .find(|r| r.id == state.requirement_id);

                let req_display = if let Some(r) = req {
                    r.spec_id.clone().unwrap_or_else(|| r.id.to_string())
                } else {
                    state.spec_id.clone()
                };

                // Get the GitLab issue
                if let Some(issue) = issue_map.get(&state.gitlab_issue_iid) {
                    // Calculate current hashes
                    let current_gitlab_hash = GitLabSyncState::hash_gitlab_issue(issue);
                    let current_aida_hash = if let Some(r) = req {
                        GitLabSyncState::hash_requirement(r)
                    } else {
                        state.aida_content_hash.clone()
                    };

                    // Determine new sync status
                    let old_status = state.sync_status.clone();
                    let aida_changed = current_aida_hash != state.aida_content_hash;
                    let gitlab_changed = current_gitlab_hash != state.gitlab_content_hash;

                    let new_status = match (aida_changed, gitlab_changed) {
                        (false, false) => SyncStatus::InSync,
                        (true, false) => SyncStatus::AidaModified,
                        (false, true) => SyncStatus::GitLabModified,
                        (true, true) => SyncStatus::Conflict,
                    };

                    // Update if changed or forced
                    if *force || new_status != old_status {
                        state.sync_status = new_status.clone();
                        state.last_sync = chrono::Utc::now();
                        state.last_error = None;

                        // Update stored hashes if this is a fresh sync
                        if state.aida_content_hash.is_empty() {
                            state.aida_content_hash = current_aida_hash;
                        }
                        if state.gitlab_content_hash.is_empty() {
                            state.gitlab_content_hash = current_gitlab_hash;
                        }

                        if let Err(e) = storage.save_sync_state(&state) {
                            println!(
                                "  {} {} GL-{}: {}",
                                "✗".red(),
                                req_display,
                                state.gitlab_issue_iid,
                                e
                            );
                            error_count += 1;
                        } else {
                            let status_indicator = match new_status {
                                SyncStatus::InSync => "✓".green(),
                                SyncStatus::AidaModified => "△".yellow(),
                                SyncStatus::GitLabModified => "▽".cyan(),
                                SyncStatus::Conflict => "⚠".red(),
                                _ => "?".dimmed(),
                            };
                            println!(
                                "  {} {} GL-{}: {}",
                                status_indicator, req_display, state.gitlab_issue_iid, new_status
                            );
                            updated_count += 1;
                        }
                    } else {
                        println!(
                            "  {} {} GL-{}: {} (unchanged)",
                            "·".dimmed(),
                            req_display,
                            state.gitlab_issue_iid,
                            old_status
                        );
                    }
                } else {
                    println!(
                        "  {} {} GL-{}: {}",
                        "?".yellow(),
                        req_display,
                        state.gitlab_issue_iid,
                        "Issue not found in GitLab"
                    );
                    error_count += 1;
                }
            }

            println!("{}", "─".repeat(60));
            println!(
                "Refreshed: {} updated, {} errors",
                updated_count.to_string().green(),
                if error_count > 0 {
                    error_count.to_string().red()
                } else {
                    "0".dimmed()
                }
            );
        }

        // trace:STORY-0327 | ai:claude
        GitLabCommand::Poll { action, interval } => match action.to_lowercase().as_str() {
            "status" => {
                let config = GitLabConfig::load()?;
                if let Some(config) = config {
                    println!("{}", "GitLab Polling Configuration".bold());
                    println!("{}", "─".repeat(40));
                    println!(
                        "Polling enabled: {}",
                        if config.polling.enabled {
                            "yes".green()
                        } else {
                            "no".dimmed()
                        }
                    );
                    println!(
                        "Interval: {} seconds ({} minutes)",
                        config.polling.interval_seconds,
                        config.polling.interval_seconds / 60
                    );
                    println!("Batch size: {}", config.polling.batch_size);
                    println!("Max concurrent: {}", config.polling.max_concurrent);
                    println!();
                    println!(
                        "{}",
                        "Note: Background polling runs in the AIDA GUI.".dimmed()
                    );
                    println!("{}", "Use 'aida gitlab refresh' for manual sync.".dimmed());
                } else {
                    println!("{}", "GitLab not configured.".yellow());
                }
            }
            "start" => {
                println!(
                    "{}",
                    "Background polling is managed by the AIDA GUI.".yellow()
                );
                println!();
                println!("To enable polling:");
                println!("  1. Open AIDA GUI");
                println!("  2. Go to Settings > GitLab");
                println!("  3. Enable 'Background Polling'");
                println!();
                println!("For CLI-based polling, use a cron job or scheduled task:");
                println!(
                    "  {} aida gitlab refresh",
                    format!("*/{} * * * *", interval / 60).dimmed()
                );
            }
            "stop" => {
                println!(
                    "{}",
                    "Background polling is managed by the AIDA GUI.".yellow()
                );
                println!();
                println!("To disable polling:");
                println!("  1. Open AIDA GUI");
                println!("  2. Go to Settings > GitLab");
                println!("  3. Disable 'Background Polling'");
            }
            _ => {
                println!(
                    "{}: Unknown action '{}'. Use: status, start, stop",
                    "Error".red(),
                    action
                );
            }
        },
    }

    Ok(())
}
