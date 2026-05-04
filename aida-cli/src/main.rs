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
    CacheCommand, Cli, Command, CommentCommand, ConfigCommand, DbCommand, DevCommand,
    FeatureCommand, GitHubCommand, GitLabCommand, JiraCommand, QueueCommand, RelDefCommand,
    RelationshipCommand, ReportCommand, RoleCommand, ScaffoldCommand, ServerCommand, TraceCommand,
    TypeCommand,
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
    // Intercept --version / -V before clap so we can include the build-time
    // banner (build.rs stamps build time + git sha + dirty flag). Clap's
    // built-in #[clap(version)] only knows the package version, which can't
    // distinguish two binaries at the same version built at different times.
    {
        let args: Vec<String> = std::env::args().collect();
        if args.len() == 2 && (args[1] == "--version" || args[1] == "-V") {
            println!("aida {}", build_banner());
            return Ok(());
        }
    }

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
        distributed: _,
        centralized,
        sibling,
        registry_remote,
    } = &cli.command
    {
        // Default: distributed (git-canonical) mode per EPIC-1-001.
        // --sibling implies distributed-sibling. --centralized opts into
        // the deprecated SQLite-canonical path.
        // trace:EPIC-1-001 | ai:claude
        if *centralized {
            handle_init_command(*no_skills, agent, *no_hooks, *force)?;
        } else if *sibling {
            handle_init_distributed_sibling(
                registry_remote.as_deref(),
                *force,
                *no_skills,
                agent,
                *no_hooks,
            )?;
        } else {
            handle_init_distributed_worktree(*force, *no_skills, agent, *no_hooks)?;
        }
        return Ok(());
    }

    // Handle upgrade before storage resolution — it needs no DB.
    // trace:EPIC-1-001 | ai:claude
    if let Command::Upgrade { check, version, yes, target } = &cli.command {
        return handle_upgrade_command(*check, version.as_deref(), *yes, target.as_deref());
    }

    // Handle dev commands before storage resolution — most need no DB.
    // (Dev::Serve does interact with storage but spawns aida-server which
    // handles that itself; the wrapper just supervises the children.)
    // trace:EPIC-1-001 | ai:claude
    if let Command::Dev(dev_cmd) = &cli.command {
        return handle_dev_command(dev_cmd);
    }

    // Help-all is pure text; no storage needed.
    if let Command::HelpAll = &cli.command {
        print_help_all();
        return Ok(());
    }

    // Roles + statusline dispatch before storage init — roles are TOML
    // files at .aida/roles/, statusline reads the cache directly.
    if let Command::Role(role_cmd) = &cli.command {
        return handle_role_command(role_cmd);
    }
    if let Command::Statusline = &cli.command {
        return handle_statusline_command();
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
        // Check for distributed mode config (.aida/config.toml with store_path)
        // Skip for commands that use their own APIs (Jira, GitHub, GitLab)
        let is_external_integration = matches!(
            &cli.command,
            Command::Jira(_) | Command::Github(_) | Command::Gitlab(_)
        );
        if !is_external_integration {
        if let Some(store_path) = detect_distributed_store() {
            // MCP server needs the Storage class — snapshot git backend to temp YAML
            if matches!(&cli.command, Command::McpServe) {
                let backend = aida_core::GitBackend::new(&store_path)?;
                let store = aida_core::DatabaseBackend::load(&backend)?;
                let cache_path = store_path.join(".aida").join("mcp-cache.yaml");
                std::fs::create_dir_all(cache_path.parent().unwrap())?;
                // Use the YAML backend to write the snapshot
                let yaml_backend = aida_core::YamlBackend::new(&cache_path);
                aida_core::DatabaseBackend::save(&yaml_backend, &store)?;
                let mcp_storage = Storage::new(cache_path);
                mcp::run_mcp_server(&mcp_storage)?;
                return Ok(());
            }
            return handle_git_backend_command(&store_path, &cli.command);
        }
        } // close is_external_integration check

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
        Command::Cache(_) => {
            anyhow::bail!(
                "aida cache commands are only available in git-canonical (distributed) mode. \
                 Run `aida init --distributed` first, or pass --file pointing to a git store."
            );
        }
        Command::Status { no_dev_context } => {
            handle_status_command(*no_dev_context, None, &storage)?;
        }
        Command::Upgrade { .. } => unreachable!("upgrade is dispatched before storage init"),
        Command::Dev(_) => unreachable!("dev is dispatched before storage init"),
        Command::HelpAll => unreachable!("help-all is dispatched before storage init"),
        Command::Role(_) => unreachable!("role is dispatched before storage init"),
        Command::Statusline => unreachable!("statusline is dispatched before storage init"),
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
        Command::Github(github_cmd) => {
            handle_github_command(github_cmd, &storage)?;
        }
        Command::Jira(jira_cmd) => {
            handle_jira_command(jira_cmd, &storage)?;
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
/// Count requirements in a git-canonical store at `store_path`. Returns
/// None if the store doesn't exist or can't be opened — caller treats that
/// as "no data to lose, proceed".
/// trace:EPIC-1-001 | ai:claude
fn count_requirements_in_store(store_path: &std::path::Path) -> Option<usize> {
    if !store_path.is_dir() {
        return None;
    }
    let backend = aida_core::GitBackend::new(store_path).ok()?;
    let store = aida_core::DatabaseBackend::load(&backend).ok()?;
    Some(store.requirements.len())
}

/// Same as count_requirements_in_store but for a legacy SQLite-canonical
/// store at `db_path`.
fn count_requirements_in_sqlite(db_path: &std::path::Path) -> Result<usize> {
    let storage = Storage::new(db_path);
    Ok(storage.load()?.requirements.len())
}

/// Surface the data-loss risk of `aida init --force` on a populated store.
/// Returns true if the user confirmed (typed "reset"), false otherwise.
/// Bails the parent caller via Ok if the user cancels — caller pattern is
/// `if !confirm_destructive_reset(...)? { return Ok(()); }`.
/// trace:EPIC-1-001 | ai:claude
fn confirm_destructive_reset(count: usize, store_path: &std::path::Path) -> Result<bool> {
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
    eprintln!(
        "If you only wanted to refresh scaffolding (CLAUDE.md, .claude/skills/, hooks),"
    );
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

fn handle_init_command(no_skills: bool, agent: &str, no_hooks: bool, force: bool) -> Result<()> {
    eprintln!(
        "{}: --centralized initializes a SQLite-canonical store, which is deprecated.",
        "warning".yellow().bold()
    );
    eprintln!(
        "         Run `aida init` (without flags) to use the recommended git-canonical store."
    );
    eprintln!();

    let db_path = std::path::PathBuf::from("requirements.db");

    // Check existing state
    if db_path.exists() && !force {
        eprintln!(
            "{} AIDA is already initialized in this directory (requirements.db exists).",
            "!".yellow()
        );
        eprintln!("  Use {} to reinitialize.", "--force".bold());
        eprintln!(
            "  To refresh just the scaffolding (CLAUDE.md, .claude/skills/, hooks),"
        );
        eprintln!("  use `aida scaffold apply --force` instead — preserves your store.");
        return Ok(());
    }

    // --force on a populated SQLite store would lose every requirement. Count
    // first; require the user to type "reset" to confirm if non-empty.
    if force && db_path.exists() {
        if let Ok(count) = count_requirements_in_sqlite(&db_path) {
            if count > 0 && !confirm_destructive_reset(count, &db_path)? {
                return Ok(());
            }
        }
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

    // Run the shared workflow scaffolding (skills, hooks, mcp, codex).
    let root = std::env::current_dir().unwrap_or_default();
    complete_init_scaffolding(
        &root,
        &store,
        agent,
        no_skills,
        no_hooks,
        force,
        db_path.clone(),
        "Requirements database (SQLite)",
    )
}

/// Workflow scaffolding shared by all `aida init` modes — builds skills,
/// commands, hooks, MCP integration, etc. Called by both centralized and
/// distributed init paths after their respective storage setup is complete.
/// trace:EPIC-1-001 | ai:claude
#[allow(clippy::too_many_arguments)]
fn complete_init_scaffolding(
    root: &std::path::Path,
    store: &RequirementsStore,
    agent: &str,
    no_skills: bool,
    no_hooks: bool,
    force: bool,
    db_path: std::path::PathBuf,
    storage_label: &str,
) -> Result<()> {
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
    let config_for_output = config.clone();
    let mut scaffolder = Scaffolder::with_database(root.to_path_buf(), config, db_path);
    let preview = scaffolder.preview(store);

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

    // Auto-configure Codex MCP if codex is installed
    if std::process::Command::new("codex")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        // Check if already configured
        let mcp_list = std::process::Command::new("codex")
            .args(["mcp", "list"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();

        if !mcp_list.contains("aida") {
            match std::process::Command::new("codex")
                .args(["mcp", "add", "aida", "--", "aida", "mcp-serve"])
                .output()
            {
                Ok(o) if o.status.success() => {
                    println!("{} Codex CLI MCP server configured", "✓".green());
                }
                _ => {
                    // Silently skip — codex mcp add may fail for various reasons
                }
            }
        }
    }

    // Print post-init message
    println!();
    println!("{}", "AIDA initialized ✓".green().bold());
    println!();
    println!("  {}:", "Created".bold());
    println!("    {}", storage_label);
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

/// Detect if the current directory has a distributed store configured.
/// Walks up from CWD looking for `.aida/config.toml` with a store_path.
fn detect_distributed_store() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let config_path = cwd.join(".aida").join("config.toml");

    if !config_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&config_path).ok()?;

    // Parse store_path from config
    // Format: store_path = "aida-store"
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("store_path") {
            if let Some(val) = line.split('=').nth(1) {
                let val = val.trim().trim_matches('"').trim_matches('\'');
                let store_path = cwd.join(val);
                if store_path.exists() && store_path.is_dir() {
                    return Some(store_path);
                }
            }
        }
    }

    None
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
    let inner = aida_core::GitBackend::new(store_path)?
        .with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    match command {
        Command::Cache(cache_cmd) => {
            return handle_cache_command(cache_cmd, &backend);
        }
        Command::Status { no_dev_context } => {
            return handle_status_command_distributed(*no_dev_context, store_path, &backend);
        }
        Command::Upgrade { .. } => unreachable!("upgrade is dispatched before storage init"),
        Command::Dev(_) => unreachable!("dev is dispatched before storage init"),
        Command::HelpAll => unreachable!("help-all is dispatched before storage init"),
        Command::Role(_) => unreachable!("role is dispatched before storage init"),
        Command::Statusline => unreachable!("statusline is dispatched before storage init"),
        Command::List { status, r#type, feature, .. } => {
            // Cache-backed list (EPIC-1-001 Phase 2). The CachedGitBackend
            // ensures the cache is fresh before querying, so this is one
            // SQLite query instead of ~360 YAML reads.
            // trace:EPIC-1-001 | ai:claude
            let filter = aida_core::ListFilter {
                status: status.clone(),
                req_type: r#type.clone(),
                feature: feature.clone(),
                ..Default::default()
            };
            let reqs = backend.list_summaries(&filter)?;

            if reqs.is_empty() {
                println!("No requirements found.");
            } else {
                let has_agreed = reqs.iter().any(|r| r.agreed_id.is_some());

                if has_agreed {
                    println!(
                        "{:<12} {:<14} {:<12} {:<10} {}",
                        "ID", "Node ID", "Type", "Status", "Title"
                    );
                    println!("{}", "─".repeat(78));
                    for req in &reqs {
                        let display_id = req
                            .agreed_id
                            .as_deref()
                            .or(req.spec_id.as_deref())
                            .unwrap_or("?");
                        println!(
                            "{:<12} {:<14} {:<12} {:<10} {}",
                            display_id,
                            req.spec_id.as_deref().unwrap_or("-"),
                            req.req_type,
                            req.status,
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
                        let display_id = req
                            .spec_id
                            .as_deref()
                            .unwrap_or("?");
                        println!(
                            "{:<14} {:<12} {:<10} {:<10} {}",
                            display_id,
                            req.req_type,
                            req.status,
                            req.priority,
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
                if let Some(sid) = last.spec_id.as_deref() {
                    record_role_activity(sid, "add");
                }
            }
        }
        Command::Show { id } => {
            record_role_activity(id, "show");
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
            record_role_activity(id, "edit");
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
            // Cache-backed FTS5 search (EPIC-1-001 Phase 2). Replaces a
            // full-store load + in-memory substring scan.
            // trace:EPIC-1-001 | ai:claude
            let mut results = backend.search(query, 200)?;
            if let Some(s) = status {
                let needle = s.clone();
                results.retain(|r| r.status.eq_ignore_ascii_case(&needle));
            }

            if results.is_empty() {
                println!("No results for: {}", query);
            } else {
                println!(
                    "{:<14} {:<12} {:<10} {}",
                    "ID", "Type", "Status", "Title"
                );
                println!("{}", "─".repeat(65));
                for req in &results {
                    let display_id = req
                        .agreed_id
                        .as_deref()
                        .or(req.spec_id.as_deref())
                        .unwrap_or("?");
                    println!(
                        "{:<14} {:<12} {:<10} {}",
                        display_id,
                        req.req_type,
                        req.status,
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
            record_role_activity(req_id, "comment");
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
                // Snapshot local state before pull for conflict detection
                let local_reqs = backend.load()
                    .map(|s| s.requirements)
                    .unwrap_or_default();

                println!("Pulling from origin/{}...", branch);
                match aida_core::git_ops::pull(store_path, "origin", &branch) {
                    Ok(()) => {
                        println!("  Pull complete.");

                        // Detect conflicts with remote changes
                        let remote_reqs = backend.load()
                            .map(|s| s.requirements)
                            .unwrap_or_default();

                        let conflicts = aida_core::conflict::detect_store_conflicts(
                            &local_reqs,
                            &remote_reqs,
                        );

                        if !conflicts.is_empty() {
                            println!();
                            println!("{}", "Conflicts detected:".red().bold());
                            for conflict in &conflicts {
                                println!();
                                print!("{}", conflict);
                            }
                            println!();
                            println!(
                                "Resolve with: aida edit <ID> --title/--status/... to pick the version you want."
                            );
                        }
                    }
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
            } else if !*pull {
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
                println!("  aida db sync --pull --push");
            }
        }

        // Status
        Command::Db(DbCommand::Status) => {
            let store = backend.load()?;

            let total = store.requirements.len();
            let with_agreed = store.requirements.iter()
                .filter(|r| r.agreed_id.is_some())
                .count();
            let without_agreed = total - with_agreed;

            println!("{}", "Distributed Store Status".bold());
            println!("{}", "─".repeat(45));
            println!("{:<20} {}", "Path:", store_path.display());
            println!("{:<20} {}", "Requirements:", total);
            println!("{:<20} {}", "With agreed ID:", with_agreed);
            println!("{:<20} {}", "Pending merge-gate:", without_agreed);

            if aida_core::git_ops::is_git_repo(store_path) {
                let has_changes = aida_core::git_ops::has_changes(store_path).unwrap_or(false);
                let head = aida_core::git_ops::head_sha(store_path).unwrap_or_else(|_| "?".into());
                let branch = aida_core::git_ops::current_branch(store_path).unwrap_or_else(|_| "?".into());

                println!("{:<20} {}", "Branch:", branch);
                println!("{:<20} {}", "HEAD:", head);
                println!("{:<20} {}", "Working tree:", if has_changes { "uncommitted changes" } else { "clean" });

                let remote_ok = aida_core::git_ops::is_remote_reachable(store_path, "origin");
                println!("{:<20} {}", "Remote:", if remote_ok { "reachable" } else { "not configured or unreachable" });
            }

            // Show dispenser state
            if let Ok(disp) = load_dispenser(store_path) {
                if let Ok(state) = disp.state() {
                    let mode_str = match &state.mode {
                        aida_core::IdMode::Centralized => "centralized".to_string(),
                        aida_core::IdMode::Distributed { node_id } => format!("distributed (node {})", node_id),
                    };
                    println!("{:<20} {}", "ID mode:", mode_str);
                    let total_dispensed: u32 = state.sequences.values().sum();
                    println!("{:<20} {}", "IDs dispensed:", total_dispensed);
                }
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

        Command::Queue(queue_cmd) => {
            // Reuse the legacy Storage handler — it works against any
            // DatabaseBackend via Storage trait shims, and our GitBackend
            // already implements queue_list/add/remove/reorder/clear.
            // Wrap our backend in a Storage façade pointing at the store path.
            let storage = Storage::new(store_path);
            handle_queue_command(queue_cmd, &storage)?;
        }
        Command::Scaffold(scaffold_cmd) => {
            // Scaffold apply / status / preview / extract — same pattern.
            // Storage façade now handles directory paths via GitBackend.load().
            let storage = Storage::new(store_path);
            handle_scaffold_command(scaffold_cmd, &storage, store_path)?;
        }
        _ => {
            eprintln!(
                "Command not yet supported for git backend.\n\
                 Supported: list, add, show, edit, del, search, comment add,\n\
                 queue list/add/remove/move/clear,\n\
                 rel add/remove, db info/status/sync/merge-gate/export-git/workspace-init"
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

/// Initialize distributed mode using an orphan branch + worktree.
/// This is the default for single-repo projects.
/// Store lives at .aida-store/ (worktree of orphan branch 'aida-store').
fn handle_init_distributed_worktree(
    force: bool,
    no_skills: bool,
    agent: &str,
    no_hooks: bool,
) -> Result<()> {
    use aida_core::git_ops;

    let cwd = std::env::current_dir()?;
    let aida_dir = cwd.join(".aida");
    let worktree_dir = ".aida-store";
    let branch_name = "aida-store";

    // Must be inside a git repo
    if !git_ops::is_git_repo(&cwd) {
        anyhow::bail!(
            "Not a git repository. Run 'git init' first, or use --sibling for a separate repo."
        );
    }

    // Check if already initialized
    if aida_dir.join("config.toml").exists() && !force {
        eprintln!(
            "{} AIDA distributed mode is already initialized (.aida/config.toml exists).",
            "!".yellow()
        );
        eprintln!("  Use {} to reinitialize.", "--force".bold());
        eprintln!(
            "  To refresh just the scaffolding (CLAUDE.md, .claude/skills/, hooks),"
        );
        eprintln!("  use `aida scaffold apply --force` instead — preserves your store.");
        return Ok(());
    }

    // --force on a populated store would silently wipe N requirements and reseed
    // an empty store with just META prompts. Surface that loudly: count what
    // would be lost and require the user to type "reset" to acknowledge.
    if force {
        let existing_store = cwd.join(worktree_dir);
        if let Some(count) = count_requirements_in_store(&existing_store) {
            if count > 0 && !confirm_destructive_reset(count, &existing_store)? {
                return Ok(());
            }
        }
    }

    println!("{}", "Initializing AIDA in distributed mode (orphan branch + worktree)...".bold());
    println!();

    // Ensure there's at least one commit on main (worktree requires it)
    let has_commits = git_ops::head_sha(&cwd).is_ok();
    if !has_commits {
        // Create an initial commit so worktree can be added
        std::fs::write(cwd.join(".gitkeep"), "")?;
        git_ops::add(&cwd, &[".gitkeep"])?;

        let git_name = git_ops::git_config_get("user.name")
            .unwrap_or_else(|_| "AIDA User".to_string());
        let git_email = git_ops::git_config_get("user.email")
            .unwrap_or_else(|_| "aida@localhost".to_string());
        git_ops::configure_user(&cwd, &git_name, &git_email)?;
        git_ops::commit(&cwd, "chore: initial commit")?;
    }

    // Create orphan branch + worktree
    let store_path = git_ops::create_store_worktree(&cwd, worktree_dir, branch_name)?;
    println!(
        "  {} orphan branch '{}' with worktree at {}",
        "Created".green(),
        branch_name,
        worktree_dir
    );

    // Configure git user in worktree
    let git_name = git_ops::git_config_get("user.name")
        .unwrap_or_else(|_| "AIDA User".to_string());
    let git_email = git_ops::git_config_get("user.email")
        .unwrap_or_else(|_| "aida@localhost".to_string());
    git_ops::configure_user(&store_path, &git_name, &git_email)?;

    // Initialize the git backend
    let backend = aida_core::GitBackend::new(&store_path)?;
    let mut store = aida_core::models::RequirementsStore::new();
    seed_meta_requirements(&mut store)?;
    backend.save(&store)?;
    println!(
        "  {} {}",
        "Created".green(),
        format!("{}/metadata.yaml", worktree_dir).white().bold()
    );

    // Create initial commit on the orphan branch
    git_ops::add(&store_path, &["metadata.yaml"])?;
    std::fs::create_dir_all(store_path.join("objects"))?;
    std::fs::write(store_path.join("objects/.gitkeep"), "")?;
    git_ops::add(&store_path, &["objects/.gitkeep"])?;
    git_ops::add(&store_path, &["objects"])?;
    git_ops::commit(&store_path, "chore: initialize AIDA distributed store")?;

    // Add .aida-store to .gitignore on main branch
    let gitignore_path = cwd.join(".gitignore");
    let gitignore_entry = format!("\n# AIDA distributed store (orphan branch worktree)\n{}/\n", worktree_dir);
    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        if !content.contains(worktree_dir) {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&gitignore_path)?;
            use std::io::Write;
            file.write_all(gitignore_entry.as_bytes())?;
        }
    } else {
        std::fs::write(&gitignore_path, gitignore_entry)?;
    }

    // Create .aida/config.toml
    std::fs::create_dir_all(&aida_dir)?;
    let config_content = format!(
        "# AIDA distributed mode configuration\n\
         [deployment]\n\
         mode = \"distributed\"\n\
         store_path = \"{}\"\n\
         store_type = \"worktree\"\n\
         branch = \"{}\"\n",
        worktree_dir, branch_name
    );
    std::fs::write(aida_dir.join("config.toml"), &config_content)?;

    // Create docs/plans/ for plan archive (per CLAUDE.md convention).
    std::fs::create_dir_all(cwd.join("docs/plans"))?;

    // Run the shared workflow scaffolding (skills, hooks, mcp, codex).
    let storage_label = format!(
        "{}{}Git-canonical store ({}, orphan branch '{}')",
        worktree_dir.white().bold(),
        " ".repeat(20),
        worktree_dir,
        branch_name
    );
    complete_init_scaffolding(
        &cwd,
        &store,
        agent,
        no_skills,
        no_hooks,
        force,
        std::path::PathBuf::from(worktree_dir),
        &storage_label,
    )?;

    println!();
    println!("  {}:", "Sync store to remote".bold());
    println!("    {}", format!("git push origin {}", branch_name).cyan());
    println!();
    println!("  {}:", "New developer setup".bold());
    println!("    {}", "git clone <repo>".cyan());
    println!(
        "    {}",
        format!("git worktree add {} {}", worktree_dir, branch_name).cyan()
    );
    println!();

    Ok(())
}

/// Initialize distributed mode using a sibling repo.
/// For multi-repo workspaces where multiple code repos share one store.
fn handle_init_distributed_sibling(
    registry_remote: Option<&str>,
    force: bool,
    no_skills: bool,
    agent: &str,
    no_hooks: bool,
) -> Result<()> {
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
        eprintln!(
            "  To refresh just the scaffolding (CLAUDE.md, .claude/skills/, hooks),"
        );
        eprintln!("  use `aida scaffold apply --force` instead — preserves your store.");
        return Ok(());
    }

    // --force on a populated store would silently wipe N requirements. Same
    // guard as the worktree path: count + require typed confirmation.
    if force {
        if let Some(count) = count_requirements_in_store(&store_dir) {
            if count > 0 && !confirm_destructive_reset(count, &store_dir)? {
                return Ok(());
            }
        }
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
    let mut store = aida_core::models::RequirementsStore::new();
    seed_meta_requirements(&mut store)?;
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

    // Create docs/plans/ for plan archive (per CLAUDE.md convention).
    std::fs::create_dir_all(cwd.join("docs/plans"))?;

    // Run the shared workflow scaffolding (skills, hooks, mcp, codex).
    let storage_label = format!(
        "{}{}Git-canonical store (sibling repo at ../aida-store/)",
        "aida-store/".white().bold(),
        " ".repeat(20)
    );
    complete_init_scaffolding(
        &cwd,
        &store,
        agent,
        no_skills,
        no_hooks,
        force,
        std::path::PathBuf::from("aida-store"),
        &storage_label,
    )?;

    println!();
    println!("  {}:", "Sync store to remote".bold());
    println!("    {}", "cd aida-store && git push -u origin main".cyan());
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

/// Handle `aida cache {rebuild,status}` against a CachedGitBackend.
/// trace:EPIC-1-001 | ai:claude
fn handle_cache_command(
    cmd: &CacheCommand,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    use aida_core::DatabaseBackend;

    match cmd {
        CacheCommand::Rebuild => {
            let n = backend.rebuild_cache()?;
            println!(
                "{}: Cache rebuilt. {} requirement(s) projected from git store at {}.",
                "OK".green(),
                n,
                backend.cache().path().display()
            );
        }
        CacheCommand::Status => {
            let cache = backend.cache();
            let recorded_sha = cache.source_head_sha()?.unwrap_or_default();
            let actual_sha = aida_core::git_ops::head_sha(backend.path()).unwrap_or_default();
            let count = cache.requirement_count()?;
            let built_at = cache.built_at()?.unwrap_or_else(|| "(never)".into());
            let store_count = backend.list_requirements(true)?.len();

            println!("Cache path:       {}", cache.path().display());
            println!("Cached requirements: {}", count);
            println!("Store requirements:  {}", store_count);
            println!("Last built:       {}", built_at);
            println!(
                "Cache HEAD SHA:   {}",
                if recorded_sha.is_empty() {
                    "(none)".to_string()
                } else {
                    recorded_sha.clone()
                }
            );
            println!(
                "Store HEAD SHA:   {}",
                if actual_sha.is_empty() {
                    "(no git head — non-git store?)".to_string()
                } else {
                    actual_sha.clone()
                }
            );
            let stale = recorded_sha != actual_sha || recorded_sha.is_empty();
            if stale && !actual_sha.is_empty() {
                println!("Status:           {} — run `aida cache rebuild`", "STALE".yellow());
            } else {
                println!("Status:           {}", "FRESH".green());
            }
        }
    }
    Ok(())
}

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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RoleState {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    purpose: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_active_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    working_directory: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    notes: Option<String>,

    /// True if this role file lives in ~/.aida/roles/ rather than per-project.
    /// Persisted so `aida role list` can mark global roles distinctly without
    /// re-checking the filesystem location.
    #[serde(default)]
    global: bool,

    /// Last N requirements touched while this role was active. Newest first.
    /// Bounded at ACTIVITY_MAX entries; older entries fall off the end.
    #[serde(default)]
    activity: Vec<RoleActivity>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct RoleActivity {
    /// Requirement spec_id (or agreed_id) touched
    spec_id: String,
    /// What the user did: "show", "edit", "add", "comment"
    action: String,
    at: chrono::DateTime<chrono::Utc>,
}

const ACTIVITY_MAX: usize = 10;

fn statusline_project_root() -> std::path::PathBuf {
    // Roles + statusline live in the project that is the user's CWD
    // (or any ancestor with `.aida/config.toml`). Falls back to CWD if
    // no marker is found.
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let mut probe = cwd.clone();
    for _ in 0..8 {
        if probe.join(".aida").join("config.toml").exists() {
            return probe;
        }
        match probe.parent() {
            Some(p) => probe = p.to_path_buf(),
            None => break,
        }
    }
    cwd
}

/// Per-project role storage: <project>/.aida/roles/
fn project_roles_dir(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida/roles")
}

/// Global role storage: ~/.aida/roles/ — for personas you carry across
/// projects (e.g., "triage", "code-review").
fn global_roles_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".aida/roles"))
}

fn project_role_file(project_root: &std::path::Path, name: &str) -> std::path::PathBuf {
    project_roles_dir(project_root).join(format!("{}.toml", name))
}

fn global_role_file(name: &str) -> Option<std::path::PathBuf> {
    global_roles_dir().map(|d| d.join(format!("{}.toml", name)))
}

/// Load a role by name. Looks in the project first, then the global dir.
/// Returns the state plus the path it was loaded from (for save-back).
fn load_role(
    project_root: &std::path::Path,
    name: &str,
) -> Result<(RoleState, std::path::PathBuf)> {
    let project_path = project_role_file(project_root, name);
    if project_path.exists() {
        let content = std::fs::read_to_string(&project_path)
            .with_context(|| format!("Failed to read role file {}", project_path.display()))?;
        let state: RoleState = toml::from_str(&content)
            .with_context(|| format!("Failed to parse role file {}", project_path.display()))?;
        return Ok((state, project_path));
    }
    if let Some(global_path) = global_role_file(name) {
        if global_path.exists() {
            let content = std::fs::read_to_string(&global_path)
                .with_context(|| format!("Failed to read role file {}", global_path.display()))?;
            let state: RoleState = toml::from_str(&content)
                .with_context(|| format!("Failed to parse role file {}", global_path.display()))?;
            return Ok((state, global_path));
        }
    }
    anyhow::bail!(
        "No such role: {} (looked at {} and {})",
        name,
        project_path.display(),
        global_role_file(name)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(no global dir)".into())
    )
}

/// Save back to the same location the role was loaded from (or the
/// project / global location based on `state.global` for fresh roles).
fn save_role_at(state: &RoleState, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(state)?;
    std::fs::write(path, content)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn role_save_path(project_root: &std::path::Path, state: &RoleState) -> Result<std::path::PathBuf> {
    if state.global {
        global_role_file(&state.name)
            .ok_or_else(|| anyhow::anyhow!("Cannot determine $HOME for global role storage"))
    } else {
        Ok(project_role_file(project_root, &state.name))
    }
}

fn list_roles(project_root: &std::path::Path) -> Result<Vec<RoleState>> {
    let mut roles = Vec::new();
    for dir in [Some(project_roles_dir(project_root)), global_roles_dir()]
        .into_iter()
        .flatten()
    {
        if !dir.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension() == Some(std::ffi::OsStr::new("toml")) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(state) = toml::from_str::<RoleState>(&content) {
                        roles.push(state);
                    }
                }
            }
        }
    }
    roles.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
    Ok(roles)
}

/// Append an activity entry to the active role's log (best-effort; silently
/// no-op if no role active or the role file is unwriteable). Called from
/// the show/edit/add/comment paths so resuming a role surfaces what the
/// user was working on last.
fn record_role_activity(spec_id: &str, action: &str) {
    let role_name = match std::env::var("AIDA_SESSION_ROLE") {
        Ok(n) if !n.is_empty() => n,
        _ => return,
    };
    let project = match std::env::var("AIDA_SESSION_PROJECT") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => statusline_project_root(),
    };
    let (mut state, path) = match load_role(&project, &role_name) {
        Ok(t) => t,
        Err(_) => return,
    };
    // Dedupe consecutive entries on the same spec_id+action.
    let entry = RoleActivity {
        spec_id: spec_id.to_string(),
        action: action.to_string(),
        at: chrono::Utc::now(),
    };
    let dup = state
        .activity
        .first()
        .map(|prev| prev.spec_id == entry.spec_id && prev.action == entry.action)
        .unwrap_or(false);
    if !dup {
        state.activity.insert(0, entry);
        state.activity.truncate(ACTIVITY_MAX);
    } else if let Some(first) = state.activity.first_mut() {
        first.at = entry.at;
    }
    state.last_active_at = chrono::Utc::now();
    let _ = save_role_at(&state, &path);
}

fn handle_role_command(cmd: &RoleCommand) -> Result<()> {
    let project_root = statusline_project_root();
    match cmd {
        RoleCommand::Enter { name, cd } => handle_role_enter(&project_root, name, *cd),
        RoleCommand::Add {
            name,
            purpose,
            global,
        } => handle_role_add(&project_root, name, purpose.as_deref(), *global),
        RoleCommand::List => handle_role_list(&project_root),
        RoleCommand::Show { name } => handle_role_show(&project_root, name.as_deref()),
        RoleCommand::End => handle_role_end(),
        RoleCommand::Delete { name, yes } => handle_role_delete(&project_root, name, *yes),
        RoleCommand::Scaffold => handle_role_scaffold(),
    }
}

fn handle_role_enter(
    project_root: &std::path::Path,
    name: &str,
    cd: bool,
) -> Result<()> {
    let (mut state, _) = load_role(project_root, name).map_err(|_| {
        anyhow::anyhow!(
            "No such role: {}\n\
             Create it with: `aida role add {}`\n\
             See available roles with: `aida role list`",
            name,
            name
        )
    })?;
    state.last_active_at = chrono::Utc::now();
    state.working_directory = std::env::current_dir().ok();
    let save_path = role_save_path(project_root, &state)?;
    save_role_at(&state, &save_path)?;
    emit_role_enter_eval(project_root, &state, cd, /* was_existing */ true);
    Ok(())
}

fn handle_role_add(
    project_root: &std::path::Path,
    name: &str,
    purpose: Option<&str>,
    global: bool,
) -> Result<()> {
    if let Ok((existing, path)) = load_role(project_root, name) {
        anyhow::bail!(
            "Role '{}' already exists at {}.\n\
             Resume it with: `aida role enter {}`\n\
             See its details with: `aida role show {}`\n\
             ({})",
            name,
            path.display(),
            name,
            name,
            if existing.global {
                "currently a global role"
            } else {
                "currently a project role"
            }
        );
    }
    let state = RoleState {
        name: name.to_string(),
        purpose: purpose.map(String::from),
        created_at: chrono::Utc::now(),
        last_active_at: chrono::Utc::now(),
        working_directory: std::env::current_dir().ok(),
        notes: None,
        global,
        activity: Vec::new(),
    };
    let save_path = role_save_path(project_root, &state)?;
    save_role_at(&state, &save_path)?;
    emit_role_enter_eval(project_root, &state, /* cd */ false, /* was_existing */ false);
    Ok(())
}

fn emit_role_enter_eval(
    project_root: &std::path::Path,
    state: &RoleState,
    cd: bool,
    was_existing: bool,
) {
    // Emit shell code for eval. The `aida()` shell wrapper installed by
    // `aida dev shell-init --install` automatically eval's our stdout for
    // role enter / dev activate / etc., so direct invocation Just Works in
    // an interactive shell. Scripts can still pipe to `eval "$(...)"`.
    let cwd = state
        .working_directory
        .as_deref()
        .unwrap_or(project_root)
        .display();
    println!("# aida role enter — {}", state.name);
    println!("export AIDA_SESSION_ROLE='{}'", state.name);
    if let Some(p) = &state.purpose {
        println!("export AIDA_SESSION_PURPOSE='{}'", p.replace('\'', "'\\''"));
    } else {
        println!("unset AIDA_SESSION_PURPOSE");
    }
    println!("export AIDA_SESSION_PROJECT='{}'", project_root.display());
    if cd {
        println!("cd '{}'", cwd);
    }
    let verb = if was_existing { "Resumed" } else { "Created and entered" };
    let scope = if state.global { " [global]" } else { "" };
    println!(
        "echo '✓ {} role: {}{}{}'",
        verb,
        state.name,
        scope,
        state
            .purpose
            .as_ref()
            .map(|p| format!(" — {}", p))
            .unwrap_or_default()
    );
    // Surface what the user was last working on under this role —
    // makes "resume" feel like a real session, not just a label switch.
    if was_existing && !state.activity.is_empty() {
        println!("echo ''");
        println!(
            "echo '  Last touched while in this role:'"
        );
        for entry in state.activity.iter().take(5) {
            let when = humanize_relative(entry.at);
            println!(
                "echo '    {} — {} ({})'",
                entry.spec_id, entry.action, when
            );
        }
    }
}

fn handle_role_end() -> Result<()> {
    // Use a uniquely-named env var rather than `local` so the eval works
    // both at the shell top level and inside a wrapper function.
    println!("# aida role end");
    println!("__AIDA_ROLE_END_PREV=\"${{AIDA_SESSION_ROLE:-}}\"");
    println!("unset AIDA_SESSION_ROLE AIDA_SESSION_PURPOSE AIDA_SESSION_PROJECT");
    println!("if [ -n \"$__AIDA_ROLE_END_PREV\" ]; then");
    println!("    echo \"✓ Deactivated role: $__AIDA_ROLE_END_PREV\"");
    println!("else");
    println!("    echo 'No role active.'");
    println!("fi");
    println!("unset __AIDA_ROLE_END_PREV");
    Ok(())
}

fn handle_role_list(project_root: &std::path::Path) -> Result<()> {
    let roles = list_roles(project_root)?;
    let active = std::env::var("AIDA_SESSION_ROLE").ok();
    if roles.is_empty() {
        println!("(no roles defined for {})", project_root.display());
        println!(
            "Create one with: {} {}",
            "aida role add".cyan(),
            "<name>".dimmed()
        );
        println!(
            "Or install a starter set: {}",
            "aida role scaffold".cyan()
        );
        return Ok(());
    }
    println!("Roles for {}:", project_root.display());
    for role in &roles {
        let marker = if active.as_deref() == Some(&role.name) {
            "*".green().to_string()
        } else {
            " ".to_string()
        };
        let scope = if role.global {
            " [global]".dimmed().to_string()
        } else {
            String::new()
        };
        let last = humanize_relative(role.last_active_at);
        let purpose = role
            .purpose
            .as_deref()
            .map(|p| format!(" — {}", p))
            .unwrap_or_default();
        println!(
            "  {} {:<16}{} last active {}{}",
            marker,
            role.name.bold(),
            scope,
            last,
            purpose
        );
    }
    Ok(())
}

fn handle_role_show(project_root: &std::path::Path, name: Option<&str>) -> Result<()> {
    let resolved = match name {
        Some(n) => n.to_string(),
        None => std::env::var("AIDA_SESSION_ROLE").map_err(|_| {
            anyhow::anyhow!("No role active and no name given. Use `aida role list` to see options.")
        })?,
    };
    let (state, path) = load_role(project_root, &resolved)?;
    println!("Role:        {}{}", state.name.bold(), if state.global { " [global]".dimmed().to_string() } else { String::new() });
    println!("Stored at:   {}", path.display());
    println!(
        "Purpose:     {}",
        state.purpose.as_deref().unwrap_or("(none)")
    );
    println!("Created:     {}", state.created_at.to_rfc3339());
    println!(
        "Last active: {} ({})",
        state.last_active_at.to_rfc3339(),
        humanize_relative(state.last_active_at)
    );
    if let Some(d) = &state.working_directory {
        println!("Last cwd:    {}", d.display());
    }
    if let Some(n) = &state.notes {
        println!("Notes:       {}", n);
    }
    if !state.activity.is_empty() {
        println!();
        println!("Recent activity (newest first):");
        for entry in &state.activity {
            println!(
                "  {:<14} {:<10} {}",
                entry.spec_id,
                entry.action,
                humanize_relative(entry.at)
            );
        }
    }
    Ok(())
}

fn handle_role_delete(project_root: &std::path::Path, name: &str, yes: bool) -> Result<()> {
    let (state, path) = load_role(project_root, name)?;
    let scope = if state.global { " [global]" } else { "" };
    if !yes {
        eprintln!(
            "Delete role '{}{}'? (purpose: {}, last active: {})",
            name,
            scope,
            state.purpose.as_deref().unwrap_or("(none)"),
            humanize_relative(state.last_active_at)
        );
        eprintln!("Type 'y' to confirm:");
        let mut answer = String::new();
        if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
            eprintln!("Cancelled.");
            return Ok(());
        }
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            eprintln!("Cancelled.");
            return Ok(());
        }
    }
    std::fs::remove_file(&path)?;
    println!("{}: deleted role '{}' ({})", "OK".green(), name, path.display());
    Ok(())
}

/// Starter role set installed by `aida role scaffold`. Idempotent — skips
/// any name that already exists (anywhere). All starter roles are global
/// since they're meant to apply across projects.
const STARTER_ROLES: &[(&str, &str)] = &[
    (
        "dialog",
        "Captain / customer / PO hat. Chat with the agent, capture requirements as they emerge, route work to doer roles via `aida queue add --for <role>`. Driver, not implementer.",
    ),
    (
        "triage",
        "Process the backlog: review drafts, close stale items, group related work.",
    ),
    (
        "architect",
        "Design work: explore tradeoffs, write/review plans, capture decisions in docs/plans/.",
    ),
    (
        "implementer",
        "Heads-down coding on a specific feature or fix. Drive a requirement to completed.",
    ),
    (
        "reviewer",
        "Code/PR review. Walk diffs, check trace comments, verify against requirements.",
    ),
];

fn handle_role_scaffold() -> Result<()> {
    let project_root = statusline_project_root();
    let mut created = 0usize;
    let mut skipped = 0usize;
    println!("Installing starter global roles at ~/.aida/roles/");
    println!();
    for (name, purpose) in STARTER_ROLES {
        if load_role(&project_root, name).is_ok() {
            println!("  {} {} (already exists, skipped)", "~".yellow(), name);
            skipped += 1;
            continue;
        }
        let state = RoleState {
            name: (*name).to_string(),
            purpose: Some((*purpose).to_string()),
            created_at: chrono::Utc::now(),
            last_active_at: chrono::Utc::now(),
            working_directory: None,
            notes: None,
            global: true,
            activity: Vec::new(),
        };
        let path = role_save_path(&project_root, &state)?;
        save_role_at(&state, &path)?;
        println!("  {} {} — {}", "+".green(), name, purpose);
        created += 1;
    }
    println!();
    if created == 0 {
        println!(
            "{}: all {} starter role(s) already exist — nothing to do.",
            "OK".green(),
            skipped
        );
    } else {
        println!(
            "{}: scaffolded {} role(s){}.",
            "OK".green(),
            created,
            if skipped > 0 {
                format!(" ({} already existed)", skipped)
            } else {
                String::new()
            }
        );
        println!();
        println!("Try them: {}", "aida role enter triage".cyan());
        println!("List all: {}", "aida role list".cyan());
    }
    Ok(())
}

fn humanize_relative(t: chrono::DateTime<chrono::Utc>) -> String {
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

fn handle_statusline_command() -> Result<()> {
    let project_root = statusline_project_root();
    let project_name = project_root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "?".into());

    // Try to read project name from store metadata (fall back to dir name).
    let store_name = std::fs::read_to_string(project_root.join("aida-store/metadata.yaml"))
        .or_else(|_| std::fs::read_to_string(project_root.join(".aida-store/metadata.yaml")))
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find_map(|l| l.strip_prefix("name:").map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string()))
                .filter(|s| !s.is_empty())
        });
    let project_label = store_name.unwrap_or(project_name);

    let role = std::env::var("AIDA_SESSION_ROLE").ok();

    // Cache stats — fast SQLite lookups, no rebuild.
    let cache_path = project_root.join(".aida/cache.db");
    let (req_count, cache_label) = if cache_path.exists() {
        match rusqlite::Connection::open_with_flags(
            &cache_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(conn) => {
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM requirements_cache WHERE archived = 0",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                let recorded_sha: Option<String> = conn
                    .query_row(
                        "SELECT value FROM cache_meta WHERE key = 'source_head_sha'",
                        [],
                        |r| r.get(0),
                    )
                    .ok();
                let actual_sha = aida_core::git_ops::head_sha(
                    &project_root.join(".aida-store"),
                )
                .ok()
                .or_else(|| {
                    aida_core::git_ops::head_sha(&project_root.join("aida-store")).ok()
                })
                .unwrap_or_default();
                let stale = recorded_sha.as_deref().map(|s| s != actual_sha).unwrap_or(true);
                let label = if !actual_sha.is_empty() && stale {
                    "stale"
                } else {
                    "fresh"
                };
                (Some(count as usize), Some(label))
            }
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };

    let mut parts: Vec<String> = vec!["aida".to_string(), project_label];
    if let Some(r) = &role {
        parts.push(format!("role:{}", r));

        // If the active role has a recent activity entry, surface the
        // newest spec_id so the prompt reminds you what you were working on.
        // Read fast: TOML file, no extra dependencies.
        if let Ok((state, _)) = load_role(&project_root, r) {
            if let Some(latest) = state.activity.first() {
                parts.push(format!("@{}", latest.spec_id));
            }
        }
    }
    if let Some(c) = req_count {
        parts.push(format!("reqs:{}", c));
    }
    if let Some(l) = cache_label {
        parts.push(format!("cache:{}", l));
    }
    println!("{}", parts.join(" · "));
    Ok(())
}

fn print_help_all() {
    let groups: &[(&str, &[(&str, &str)])] = &[
        (
            "Daily use",
            &[
                ("add",     "Add a new requirement"),
                ("list",    "List all requirements"),
                ("show",    "Show details for a specific requirement"),
                ("edit",    "Edit an existing requirement"),
                ("del",     "Delete a requirement"),
                ("search",  "Simple search for requirements (case-insensitive)"),
                ("comment", "Manage comments on requirements"),
                ("status",  "Show this project's status (storage, counts, sync, recent activity)"),
            ],
        ),
        (
            "Project lifecycle",
            &[
                ("init",     "Initialize AIDA in the current project"),
                ("upgrade",  "Upgrade aida to the latest release"),
                ("scaffold", "Scaffolding management (skills, hooks, MCP)"),
            ],
        ),
        (
            "Requirements graph",
            &[
                ("rel",     "Relationship management commands"),
                ("rel-def", "Relationship-type definitions"),
                ("trace",   "Code-to-requirement traceability"),
                ("queue",   "Personal work queue"),
            ],
        ),
        (
            "Configuration & metadata",
            &[
                ("config",  "ID configuration (prefixes, formats, etc.)"),
                ("type",    "Requirement-type management"),
                ("feature", "Feature management"),
            ],
        ),
        (
            "Storage management",
            &[
                ("db",    "Database management commands (migrate, sync, merge-gate, etc.)"),
                ("cache", "SQLite cache view (rebuild, status) — git-canonical mode only"),
            ],
        ),
        (
            "Data exchange",
            &[
                ("export", "Export requirements to different formats"),
                ("import", "Import requirements from a tree JSON file"),
                ("grep",   "Advanced regex search across requirements"),
                ("report", "Report generation"),
            ],
        ),
        (
            "Integrations & servers",
            &[
                ("server",     "Connect to or manage a remote AIDA server"),
                ("mcp-serve",  "Start MCP server over stdio (for Claude Code)"),
                ("github",     "GitHub Issues integration"),
                ("gitlab",     "GitLab Issues integration"),
                ("jira",       "Jira integration"),
            ],
        ),
        (
            "AIDA development (working on aida itself)",
            &[
                ("dev",        "Activate dev binary, run dev servers, install shell helpers"),
                ("help-all",   "This command — full inventory grouped by topic"),
            ],
        ),
        (
            "Misc",
            &[
                ("user-guide", "Open the user guide in the default browser"),
            ],
        ),
    ];

    println!(
        "{}",
        "AIDA — full command inventory (run `aida <command> --help` for details)".bold()
    );
    println!();
    for (group, cmds) in groups {
        println!("{}", group.cyan().bold());
        for (name, desc) in *cmds {
            println!("  {:<14} {}", name.green(), desc);
        }
        println!();
    }
    println!(
        "Default `aida --help` shows only the daily-use subset. {}",
        "Tip:".bold()
    );
    println!("  - `aida <topic> --help` works for any command, even hidden ones");
    println!("  - `aida status` is the best entry point for \"what's going on here?\"");
    println!("  - `aida dev shell-init --install` to wire up the `aida` shell wrapper");
}

fn handle_dev_command(cmd: &DevCommand) -> Result<()> {
    match cmd {
        DevCommand::Activate { repo } => handle_dev_activate(repo.as_deref()),
        DevCommand::Deactivate => handle_dev_deactivate(),
        DevCommand::Status => handle_dev_status(),
        DevCommand::ShellInit { install } => handle_dev_shell_init(*install),
        DevCommand::Serve {
            rest_port,
            grpc_port,
            web_port,
            no_web,
        } => handle_dev_serve(*rest_port, *grpc_port, *web_port, *no_web),
        DevCommand::Release { bump } => handle_dev_release(bump),
        DevCommand::Patch => handle_dev_release("patch"),
    }
}

/// Locate an AIDA repo: prefer `--repo` arg, then $AIDA_DEV_REPO, then CWD
/// if it looks like one. Returns absolute path.
fn resolve_aida_repo(repo_arg: Option<&str>) -> Result<std::path::PathBuf> {
    // Track WHICH source we used so the error message can be specific about
    // what to fix.
    let (candidate, source): (std::path::PathBuf, &str) = if let Some(p) = repo_arg {
        (std::path::PathBuf::from(p), "--repo")
    } else if let Ok(p) = std::env::var("AIDA_DEV_REPO") {
        (std::path::PathBuf::from(p), "$AIDA_DEV_REPO")
    } else {
        (std::env::current_dir()?, "PWD")
    };

    let canonical = candidate.canonicalize().with_context(|| {
        format!("Cannot resolve AIDA repo path ({}): {}", source, candidate.display())
    })?;

    if !is_aida_repo(&canonical) {
        // Build a context-specific error. PWD-based failure is most often a
        // shell that hasn't picked up the AIDA_DEV_REPO export yet (the
        // `aida dev shell-init --install` flow writes it to .bashrc but
        // doesn't reload the current shell). Surface that fix prominently.
        let in_bashrc = dirs::home_dir()
            .map(|h| h.join(".bashrc"))
            .filter(|p| p.exists())
            .and_then(|p| std::fs::read_to_string(&p).ok())
            .map(|s| s.contains("AIDA_DEV_REPO"))
            .unwrap_or(false);

        let mut msg = format!(
            "Cannot locate the aida repo for activation:\n  \
             - {} ({}) is not a joemooney/aida checkout",
            source,
            canonical.display()
        );
        if source == "PWD" {
            msg.push_str("\n  - $AIDA_DEV_REPO is not set in this shell");
            if in_bashrc {
                msg.push_str(
                    "\n\n\
                     Your ~/.bashrc has the export, but this shell hasn't picked it up yet:\n  \
                       exec bash      (restart bash in place)\n  \
                       source ~/.bashrc",
                );
            } else {
                msg.push_str(
                    "\n\n\
                     One-time setup (from inside the aida repo):\n  \
                       aida dev shell-init --install\n  \
                       exec bash    (or: source ~/.bashrc)",
                );
            }
            msg.push_str(
                "\n\nOr pass it directly:\n  \
                 aida dev activate --repo /path/to/aida",
            );
        } else {
            msg.push_str(&format!(
                "\n\n\
                 Check that {} points at a real aida checkout (must contain a Cargo.toml \
                 with `repository = \"https://github.com/joemooney/aida\"`).",
                source
            ));
        }
        anyhow::bail!("{}", msg);
    }
    Ok(canonical)
}

/// Pick the freshest aida binary in the repo's target/. Prefers
/// `target/release/aida` when its mtime is newer than `target/debug/aida`,
/// else falls back to whichever exists. Errors when neither exists.
fn pick_dev_binary_dir(repo: &std::path::Path) -> Result<(std::path::PathBuf, &'static str)> {
    let release = repo.join("target/release/aida");
    let debug = repo.join("target/debug/aida");
    let release_mtime = std::fs::metadata(&release).and_then(|m| m.modified()).ok();
    let debug_mtime = std::fs::metadata(&debug).and_then(|m| m.modified()).ok();
    match (release_mtime, debug_mtime) {
        (Some(rm), Some(dm)) => {
            if rm >= dm {
                Ok((repo.join("target/release"), "release"))
            } else {
                Ok((repo.join("target/debug"), "debug"))
            }
        }
        (Some(_), None) => Ok((repo.join("target/release"), "release")),
        (None, Some(_)) => Ok((repo.join("target/debug"), "debug")),
        (None, None) => anyhow::bail!(
            "No aida binary found at {} or {}.\n\
             Run `cargo build --release` (or just `cargo build`) first.",
            release.display(),
            debug.display()
        ),
    }
}

fn handle_dev_activate(repo_arg: Option<&str>) -> Result<()> {
    let repo = resolve_aida_repo(repo_arg)?;
    let (bin_dir, profile) = pick_dev_binary_dir(&repo)?;

    // Quote-safety: paths shouldn't contain double-quotes in practice;
    // single-quote everything we emit so shell evaluation is safe.
    println!("# aida dev activate — using {} build at {}", profile, bin_dir.display());
    println!("export AIDA_DEV_REPO='{}'", repo.display());
    println!("export AIDA_DEV_BIN='{}'", bin_dir.display());
    println!("export AIDA_DEV_PROFILE='{}'", profile);
    println!("export AIDA_DEV_ACTIVE=1");
    println!("if [ -z \"${{AIDA_DEV_PREV_PATH+x}}\" ]; then");
    println!("    export AIDA_DEV_PREV_PATH=\"$PATH\"");
    println!("fi");
    println!("export PATH='{}':\"$PATH\"", bin_dir.display());
    println!("if [ -z \"${{AIDA_DEV_PREV_PS1+x}}\" ]; then");
    println!("    export AIDA_DEV_PREV_PS1=\"$PS1\"");
    println!("fi");
    println!("export PS1=\"(aida-{}) $PS1\"", profile);
    println!(
        "echo '✓ aida dev activated ({} build at {})'",
        profile,
        bin_dir.display()
    );
    Ok(())
}

fn handle_dev_deactivate() -> Result<()> {
    println!("# aida dev deactivate — restoring previous PATH and PS1");
    println!("if [ -n \"${{AIDA_DEV_PREV_PATH+x}}\" ]; then");
    println!("    export PATH=\"$AIDA_DEV_PREV_PATH\"");
    println!("    unset AIDA_DEV_PREV_PATH");
    println!("fi");
    println!("if [ -n \"${{AIDA_DEV_PREV_PS1+x}}\" ]; then");
    println!("    export PS1=\"$AIDA_DEV_PREV_PS1\"");
    println!("    unset AIDA_DEV_PREV_PS1");
    println!("fi");
    println!("unset AIDA_DEV_REPO AIDA_DEV_BIN AIDA_DEV_PROFILE AIDA_DEV_ACTIVE");
    println!("echo '✓ aida dev deactivated'");
    Ok(())
}

fn handle_dev_status() -> Result<()> {
    let active = std::env::var("AIDA_DEV_ACTIVE").is_ok();
    println!(
        "Activation:   {}",
        if active {
            "ACTIVE".green().to_string()
        } else {
            "(not active — `eval \"$(aida dev activate)\"` to enable)"
                .yellow()
                .to_string()
        }
    );
    if active {
        if let Ok(p) = std::env::var("AIDA_DEV_REPO") {
            println!("Repo:         {}", p);
        }
        if let Ok(b) = std::env::var("AIDA_DEV_BIN") {
            println!("Binary dir:   {}", b);
            let aida_path = std::path::PathBuf::from(&b).join("aida");
            if let Ok(meta) = std::fs::metadata(&aida_path) {
                if let Ok(modified) = meta.modified() {
                    if let Ok(d) = modified.duration_since(std::time::UNIX_EPOCH) {
                        let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, 0)
                            .map(|t| t.to_rfc3339())
                            .unwrap_or_else(|| "?".into());
                        println!("Built at:     {}", dt);
                    }
                }
            }
        }
        if let Ok(p) = std::env::var("AIDA_DEV_PROFILE") {
            println!("Build profile: {}", p);
        }
    }

    // Also report which `aida` actually wins on PATH right now.
    if let Ok(out) = std::process::Command::new("which").arg("aida").output() {
        let resolved = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !resolved.is_empty() {
            println!("`which aida`: {}", resolved);
        }
    }
    Ok(())
}

/// Shell helpers emitted by `aida dev shell-init`. A single `aida()` wrapper
/// function — pyenv/rbenv style. For most subcommands it just delegates to
/// the binary. For the handful of eval-only subcommands (dev activate, dev
/// deactivate, role enter, role end, role add — those that need to mutate
/// the calling shell), it wraps them in `eval "$(command aida ...)"` so
/// they actually take effect in the user's shell instead of getting lost
/// in the subprocess.
///
/// Use `command aida ...` to bypass the wrapper and invoke the binary
/// directly (e.g., for scripting where you want raw stdout).
const SHELL_HELPERS: &str = r#"# AIDA shell wrapper.
#
# Most `aida` subcommands run as plain commands. The few that need to
# modify the calling shell (set env vars, prepend PATH, change PS1) get
# automatically eval'd so they take effect here, not in the subprocess.
#
# Bypass the wrapper with `command aida ...` if you need raw stdout.
aida() {
    # Take the first two positional words verbatim — that's enough to
    # disambiguate every eval-required subcommand we have.
    local _aida_cmd="${1:-} ${2:-}"
    case "$_aida_cmd" in
        "dev activate"|"dev deactivate"|"role enter"|"role end"|"role add")
            eval "$(command aida "$@")"
            ;;
        *)
            command aida "$@"
            ;;
    esac
}
"#;

const HELPERS_BEGIN_MARKER: &str = "# >>> aida shell helpers >>>";
const HELPERS_END_MARKER: &str = "# <<< aida shell helpers <<<";

/// Old marker pair from before the helpers were split out into a separate
/// file. Detected during --install so we can migrate the user's rc cleanly.
const LEGACY_BEGIN_MARKER: &str = "# >>> aida dev workflow helpers >>>";
const LEGACY_END_MARKER: &str = "# <<< aida dev workflow helpers <<<";

fn handle_dev_shell_init(install: bool) -> Result<()> {
    // If we're inside the aida repo, capture its absolute path so we can
    // bake an `export AIDA_DEV_REPO=...` line into the helpers file. That
    // lets `aida dev activate` find the in-repo build from any directory
    // (e.g. while working in ~/ai/paradox), not only from inside or under
    // the aida checkout.
    let repo = std::env::current_dir()
        .ok()
        .and_then(|cwd| find_aida_repo_above(&cwd));
    let env_export = match &repo {
        Some(r) => format!("export AIDA_DEV_REPO='{}'\n\n", r.display()),
        None => String::new(),
    };
    let helpers_body = format!(
        "{}{}{}{}",
        "# AIDA shell helpers — generated by `aida dev shell-init --install`.\n\
         # Re-run that command to regenerate this file (e.g. after upgrading aida).\n\n",
        env_export,
        SHELL_HELPERS,
        ""
    );

    if !install {
        // Preview mode — show what would land in the helpers file, marker-wrapped
        // so the user can also paste it directly into a shell rc if they prefer.
        print!("{}\n{}{}\n", HELPERS_BEGIN_MARKER, helpers_body, HELPERS_END_MARKER);
        return Ok(());
    }

    let shell = std::env::var("SHELL").unwrap_or_default();
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    let rc_path = if shell.ends_with("/zsh") || shell.ends_with("zsh") {
        home.join(".zshrc")
    } else {
        home.join(".bashrc")
    };

    // The helpers file lives at ~/.aida/shell-init.sh — the rc only gets a
    // one-line `[ -f ... ] && source ...` stub. Lets us update helpers on
    // every `--install` without growing the rc.
    let helpers_path = home.join(".aida").join("shell-init.sh");
    if let Some(parent) = helpers_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&helpers_path, &helpers_body)
        .with_context(|| format!("Failed to write {}", helpers_path.display()))?;

    // Now ensure the rc has the source-stub block. Three input states:
    //   (a) rc already has the new marker pair → replace stub in place
    //   (b) rc has the old (pre-split) marker pair → migrate: drop old fat
    //       block, append the new stub
    //   (c) rc has no aida block → append the new stub
    let stub_body = format!(
        "# Auto-generated by `aida dev shell-init --install`. The actual helpers\n\
         # live at the path below; re-run that command to update them.\n\
         [ -f \"{path}\" ] && source \"{path}\"\n",
        path = helpers_path.display()
    );
    let new_block = format!(
        "{}\n{}{}\n",
        HELPERS_BEGIN_MARKER, stub_body, HELPERS_END_MARKER
    );

    let existing = std::fs::read_to_string(&rc_path).unwrap_or_default();
    let mut migration_note: Option<String> = None;

    let new_content = if let Some(start) = existing.find(HELPERS_BEGIN_MARKER) {
        // (a) Replace existing stub.
        let end_after = existing[start..]
            .find(HELPERS_END_MARKER)
            .map(|e| start + e + HELPERS_END_MARKER.len())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "found begin marker but no end marker in {} — please clean up manually",
                    rc_path.display()
                )
            })?;
        let end_after = if existing.as_bytes().get(end_after) == Some(&b'\n') {
            end_after + 1
        } else {
            end_after
        };
        let mut s = existing[..start].to_string();
        s.push_str(&new_block);
        s.push_str(&existing[end_after..]);
        s
    } else if let Some(start) = existing.find(LEGACY_BEGIN_MARKER) {
        // (b) Migrate from the previous fat block (helpers inlined into rc).
        let end_after = existing[start..]
            .find(LEGACY_END_MARKER)
            .map(|e| start + e + LEGACY_END_MARKER.len())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "found legacy begin marker but no legacy end marker in {} — please clean up manually",
                    rc_path.display()
                )
            })?;
        let removed_lines = existing[start..end_after].lines().count();
        let end_after = if existing.as_bytes().get(end_after) == Some(&b'\n') {
            end_after + 1
        } else {
            end_after
        };
        let mut s = existing[..start].to_string();
        s.push_str(&new_block);
        s.push_str(&existing[end_after..]);
        migration_note = Some(format!(
            "  (migrated: {} lines of inline helpers replaced with a 3-line source stub)",
            removed_lines
        ));
        s
    } else if existing.contains("# AIDA dev workflow helpers") {
        // Markerless legacy form (very early — pre-marker). Same drop-by-line
        // approach as before, then append the new stub.
        let mut kept: Vec<&str> = Vec::new();
        let mut skipping = false;
        let mut skipped = 0usize;
        for line in existing.lines() {
            if line.starts_with("# AIDA dev workflow helpers") {
                skipping = true;
                skipped += 1;
                continue;
            }
            if skipping {
                if line.starts_with("aida-on") || line.starts_with("aida-off") {
                    skipped += 1;
                    continue;
                }
                skipping = false;
            }
            kept.push(line);
        }
        migration_note = Some(format!(
            "  (migrated: removed {} lines of pre-marker legacy helpers)",
            skipped
        ));
        let mut s = kept.join("\n");
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push('\n');
        s.push_str(&new_block);
        s
    } else {
        // (c) Fresh install.
        let mut s = existing;
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push('\n');
        s.push_str(&new_block);
        s
    };

    std::fs::write(&rc_path, new_content)
        .with_context(|| format!("Failed to write {}", rc_path.display()))?;

    if let Some(note) = migration_note {
        eprintln!("{}", note);
    }
    eprintln!(
        "{}: helpers installed.",
        "OK".green()
    );
    eprintln!(
        "  Helpers file: {} ({} lines)",
        helpers_path.display(),
        helpers_body.lines().count()
    );
    eprintln!(
        "  Source stub:  {} (3 lines, sourced on shell startup)",
        rc_path.display()
    );
    match &repo {
        Some(r) => eprintln!(
            "  AIDA_DEV_REPO={} (baked into the helpers file)",
            r.display()
        ),
        None => {
            eprintln!(
                "  {}: not run from inside the aida repo, so AIDA_DEV_REPO was NOT set.",
                "Note".yellow()
            );
            eprintln!(
                "         `aida dev activate` will only find the dev binary when you cd into the repo."
            );
            eprintln!(
                "         To make it work everywhere, re-run from the aida repo or add manually:"
            );
            eprintln!("           export AIDA_DEV_REPO=/path/to/aida");
        }
    }
    eprintln!("  Reload: source {}", rc_path.display());
    eprintln!(
        "  Then any of: {}, {}, {}",
        "aida dev activate".cyan(),
        "aida role list".cyan(),
        "aida role enter <name>".cyan()
    );
    eprintln!(
        "  All eval-required commands now Just Work — the wrapper handles the eval."
    );
    Ok(())
}

fn handle_dev_serve(
    rest_port: Option<u16>,
    grpc_port: Option<u16>,
    web_port: Option<u16>,
    no_web: bool,
) -> Result<()> {
    use std::process::Stdio;
    use tokio::process::Command as TokioCommand;
    use tokio::sync::mpsc;

    let cwd = std::env::current_dir()?;
    let repo_for_web = if !no_web && cwd.join("aida-web-react").is_dir() {
        Some(cwd.clone())
    } else {
        None
    };

    // Locate the aida-server binary: prefer the in-repo build (since dev
    // workflow), fall back to PATH.
    let server_bin = locate_aida_server_binary(&cwd)?;

    let rest = rest_port.unwrap_or(8080);
    let grpc = grpc_port.unwrap_or(50051);
    let web = web_port.unwrap_or(5173);

    let store_path = detect_distributed_store().unwrap_or_else(|| cwd.clone());

    println!("{}", "─── aida dev serve ───".bold());
    println!("  REST/HTTP:  http://localhost:{}", rest);
    println!("  gRPC:       localhost:{}", grpc);
    if repo_for_web.is_some() {
        println!("  React dev:  http://localhost:{}", web);
    } else if no_web {
        println!("  React dev:  skipped (--no-web)");
    } else {
        println!("  React dev:  skipped (no aida-web-react/ in cwd)");
    }
    println!("  Store:      {}", store_path.display());
    println!("  Press Ctrl+C to stop");
    println!();

    // Run inside a tokio runtime so we can supervise children + signals.
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let (tx, mut rx) = mpsc::unbounded_channel::<()>();

        // Start aida-server.
        let mut server_child = TokioCommand::new(&server_bin)
            .args([
                "--host",
                "0.0.0.0",
                "--port",
                &grpc.to_string(),
                "--rest-port",
                &rest.to_string(),
                "--database",
            ])
            .arg(&store_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("Failed to spawn aida-server at {}", server_bin.display()))?;

        spawn_log_pump("server", server_child.stdout.take(), tx.clone());
        spawn_log_pump("server", server_child.stderr.take(), tx.clone());

        // Optionally start vite dev server.
        let mut web_child = if let Some(repo) = repo_for_web {
            let cwd = repo.join("aida-web-react");
            if !cwd.join("node_modules").is_dir() {
                eprintln!(
                    "[web] note: aida-web-react/node_modules not found — run `npm install` first."
                );
            }
            let child = TokioCommand::new("npm")
                .args(["run", "dev", "--", "--port", &web.to_string()])
                .current_dir(&cwd)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("Failed to spawn `npm run dev` for aida-web-react")?;
            Some(child)
        } else {
            None
        };

        if let Some(ref mut w) = web_child {
            spawn_log_pump("web", w.stdout.take(), tx.clone());
            spawn_log_pump("web", w.stderr.take(), tx.clone());
        }

        // Helper future: wait for a child to exit naturally.
        async fn wait_child(child: &mut tokio::process::Child) -> std::io::Result<std::process::ExitStatus> {
            child.wait().await
        }

        // Race Ctrl+C against either child exiting.
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\n[dev serve] Ctrl+C — stopping children...");
            }
            r = wait_child(&mut server_child) => {
                eprintln!("\n[dev serve] aida-server exited unexpectedly: {:?}", r);
            }
            r = async {
                if let Some(ref mut w) = web_child {
                    wait_child(w).await
                } else {
                    std::future::pending().await
                }
            } => {
                eprintln!("\n[dev serve] vite dev server exited unexpectedly: {:?}", r);
            }
        }

        // Send SIGTERM (kill_on_drop fires SIGKILL on drop, but we want
        // a chance for clean shutdown first).
        let _ = server_child.start_kill();
        if let Some(ref mut w) = web_child {
            let _ = w.start_kill();
        }
        let _ = server_child.wait().await;
        if let Some(mut w) = web_child {
            let _ = w.wait().await;
        }

        // Drop the sender so log-pump tasks can exit cleanly.
        drop(tx);
        while rx.recv().await.is_some() {}

        Ok::<_, anyhow::Error>(())
    })?;

    eprintln!("[dev serve] stopped.");
    Ok(())
}

/// Stream a child's stdout/stderr to the parent's stderr with a prefix.
/// `aida dev release [bump]` — the one-command release flow:
/// 1. run scripts/release.sh (bumps version, tags, pushes, interactive)
/// 2. wait for the GitHub Actions workflow to publish the binary tarballs
///    (HEAD-poll the asset URL with timeout)
/// 3. upgrade sibling installs to the new version (auto-yes)
///
/// `aida dev patch` is a thin alias that calls this with bump = "patch".
/// trace:EPIC-1-001 | ai:claude
fn handle_dev_release(bump: &str) -> Result<()> {
    // Locate the aida repo. Prefer PWD walk, then $AIDA_DEV_REPO.
    let cwd = std::env::current_dir()?;
    let repo = find_aida_repo_above(&cwd)
        .or_else(|| {
            std::env::var("AIDA_DEV_REPO")
                .ok()
                .map(std::path::PathBuf::from)
                .filter(|p| is_aida_repo(p))
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Not in an aida repo. cd into the aida checkout, set AIDA_DEV_REPO, \
                 or run `eval \"$(aida dev activate)\"` first."
            )
        })?;

    let script = repo.join("scripts/release.sh");
    if !script.is_file() {
        anyhow::bail!(
            "scripts/release.sh not found at {} — is this checkout up to date?",
            script.display()
        );
    }

    println!(
        "{}",
        format!("─── Step 1/3: ./scripts/release.sh {} ───", bump).bold()
    );
    println!("Working in {}", repo.display());
    println!();

    // Run release.sh interactively — it prints the version-bump diff and
    // asks for confirmation. Inheriting stdio means the user sees and
    // responds to the prompts directly.
    let status = std::process::Command::new(&script)
        .arg(bump)
        .current_dir(&repo)
        .status()
        .with_context(|| format!("Failed to invoke {}", script.display()))?;
    if !status.success() {
        anyhow::bail!(
            "release.sh exited non-zero (likely cancelled at the confirmation \
             prompt). Aborting upgrade phase — your tree may have a pending \
             version bump; resolve manually if needed."
        );
    }

    // After release.sh completes, the new tag is the latest one in the repo.
    let new_tag = git_describe_latest_tag(&repo).ok_or_else(|| {
        anyhow::anyhow!("release.sh succeeded but no tag is reachable — confused state, please check `git tag --list` manually.")
    })?;

    println!();
    println!(
        "{}",
        format!("─── Step 2/3: waiting for {} release artifacts ───", new_tag).bold()
    );
    let target = release_target().ok_or_else(|| {
        anyhow::anyhow!(
            "Unsupported platform — can't auto-poll for tarballs. Wait for the \
             release.yml workflow to finish and run `aida upgrade` manually."
        )
    })?;
    let asset_url = format!(
        "https://github.com/joemooney/aida/releases/download/{}/aida-{}.tar.gz",
        new_tag, target
    );
    println!("Polling {} ...", asset_url);
    poll_until_published(&asset_url, std::time::Duration::from_secs(600))?;

    println!();
    println!(
        "{}",
        format!("─── Step 3/3: upgrading sibling installs to {} ───", new_tag).bold()
    );
    let bare_version = strip_v(&new_tag).to_string();
    upgrade_dev_mode_sibling_scan(false, Some(&bare_version), true)?;

    println!();
    println!(
        "{}: shipped {} and refreshed sibling installs.",
        "DONE".green().bold(),
        new_tag
    );
    Ok(())
}

/// HEAD-poll an URL until it returns 200 or `timeout` elapses. Used by
/// `aida dev release` to wait for the GitHub Actions release workflow to
/// publish its tarballs after we push the tag.
fn poll_until_published(url: &str, timeout: std::time::Duration) -> Result<()> {
    let start = std::time::Instant::now();
    let mut tick: u32 = 0;
    loop {
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Timed out after {} seconds. The release workflow may have failed.\n\
                 Check status: https://github.com/joemooney/aida/actions\n\
                 Once tarballs are published, run `aida upgrade --yes`.",
                timeout.as_secs()
            );
        }
        let out = std::process::Command::new("curl")
            .args(["-sIL", "-o", "/dev/null", "-w", "%{http_code}"])
            .arg(url)
            .output()
            .context("Failed to invoke curl")?;
        let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if code == "200" {
            println!("\n  ✓ artifact available ({}s)", start.elapsed().as_secs());
            return Ok(());
        }
        // Animated dots so the user sees we're not stuck.
        let dots = ".".repeat(((tick % 4) + 1) as usize);
        print!(
            "\r  ... waiting for tarball ({:>3}s elapsed, http {}, last poll {}){}    ",
            start.elapsed().as_secs(),
            code,
            tick,
            dots
        );
        std::io::Write::flush(&mut std::io::stdout()).ok();
        tick += 1;
        std::thread::sleep(std::time::Duration::from_secs(15));
    }
}

fn spawn_log_pump<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    prefix: &'static str,
    reader: Option<R>,
    _done: tokio::sync::mpsc::UnboundedSender<()>,
) {
    let Some(reader) = reader else { return };
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("[{}] {}", prefix, line);
        }
    });
}

/// Resolve the aida-server binary: prefer the in-repo target/{release,debug}
/// build, fall back to whatever's on PATH.
fn locate_aida_server_binary(cwd: &std::path::Path) -> Result<std::path::PathBuf> {
    // If we're in (or under) the aida repo, use its built binary.
    let mut probe = cwd.to_path_buf();
    for _ in 0..4 {
        if is_aida_repo(&probe) {
            let release = probe.join("target/release/aida-server");
            if release.exists() {
                return Ok(release);
            }
            let debug = probe.join("target/debug/aida-server");
            if debug.exists() {
                return Ok(debug);
            }
            anyhow::bail!(
                "Found aida repo at {} but no aida-server binary in target/. Run `cargo build` first.",
                probe.display()
            );
        }
        match probe.parent() {
            Some(p) => probe = p.to_path_buf(),
            None => break,
        }
    }
    // Fall back to PATH.
    if let Ok(out) = std::process::Command::new("which").arg("aida-server").output() {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !p.is_empty() {
            return Ok(std::path::PathBuf::from(p));
        }
    }
    anyhow::bail!("aida-server not found on PATH and no in-repo build available")
}

// ----------------------------------------------------------------------------
// `aida status` — comprehensive project overview, with extra sections when
// the current project is the aida repo itself.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

/// Distributed-mode status: read from CachedGitBackend so we get cache-backed
/// counts, sync state, and recent activity.
fn handle_status_command_distributed(
    no_dev_context: bool,
    store_path: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    use aida_core::DatabaseBackend;

    let store = backend.load()?;
    let project_root = std::env::current_dir()?;

    println!("{}", "─── Project ───".bold());
    let name = if store.name.is_empty() { "(unnamed)" } else { &store.name };
    println!("  Name:         {}", name.white().bold());
    println!("  Mode:         {} (orphan branch)", "distributed git-canonical".cyan());
    println!("  Store path:   {}", store_path.display());
    println!();

    // Requirement counts grouped by status.
    let summaries = backend.list_summaries(&aida_core::ListFilter {
        include_archived: true,
        ..Default::default()
    })?;
    let total = summaries.len();
    let mut by_status: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for s in &summaries {
        *by_status.entry(s.status.clone()).or_insert(0) += 1;
    }
    println!("{}", "─── Requirements ───".bold());
    println!("  Total:        {}", total);
    for (status, count) in &by_status {
        println!("    {:<14} {}", status, count);
    }
    println!();

    // Cache state.
    let cache = backend.cache();
    let cached = cache.requirement_count()?;
    let recorded_sha = cache.source_head_sha()?.unwrap_or_default();
    let actual_sha = aida_core::git_ops::head_sha(store_path).unwrap_or_default();
    let stale = recorded_sha != actual_sha || recorded_sha.is_empty();
    println!("{}", "─── Cache ───".bold());
    println!("  Path:         {}", cache.path().display());
    println!("  Rows:         {} (store has {})", cached, total);
    println!(
        "  Status:       {}",
        if stale && !actual_sha.is_empty() {
            format!("{} — run `aida cache rebuild`", "STALE".yellow())
        } else {
            "FRESH".green().to_string()
        }
    );
    println!();

    // Sync state — orphan-branch ahead/behind origin/aida-store.
    if let Some((ahead, behind)) = orphan_branch_sync_state(store_path) {
        println!("{}", "─── Sync ───".bold());
        match (ahead, behind) {
            (0, 0) => println!("  Branch aida-store: in sync with origin"),
            (a, 0) => println!(
                "  Branch aida-store: {} ahead of origin (run `git push origin aida-store`)",
                a.to_string().yellow()
            ),
            (0, b) => println!(
                "  Branch aida-store: {} behind origin (run `git fetch && git pull` in {})",
                b.to_string().yellow(),
                store_path.display()
            ),
            (a, b) => println!(
                "  Branch aida-store: {} ahead, {} behind (diverged)",
                a.to_string().red(),
                b.to_string().red()
            ),
        }
        println!();
    }

    // Recent activity — top 5 most recently modified.
    let mut recent = summaries.clone();
    recent.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
    println!("{}", "─── Recent activity ───".bold());
    for r in recent.iter().take(5) {
        let id = r
            .agreed_id
            .as_deref()
            .or(r.spec_id.as_deref())
            .unwrap_or("?");
        let modified = r.modified_at.split('T').next().unwrap_or(&r.modified_at);
        println!(
            "  {:<14} {:<12} {} — {}",
            id, r.status, modified, r.title
        );
    }
    if recent.is_empty() {
        println!("  (no requirements yet)");
    }
    println!();

    // Scaffolding freshness — only useful for non-AIDA-self projects, since
    // AIDA's own .claude/ uses symlinks into aida-core/templates/ and can't
    // drift. The aida-self block below has its own template-symlink check.
    if !is_aida_repo(&project_root) {
        print_scaffolding_freshness(&project_root);
    }

    // AIDA-self developer context — only when this project IS the aida repo.
    if !no_dev_context && is_aida_repo(&project_root) {
        print_aida_dev_context(&project_root);
    }

    Ok(())
}

/// Compare a project's `.claude/skills/`, `.claude/commands/`, `.claude/hooks/`
/// (and CLAUDE.md / AGENTS.md / .mcp.json) against the templates embedded in
/// the running aida binary. Reports counts of files that match exactly vs
/// files that have drifted, and suggests `aida scaffold apply --force` if
/// there's drift. Quiet when the project has no scaffolding at all.
/// trace:EPIC-1-001 | ai:claude
fn print_scaffolding_freshness(project_root: &std::path::Path) {
    use aida_core::scaffolding::{ScaffoldConfig, Scaffolder};

    // Need a RequirementsStore to drive the scaffolder, but we only care
    // about template content — an empty store is fine for comparison.
    let empty_store = aida_core::models::RequirementsStore::new();
    let config = ScaffoldConfig::default();
    let mut scaffolder = Scaffolder::with_database(
        project_root.to_path_buf(),
        config,
        std::path::PathBuf::from("requirements.db"), // dummy; only used for path-substitution in templates
    );
    let preview = scaffolder.preview(&empty_store);

    let mut total = 0usize;
    let mut present = 0usize;
    let mut matches = 0usize;
    let mut drifted: Vec<std::path::PathBuf> = Vec::new();

    for artifact in &preview.artifacts {
        total += 1;
        let full = project_root.join(&artifact.path);
        if !full.exists() {
            // missing; not "drifted" (probably user opted out via --no-skills)
            continue;
        }
        present += 1;
        let on_disk = match std::fs::read(&full) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if on_disk == artifact.content.as_bytes() {
            matches += 1;
        } else {
            drifted.push(artifact.path.clone());
        }
    }

    // No scaffolding present at all — stay quiet (probably a non-aida project
    // that just happens to have a .aida/config.toml from somewhere unrelated).
    if present == 0 {
        return;
    }

    println!("{}", "─── Scaffolding ───".bold());
    println!("  Templates compared: {} total, {} present in project", total, present);
    if drifted.is_empty() {
        println!(
            "  Status:             {} — all {} present file(s) match the embedded templates",
            "FRESH".green(),
            matches
        );
    } else {
        println!(
            "  Status:             {} — {} file(s) differ from the embedded templates",
            "STALE".yellow(),
            drifted.len()
        );
        for path in drifted.iter().take(5) {
            println!("    - {}", path.display());
        }
        if drifted.len() > 5 {
            println!("    ... and {} more", drifted.len() - 5);
        }
        println!(
            "  Refresh with:       {} (or `aida scaffold apply --dry-run` to preview)",
            "aida scaffold apply --force".cyan()
        );
    }
    println!();
}

/// Legacy-mode status: minimal output via the file-based Storage class.
fn handle_status_command(
    no_dev_context: bool,
    store_path_override: Option<&std::path::Path>,
    storage: &Storage,
) -> Result<()> {
    let store = storage.load()?;
    let project_root = std::env::current_dir()?;

    println!("{}", "─── Project ───".bold());
    let name = if store.name.is_empty() { "(unnamed)" } else { &store.name };
    println!("  Name:         {}", name.white().bold());
    let mode = if storage.is_sqlite() {
        "centralized SQLite (deprecated)"
    } else {
        "centralized YAML (deprecated)"
    };
    println!("  Mode:         {}", mode.yellow());
    println!("  Store path:   {}", storage.path().display());
    println!();

    let total = store.requirements.len();
    let mut by_status: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for r in &store.requirements {
        *by_status.entry(r.effective_status()).or_insert(0) += 1;
    }
    println!("{}", "─── Requirements ───".bold());
    println!("  Total:        {}", total);
    for (status, count) in &by_status {
        println!("    {:<14} {}", status, count);
    }
    println!();

    println!(
        "{}: this project is on a deprecated centralized backend.",
        "WARN".yellow()
    );
    println!(
        "      Migrate by running `aida db export-git -o aida-store && aida init` to switch to git-canonical."
    );
    println!();

    if !no_dev_context && is_aida_repo(&project_root) {
        print_aida_dev_context(&project_root);
    }

    let _ = store_path_override; // reserved for future use
    Ok(())
}

/// Detect whether `project_root` is the aida repo itself — used to opt into
/// the developer-context section. We check the workspace root Cargo.toml for
/// the joemooney/aida repository URL.
fn is_aida_repo(project_root: &std::path::Path) -> bool {
    let cargo_toml = project_root.join("Cargo.toml");
    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
        // Match the workspace.package repository field exactly so this is
        // robust against forks: only the canonical repo gets dev context.
        return content.contains("repository = \"https://github.com/joemooney/aida\"")
            && content.contains("[workspace]");
    }
    false
}

fn print_aida_dev_context(project_root: &std::path::Path) {
    println!("{}", "─── AIDA development context ───".bold());

    // Workspace version vs latest tag.
    let workspace_version = read_workspace_version(project_root).unwrap_or_else(|| "?".into());
    let latest_tag = git_describe_latest_tag(project_root).unwrap_or_else(|| "(none)".into());
    let commits_since_tag = git_commits_since_tag(project_root, &latest_tag).unwrap_or(0);
    println!("  Running binary:     {}", build_banner());
    println!("  Workspace version:  v{}", workspace_version);
    println!("  Latest release tag: {}", latest_tag);
    if commits_since_tag > 0 {
        println!(
            "  {} commits ahead of {} — release-readiness: {}",
            commits_since_tag,
            latest_tag,
            "ready to cut a release".yellow()
        );
    } else {
        println!("  Tree matches latest tag — no pending release");
    }

    // Template-symlink integrity.
    let symlink_status = check_template_symlinks(project_root);
    println!("  Template symlinks:  {}", symlink_status);

    // Quick build sanity (just check `target/` exists and Cargo.lock is in sync).
    let cargo_lock_synced = project_root.join("Cargo.lock").exists();
    println!(
        "  Cargo.lock present: {}",
        if cargo_lock_synced {
            "yes".green().to_string()
        } else {
            "NO".red().to_string()
        }
    );

    println!();
    println!("  Helpful:");
    println!("    {}    {}", "scripts/release.sh".cyan(), "— bump version + tag + push");
    println!("    {}    {}", "scripts/publish.sh".cyan(), "— cargo publish to crates.io");
    println!();
}

fn read_workspace_version(root: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let mut in_workspace_package = false;
    for line in content.lines() {
        let line = line.trim();
        if line == "[workspace.package]" {
            in_workspace_package = true;
            continue;
        }
        if in_workspace_package {
            if line.starts_with('[') {
                break;
            }
            if let Some(rest) = line.strip_prefix("version") {
                let v = rest
                    .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
                    .trim_matches('"');
                return Some(v.to_string());
            }
        }
    }
    None
}

fn git_describe_latest_tag(root: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_commits_since_tag(root: &std::path::Path, tag: &str) -> Option<usize> {
    if tag == "(none)" {
        return None;
    }
    let out = std::process::Command::new("git")
        .args(["rev-list", "--count", &format!("{}..HEAD", tag)])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn check_template_symlinks(root: &std::path::Path) -> String {
    let claude_skills = root.join(".claude/skills");
    if !claude_skills.is_dir() {
        return "no .claude/skills/ directory".to_string();
    }
    let mut total = 0;
    let mut broken = 0;
    if let Ok(entries) = std::fs::read_dir(&claude_skills) {
        for entry in entries.flatten() {
            let path = entry.path();
            // Only count the actual symlinks (skip non-symlink files).
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if meta.file_type().is_symlink() {
                    total += 1;
                    if !path.exists() {
                        broken += 1;
                    }
                }
            }
        }
    }
    if total == 0 {
        return "no symlinks (templates copied, not symlinked)".to_string();
    }
    if broken == 0 {
        format!("{}/{} OK", total, total).green().to_string()
    } else {
        format!("{} broken / {}", broken, total).red().to_string()
    }
}

/// Returns (ahead, behind) of `aida-store` branch vs `origin/aida-store`.
fn orphan_branch_sync_state(store_path: &std::path::Path) -> Option<(usize, usize)> {
    // Run inside the store worktree so we see the right branch.
    let ahead = std::process::Command::new("git")
        .args(["rev-list", "--count", "origin/aida-store..HEAD"])
        .current_dir(store_path)
        .output()
        .ok()?;
    let behind = std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD..origin/aida-store"])
        .current_dir(store_path)
        .output()
        .ok()?;
    if !ahead.status.success() || !behind.status.success() {
        return None;
    }
    let a: usize = String::from_utf8_lossy(&ahead.stdout)
        .trim()
        .parse()
        .ok()?;
    let b: usize = String::from_utf8_lossy(&behind.stdout)
        .trim()
        .parse()
        .ok()?;
    Some((a, b))
}

// ----------------------------------------------------------------------------
// `aida upgrade` — fetch latest release and replace the running binary.
// trace:EPIC-1-001 | ai:claude
// ----------------------------------------------------------------------------

/// How aida was installed on this machine. Determines the upgrade strategy.
enum InstallMethod {
    /// Found under `~/.cargo/bin/` — installed via `cargo install`.
    /// Upgrade by re-running `cargo install --git`.
    Cargo(std::path::PathBuf),
    /// Found in a system bin dir (`/usr/local/bin`, `/opt/...`, etc.) —
    /// installed via release tarball. Upgrade by downloading the matching
    /// release artifact and replacing the binary in place.
    Binary(std::path::PathBuf),
    /// Found inside a `target/debug` or `target/release` directory — the
    /// running binary is a developer build. Refuse to upgrade.
    DeveloperBuild(std::path::PathBuf),
}

fn detect_install_method() -> Result<InstallMethod> {
    let exe = std::env::current_exe().context("Failed to determine current binary path")?;
    let exe_str = exe.to_string_lossy();

    if exe_str.contains("/target/debug/") || exe_str.contains("/target/release/") {
        return Ok(InstallMethod::DeveloperBuild(exe));
    }

    // Cargo install puts binaries in $CARGO_HOME/bin (default ~/.cargo/bin).
    let cargo_home = std::env::var("CARGO_HOME").ok();
    let cargo_bin = cargo_home
        .map(|h| std::path::PathBuf::from(h).join("bin"))
        .or_else(|| dirs::home_dir().map(|h| h.join(".cargo/bin")));
    if let Some(bin) = cargo_bin {
        if exe.starts_with(&bin) {
            return Ok(InstallMethod::Cargo(exe));
        }
    }

    Ok(InstallMethod::Binary(exe))
}

fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Build git SHA stamped at compile time (or "unknown" if git wasn't
/// available in the build env). Set by build.rs.
fn build_git_sha() -> &'static str {
    env!("AIDA_BUILD_GIT_SHA")
}

/// Whether the working tree had uncommitted changes at build time.
fn build_git_dirty() -> bool {
    env!("AIDA_BUILD_GIT_DIRTY") == "1"
}

/// Build time as ISO-8601 UTC, formatted at runtime from the unix-epoch
/// seconds stamped at compile time.
fn build_time_iso() -> String {
    let secs: i64 = env!("AIDA_BUILD_UNIX_TIME").parse().unwrap_or(0);
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| "(unknown)".to_string())
}

/// One-line build banner for use in --version and status output:
///   "0.4.0 (built 2026-05-03T01:23:45Z, sha 866b050[+dirty])"
fn build_banner() -> String {
    format!(
        "{} (built {}, sha {}{})",
        current_version(),
        build_time_iso(),
        build_git_sha(),
        if build_git_dirty() { "+dirty" } else { "" }
    )
}

/// Compile-time-resolved release-artifact target name (matches the matrix in
/// `.github/workflows/release.yml`). Returns None on unsupported platforms.
fn release_target() -> Option<&'static str> {
    use std::env::consts::{ARCH, OS};
    match (OS, ARCH) {
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("macos", "x86_64") => Some("darwin-x86_64"),
        ("macos", "aarch64") => Some("darwin-arm64"),
        _ => None,
    }
}

/// Query GitHub for the latest release tag. Uses curl; no extra dep needed.
fn fetch_latest_release_tag() -> Result<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-sSL",
            "-H",
            "Accept: application/vnd.github+json",
            "https://api.github.com/repos/joemooney/aida/releases/latest",
        ])
        .output()
        .context("Failed to invoke curl — is it installed?")?;
    if !out.status.success() {
        anyhow::bail!("curl failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let body = String::from_utf8(out.stdout).context("GitHub API response not UTF-8")?;
    // Tiny parser — avoids a serde_json dep here and keeps the code simple.
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("\"tag_name\":") {
            let v = rest.trim().trim_start_matches('"');
            if let Some(end) = v.find('"') {
                return Ok(v[..end].to_string());
            }
        }
    }
    anyhow::bail!("Could not parse latest release tag from GitHub response");
}

/// Strip a leading `v` from a tag string (`v0.4.0` -> `0.4.0`).
fn strip_v(s: &str) -> &str {
    s.strip_prefix('v').unwrap_or(s)
}

fn handle_upgrade_command(
    check: bool,
    version: Option<&str>,
    yes: bool,
    target: Option<&str>,
) -> Result<()> {
    // --target path: upgrade a specific binary, regardless of what's running.
    if let Some(target) = target {
        return upgrade_specific_binary(std::path::Path::new(target), check, version, yes);
    }

    let install = detect_install_method()?;

    // Developer build: don't try to upgrade ourselves; instead scan for
    // sibling installs and offer to upgrade them.
    if let InstallMethod::DeveloperBuild(_) = &install {
        return upgrade_dev_mode_sibling_scan(check, version, yes);
    }

    upgrade_running_binary(install, check, version, yes)
}

/// The original "upgrade the binary I'm running" flow. Used for cargo and
/// pre-built binary installs.
fn upgrade_running_binary(
    install: InstallMethod,
    check: bool,
    version: Option<&str>,
    yes: bool,
) -> Result<()> {
    let current = current_version();
    let install_label = match &install {
        InstallMethod::Cargo(p) => format!("cargo install ({})", p.display()),
        InstallMethod::Binary(p) => format!("pre-built binary ({})", p.display()),
        InstallMethod::DeveloperBuild(p) => format!("developer build ({})", p.display()),
    };
    println!("Current version: {}", build_banner());
    println!("Installed via:   {}", install_label);
    let _ = current; // version is included in build_banner()

    let target_tag = resolve_target_tag(version)?;
    let target_version = strip_v(&target_tag);
    println!("Target version:  {}", target_tag);

    if target_version == current {
        println!("\n{}: already on {}.", "OK".green(), target_tag);
        return Ok(());
    }

    if check {
        println!(
            "\n{}: an upgrade is available. Run `aida upgrade` (without --check) to install.",
            "INFO".blue()
        );
        return Ok(());
    }

    if !yes && !confirm(&format!("\nUpgrade from v{} to {}? [y/N]: ", current, target_tag)) {
        println!("Cancelled.");
        return Ok(());
    }

    match install {
        InstallMethod::Cargo(_) => upgrade_via_cargo(&target_tag),
        InstallMethod::Binary(p) => upgrade_via_release_tarball(&p, &target_tag),
        InstallMethod::DeveloperBuild(_) => unreachable!(),
    }
}

/// `--target PATH` flow: upgrade a specific binary path regardless of what's
/// currently running. Lets a dev-build session refresh `~/.local/bin/aida`.
fn upgrade_specific_binary(
    target_path: &std::path::Path,
    check: bool,
    version: Option<&str>,
    yes: bool,
) -> Result<()> {
    let install = classify_install_path(target_path);

    let probed = query_binary_version(target_path);
    let current_version = probed.as_ref().map(|(v, _)| v.clone());
    let current_banner = probed
        .as_ref()
        .map(|(_, b)| b.clone())
        .unwrap_or_else(|| "(not installed)".into());
    let target_tag = resolve_target_tag(version)?;

    println!("Target binary: {}", target_path.display());
    println!("Install type:  {}", install_method_label(&install));
    println!("Current:       {}", current_banner);
    println!("Target:        {}", target_tag);

    if let InstallMethod::DeveloperBuild(p) = &install {
        anyhow::bail!(
            "Refusing to upgrade a developer build at {}.\n\
             Pass --target pointing at a real install (e.g. ~/.local/bin/aida).",
            p.display()
        );
    }

    if current_version.as_deref() == Some(strip_v(&target_tag)) {
        println!("\n{}: {} already on {}.", "OK".green(), target_path.display(), target_tag);
        return Ok(());
    }

    if check {
        println!(
            "\n{}: upgrade available for {}. Re-run without --check to install.",
            "INFO".blue(),
            target_path.display()
        );
        return Ok(());
    }

    if !yes
        && !confirm(&format!(
            "\nUpgrade {} to {}? [y/N]: ",
            target_path.display(),
            target_tag
        ))
    {
        println!("Cancelled.");
        return Ok(());
    }

    match install {
        InstallMethod::Cargo(_) => upgrade_via_cargo(&target_tag),
        InstallMethod::Binary(p) => upgrade_via_release_tarball(&p, &target_tag),
        InstallMethod::DeveloperBuild(_) => unreachable!(),
    }
}

/// From a developer build, scan known install locations and report on
/// sibling aida installs, offering to upgrade any that are stale.
fn upgrade_dev_mode_sibling_scan(check: bool, version: Option<&str>, yes: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    println!("Current version: {}", build_banner());
    println!(
        "Installed via:   developer build ({})",
        exe.display()
    );
    println!(
        "Note: developer build doesn't need upgrading. Looking for other installs..."
    );
    println!();

    let target_tag = resolve_target_tag(version)?;
    let target_version = strip_v(&target_tag);

    let candidates = sibling_install_candidates();
    let mut found: Vec<(std::path::PathBuf, Option<(String, String)>)> = Vec::new();
    for path in candidates {
        if path.exists() && path != exe {
            let probed = query_binary_version(&path);
            found.push((path, probed));
        }
    }

    if found.is_empty() {
        println!("(no other aida installs found at common locations)");
        println!("  Searched: ~/.local/bin/, ~/.cargo/bin/, /usr/local/bin/, /opt/aida/bin/");
        return Ok(());
    }

    println!("Found:");
    let mut stale: Vec<std::path::PathBuf> = Vec::new();
    for (path, probed) in &found {
        let mtime = file_mtime_short(path);
        let (label, is_stale) = match probed {
            Some((v, banner)) if v == target_version => (
                format!("{}  · mtime {}  {}", banner, mtime, "up to date".green()),
                false,
            ),
            Some((_, banner)) => (
                format!(
                    "{}  · mtime {}  ({}, latest is {})",
                    banner,
                    mtime,
                    "stale".yellow(),
                    target_tag
                ),
                true,
            ),
            None => (
                format!("(could not detect version) · mtime {}", mtime),
                false,
            ),
        };
        println!("  {:<36}  {}", path.display(), label);
        if is_stale {
            stale.push(path.clone());
        }
    }
    println!();

    if check {
        if stale.is_empty() {
            println!("{}: all sibling installs are at {}.", "OK".green(), target_tag);
            print_unreleased_dev_hint(&exe, &target_tag);
        } else {
            println!(
                "{}: {} sibling install(s) are stale. Re-run without --check to upgrade.",
                "INFO".blue(),
                stale.len()
            );
        }
        return Ok(());
    }

    if stale.is_empty() {
        println!("{}: nothing to do — all sibling installs are at {}.", "OK".green(), target_tag);
        print_unreleased_dev_hint(&exe, &target_tag);
        return Ok(());
    }

    for path in stale {
        println!();
        if !yes
            && !confirm(&format!(
                "Upgrade {} to {}? [y/N]: ",
                path.display(),
                target_tag
            ))
        {
            println!("  skipped {}", path.display());
            continue;
        }
        let install = classify_install_path(&path);
        let result = match install {
            InstallMethod::Cargo(_) => upgrade_via_cargo(&target_tag),
            InstallMethod::Binary(_) => upgrade_via_release_tarball(&path, &target_tag),
            InstallMethod::DeveloperBuild(_) => {
                eprintln!(
                    "  {} {} is itself a developer build, skipping",
                    "warning:".yellow(),
                    path.display()
                );
                Ok(())
            }
        };
        if let Err(e) = result {
            eprintln!("  {} {}: {}", "error:".red(), path.display(), e);
        }
    }

    Ok(())
}

// ---- shared helpers -------------------------------------------------------

fn install_method_label(install: &InstallMethod) -> String {
    match install {
        InstallMethod::Cargo(_) => "cargo install".to_string(),
        InstallMethod::Binary(_) => "pre-built binary".to_string(),
        InstallMethod::DeveloperBuild(_) => "developer build".to_string(),
    }
}

fn resolve_target_tag(version: Option<&str>) -> Result<String> {
    match version {
        Some(v) => Ok(format!("v{}", v.strip_prefix('v').unwrap_or(v))),
        None => {
            print!("Querying github.com/joemooney/aida for latest release... ");
            std::io::Write::flush(&mut std::io::stdout()).ok();
            let tag = fetch_latest_release_tag()?;
            println!("{}", tag);
            Ok(tag)
        }
    }
}

fn confirm(prompt: &str) -> bool {
    print!("{}", prompt);
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let mut answer = String::new();
    if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Run `<path> --version` and parse out (version, full_banner). The banner
/// is everything after the program-name prefix and may include a build-time
/// stamp ("0.4.0 (built 2026-05-03T01:30:00Z, sha 866b050)") for binaries
/// built post-EPIC-1-001 — older binaries just have "0.4.0". Returns None
/// if the binary doesn't run or output doesn't look like a version.
fn query_binary_version(path: &std::path::Path) -> Option<(String, String)> {
    let out = std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // Strip the leading program name ("aida ", "aida-cli ", "aida-server ").
    let banner = ["aida-cli ", "aida-server ", "aida "]
        .iter()
        .find_map(|p| s.strip_prefix(*p))
        .map(String::from)
        .unwrap_or(s);
    // Pluck the first whitespace-separated token as the bare version.
    let version = banner
        .split_whitespace()
        .next()
        .filter(|v| v.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))?
        .to_string();
    Some((version, banner))
}

/// Common locations where users typically have aida installed. Order matters
/// for display; we use it as-is for the scan-and-report output.
/// Walk up from `start` (a binary path or directory) looking for the aida
/// repo root. Used to discover the dev binary's source repo so we can ask
/// "is this build ahead of the latest release tag".
fn find_aida_repo_above(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut probe = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    for _ in 0..6 {
        if is_aida_repo(&probe) {
            return Some(probe);
        }
        probe = match probe.parent() {
            Some(p) => p.to_path_buf(),
            None => return None,
        };
    }
    None
}

/// When `aida upgrade` runs from a developer build and finds all sibling
/// installs already at the latest released tag, surface the fact that the
/// dev build itself is ahead of that tag — otherwise the user might think
/// "everything's up to date" when really there are unreleased commits
/// sitting in their repo. Pure hint; doesn't trigger any action.
fn print_unreleased_dev_hint(exe: &std::path::Path, target_tag: &str) {
    let repo = match find_aida_repo_above(exe) {
        Some(r) => r,
        None => return,
    };
    let latest = match git_describe_latest_tag(&repo) {
        Some(t) => t,
        None => return,
    };
    if latest != target_tag {
        // Latest tag locally doesn't match the latest published release —
        // probably a fetch lag. Don't speculate; just bail quietly.
        return;
    }
    let ahead = git_commits_since_tag(&repo, &latest).unwrap_or(0);
    if ahead == 0 {
        return;
    }
    println!();
    println!(
        "{}: this dev build is {} commit{} ahead of {}.",
        "Note".blue(),
        ahead,
        if ahead == 1 { "" } else { "s" },
        latest
    );
    println!("      To ship those changes AND refresh your sibling installs in one shot:");
    println!("        {}", "aida dev patch".cyan());
    println!(
        "      (or `aida dev release {{minor|major|<version>}}` for a different bump)"
    );
    println!();
    println!(
        "      Or do it manually: `cd {} && ./scripts/release.sh patch`,",
        repo.display()
    );
    println!("      then re-run `aida upgrade`.");
}

/// File mtime as `YYYY-MM-DD` for display next to a binary's version. Useful
/// as a universal "when was this binary placed here" indicator — works even
/// for binaries built before the build-banner stamps existed (pre-EPIC-1-001).
fn file_mtime_short(path: &std::path::Path) -> String {
    let modified = match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(m) => m,
        Err(_) => return "(?)".to_string(),
    };
    let secs = match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return "(?)".to_string(),
    };
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "(?)".to_string())
}

fn sibling_install_candidates() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".local/bin/aida"));
        paths.push(home.join(".cargo/bin/aida"));
    }
    paths.push(std::path::PathBuf::from("/usr/local/bin/aida"));
    paths.push(std::path::PathBuf::from("/opt/aida/bin/aida"));
    paths
}

/// Like `detect_install_method` but classifies an arbitrary path rather
/// than the running binary.
fn classify_install_path(path: &std::path::Path) -> InstallMethod {
    let path_str = path.to_string_lossy();
    if path_str.contains("/target/debug/") || path_str.contains("/target/release/") {
        return InstallMethod::DeveloperBuild(path.to_path_buf());
    }
    let cargo_home = std::env::var("CARGO_HOME").ok();
    let cargo_bin = cargo_home
        .map(|h| std::path::PathBuf::from(h).join("bin"))
        .or_else(|| dirs::home_dir().map(|h| h.join(".cargo/bin")));
    if let Some(bin) = cargo_bin {
        if path.starts_with(&bin) {
            return InstallMethod::Cargo(path.to_path_buf());
        }
    }
    InstallMethod::Binary(path.to_path_buf())
}

/// Re-run `cargo install --git ...` to refresh the binary. Pins to the
/// requested tag so the install matches what the user asked for.
fn upgrade_via_cargo(tag: &str) -> Result<()> {
    println!(
        "\nRunning: cargo install --git https://github.com/joemooney/aida.git --tag {} --force aida-cli",
        tag
    );
    let status = std::process::Command::new("cargo")
        .args([
            "install",
            "--git",
            "https://github.com/joemooney/aida.git",
            "--tag",
            tag,
            "--force",
            "aida-cli",
        ])
        .status()
        .context("Failed to invoke cargo")?;
    if !status.success() {
        anyhow::bail!("cargo install failed");
    }
    println!("\n{}: upgraded to {}.", "OK".green(), tag);
    Ok(())
}

/// Download the release tarball matching this platform, extract, and install
/// over the existing binary. Uses sudo if the destination is not writable by
/// the current user.
fn upgrade_via_release_tarball(current_exe: &std::path::Path, tag: &str) -> Result<()> {
    let target = release_target()
        .ok_or_else(|| anyhow::anyhow!("Unsupported platform — no release artifact available. Use `cargo install --git` instead."))?;
    let url = format!(
        "https://github.com/joemooney/aida/releases/download/{}/aida-{}.tar.gz",
        tag, target
    );

    let temp_dir = std::env::temp_dir().join(format!("aida-upgrade-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir)?;

    println!("\nDownloading {} ...", url);
    let status = std::process::Command::new("curl")
        .args(["-fSL", "-o"])
        .arg(temp_dir.join("aida.tar.gz"))
        .arg(&url)
        .status()
        .context("Failed to invoke curl")?;
    if !status.success() {
        anyhow::bail!(
            "Download failed. Verify {} exists and you have network access.",
            url
        );
    }

    println!("Extracting...");
    let status = std::process::Command::new("tar")
        .args(["xzf"])
        .arg(temp_dir.join("aida.tar.gz"))
        .arg("-C")
        .arg(&temp_dir)
        .status()
        .context("Failed to invoke tar")?;
    if !status.success() {
        anyhow::bail!("tar extraction failed");
    }

    let dest_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Could not determine install directory"))?;
    let needs_sudo = !dest_writable(dest_dir);

    let install_from = |src: &std::path::Path, dst_name: &str| -> Result<()> {
        let dst = dest_dir.join(dst_name);
        let mut cmd = if needs_sudo {
            let mut c = std::process::Command::new("sudo");
            c.arg("install");
            c
        } else {
            std::process::Command::new("install")
        };
        cmd.args(["-m", "755"]).arg(src).arg(&dst);
        let s = cmd
            .status()
            .with_context(|| format!("Failed to install {}", dst.display()))?;
        if !s.success() {
            anyhow::bail!("install failed for {}", dst.display());
        }
        println!("  {} {}", "Installed".green(), dst.display());
        Ok(())
    };

    // Find binaries in the extracted tarball. The release workflow has
    // shipped two layouts at different times:
    //   v0.4.0 era: a single file named `aida-${target}` (the renamed
    //               aida binary; no aida-server).
    //   future:     two files `aida` and `aida-server` at top level.
    // Handle both.
    let mut installed_any = false;

    let single = temp_dir.join(format!("aida-{}", target));
    if single.is_file() {
        install_from(&single, "aida")?;
        installed_any = true;
    }

    let aida_top = temp_dir.join("aida");
    if aida_top.is_file() {
        install_from(&aida_top, "aida")?;
        installed_any = true;
    }

    let server_top = temp_dir.join("aida-server");
    if server_top.is_file() {
        install_from(&server_top, "aida-server")?;
        installed_any = true;
    }

    if !installed_any {
        // Surface what WAS in the tarball so the user can debug, instead
        // of the previous silent "OK: upgraded" lie.
        let mut entries: Vec<String> = std::fs::read_dir(&temp_dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        entries.sort();
        anyhow::bail!(
            "Extracted tarball at {} contains no aida binary I recognize.\n\
             Expected one of: aida-{}, aida.\n\
             Tarball contents: {:?}\n\
             This is an aida bug — please report.",
            temp_dir.display(),
            target,
            entries
        );
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
    println!("\n{}: upgraded to {}.", "OK".green(), tag);
    Ok(())
}

fn dest_writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(format!(".aida-upgrade-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
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
        DbCommand::Status => {
            println!(
                "{} Status is only available for git-backed stores. Use: aida --file <dir> db status",
                "!".yellow()
            );
        }
        DbCommand::WorkspaceInit { name, remote } => {
            let cwd = std::env::current_dir()?;
            let ws_name = name.as_deref().unwrap_or_else(|| {
                cwd.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
            });

            println!("{}", "Initializing AIDA workspace...".bold());

            let manifest = aida_core::workspace::init_workspace(
                &cwd,
                ws_name,
                None,
                remote.as_deref(),
            )?;

            println!();
            println!("{} Workspace '{}' initialized", "✓".green(), manifest.name);
            println!();
            println!("  {}:", "Repos discovered".bold());
            for repo in &manifest.repos {
                println!("    {} ({})", repo.path, repo.name);
            }
            println!("  {}:", "Store".bold());
            println!("    {}/", manifest.store_path);
            println!();
            println!("  {}:", "Usage from any repo".bold());
            println!("    {}", "cd <repo> && aida list".cyan());
            println!("    {}", "cd <repo> && aida add --title \"...\" --type functional".cyan());
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

            let mut created = 0usize;
            let mut updated = 0usize;
            let mut unchanged = 0usize;
            let mut skipped = 0usize;
            let mut would_create = 0usize;
            let mut would_update = 0usize;

            for artifact in &preview.artifacts {
                let full_path = root.join(&artifact.path);
                let exists = full_path.exists();

                if exists && !force && !dry_run {
                    println!(
                        "  {} {} (skipped - exists, use --force to overwrite)",
                        "~".yellow(),
                        artifact.path.display()
                    );
                    skipped += 1;
                    continue;
                }

                // Detect "no-op" updates: file exists and content already
                // matches what we'd write. Lets us tell the user "0 files
                // needed updating" instead of "all files updated".
                let already_matches = exists
                    && std::fs::read(&full_path)
                        .map(|bytes| bytes == artifact.content.as_bytes())
                        .unwrap_or(false);

                if already_matches {
                    unchanged += 1;
                    continue;
                }

                if !*dry_run {
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
                if *dry_run {
                    if exists {
                        would_update += 1;
                    } else {
                        would_create += 1;
                    }
                } else if exists {
                    updated += 1;
                } else {
                    created += 1;
                }
            }

            println!();
            if *dry_run {
                let total_changes = would_create + would_update;
                if total_changes == 0 {
                    println!(
                        "{} Already up to date — {} file(s) match templates exactly, nothing would change.",
                        "✓".green(),
                        unchanged
                    );
                } else {
                    println!(
                        "{} Dry run: would create {}, update {} ({} unchanged, {} skipped).",
                        "ℹ".blue(),
                        would_create,
                        would_update,
                        unchanged,
                        skipped
                    );
                }
            } else {
                let total_changes = created + updated;
                if total_changes == 0 && skipped == 0 {
                    println!(
                        "{} Already up to date — {} file(s) match templates exactly, nothing changed.",
                        "✓".green(),
                        unchanged
                    );
                } else {
                    println!(
                        "{} Scaffold applied: {} created, {} updated, {} unchanged{}.",
                        "✓".green(),
                        created,
                        updated,
                        unchanged,
                        if skipped > 0 {
                            format!(", {} skipped (use --force)", skipped)
                        } else {
                            String::new()
                        }
                    );
                }
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
            role,
            all,
        } => {
            let user_id = get_user(user);
            let raw_entries = storage.queue_list(&user_id, *include_completed)?;
            let store = storage.load()?;

            // Determine effective role filter:
            //   --all       → no filter (override active-role default)
            //   --role X    → filter to for_role==X (or X="any" → no filter)
            //   neither     → if AIDA_SESSION_ROLE set, filter to that
            //                 else no filter
            let role_filter: Option<String> = if *all {
                None
            } else if let Some(r) = role {
                if r == "any" {
                    None
                } else {
                    Some(r.clone())
                }
            } else {
                std::env::var("AIDA_SESSION_ROLE").ok().filter(|s| !s.is_empty())
            };

            let entries: Vec<&aida_core::QueueEntry> = match &role_filter {
                Some(r) => raw_entries
                    .iter()
                    .filter(|e| e.for_role.as_deref() == Some(r.as_str()))
                    .collect(),
                None => raw_entries.iter().collect(),
            };

            if entries.is_empty() {
                if let Some(r) = &role_filter {
                    println!(
                        "{} (no items routed to role {}; pass --all to see your full queue)",
                        "Your queue".dimmed(),
                        r.cyan()
                    );
                } else {
                    println!("{}", "Your queue is empty.".dimmed());
                }
                return Ok(());
            }

            let title = match &role_filter {
                Some(r) => format!("My Queue · role:{} ({} items)", r, entries.len()),
                None => format!("My Queue ({} items)", entries.len()),
            };
            println!("{}", title.bold());
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
                // Always surface the for_role tag in unfiltered views, OR
                // when the entry is routed somewhere different from the
                // active filter (shouldn't normally happen post-filter).
                if role_filter.is_none() {
                    if let Some(ref r) = entry.for_role {
                        print!("  {}", format!("[for:{}]", r).cyan());
                    }
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
            r#for,
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
                for_role: r#for.clone(),
            };
            storage.queue_add(entry)?;

            let spec_id = req.spec_id.as_deref().unwrap_or("???");
            let routing = match r#for {
                Some(r) => format!(" [for:{}]", r.cyan()),
                None => String::new(),
            };
            println!(
                "{} Added {} ({}) to queue{}",
                "✓".green(),
                spec_id.bold(),
                req.title,
                routing
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
        // trace:EPIC-1-001 | ai:claude
        QueueCommand::Next { role, all, user } => {
            let user_id = get_user(user);
            let raw_entries = storage.queue_list(&user_id, /* include_completed */ false)?;
            let store = storage.load()?;

            // Same role-filter logic as queue list: --all overrides; --role X
            // explicit; otherwise inherit from active role; fall through to
            // "no filter" if nothing's set.
            let role_filter: Option<String> = if *all {
                None
            } else if let Some(r) = role {
                if r == "any" { None } else { Some(r.clone()) }
            } else {
                std::env::var("AIDA_SESSION_ROLE")
                    .ok()
                    .filter(|s| !s.is_empty())
            };

            let next_entry = raw_entries
                .iter()
                .filter(|e| match &role_filter {
                    Some(r) => e.for_role.as_deref() == Some(r.as_str()),
                    None => true,
                })
                .min_by_key(|e| e.position);

            match next_entry {
                None => {
                    let scope = match &role_filter {
                        Some(r) => format!(" for role {}", r.cyan()),
                        None => String::new(),
                    };
                    println!("{} Queue is empty{}.", "Nothing to do —".dimmed(), scope);
                    println!("  ({})", "pick up new work via `aida role enter dialog` or wait for items".dimmed());
                    return Ok(());
                }
                Some(entry) => {
                    let req = store
                        .requirements
                        .iter()
                        .find(|r| r.id == entry.requirement_id);
                    let spec_id = req.and_then(|r| r.spec_id.as_deref()).unwrap_or("???");
                    let title = req.map(|r| r.title.as_str()).unwrap_or("(deleted)");
                    let status = req
                        .map(|r| format!("{}", r.status))
                        .unwrap_or_else(|| "Unknown".to_string());
                    let priority = req
                        .map(|r| format!("{}", r.priority))
                        .unwrap_or_else(|| "?".to_string());
                    let owner = req.map(|r| r.owner.as_str()).unwrap_or("");
                    let description = req.map(|r| r.description.as_str()).unwrap_or("");

                    println!("{}", "Next up:".bold());
                    println!("  {}: {}", spec_id.green().bold(), title.bold());
                    println!(
                        "  Status: {}  ·  Priority: {}{}",
                        status,
                        priority,
                        if owner.is_empty() {
                            String::new()
                        } else {
                            format!("  ·  Owner: {}", owner)
                        }
                    );
                    if let Some(ref r) = entry.for_role {
                        println!("  Routed for: {}", r.cyan());
                    }
                    if let Some(ref note) = entry.note {
                        println!("  Note: {}", note.italic());
                    }
                    if !description.is_empty() {
                        println!();
                        println!("{}", "Description (first 10 lines):".dimmed());
                        for line in description.lines().take(10) {
                            println!("  {}", line);
                        }
                        if description.lines().count() > 10 {
                            println!("  {}", "…".dimmed());
                        }
                    }
                    println!();
                    println!("{}", "Suggested commands:".dimmed());
                    println!("  {} {}    full details", "aida show".cyan(), spec_id);
                    println!(
                        "  {} {}     mark in-progress",
                        "aida edit".cyan(),
                        format!("{} --status in-progress", spec_id)
                    );
                    println!(
                        "  {} {}    when finished (marks complete + dequeues)",
                        "aida queue done".cyan(),
                        spec_id
                    );
                }
            }
        }
        // trace:EPIC-1-001 | ai:claude
        QueueCommand::Done { id, user, yes } => {
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

            let spec_id = req.spec_id.as_deref().unwrap_or("???");

            if !yes {
                eprintln!(
                    "Mark {} ({}) as completed and remove from queue?",
                    spec_id.bold(),
                    req.title
                );
                eprintln!("Type 'y' to confirm:");
                let mut answer = String::new();
                if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
                    eprintln!("Cancelled.");
                    return Ok(());
                }
                if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    eprintln!("Cancelled. Requirement and queue untouched.");
                    return Ok(());
                }
            }

            // Update status to Completed via update_atomically — works
            // across SQLite and git-canonical modes.
            let req_id = req.id;
            storage.update_atomically(|s| {
                if let Some(r) = s.requirements.iter_mut().find(|r| r.id == req_id) {
                    r.status = aida_core::RequirementStatus::Completed;
                    r.modified_at = chrono::Utc::now();
                }
            })?;
            storage.queue_remove(&user_id, &req_id)?;

            println!(
                "{} {} marked completed and removed from queue.",
                "✓".green(),
                spec_id.bold()
            );
            println!(
                "  ({})",
                "run `aida queue next` to see what's next".dimmed()
            );
        }
    }
    Ok(())
}

// trace:ARCH-github-integration | ai:claude
/// Handle GitHub integration commands
// trace:ARCH-jira-integration | ai:claude
fn handle_jira_command(cmd: &JiraCommand, storage: &Storage) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;

    match cmd {
        JiraCommand::Config { url, project, email, show, show_mapping } => {
            let mut config = aida_core::JiraConfig::load()?;

            if *show {
                println!("{}", "Jira Configuration".bold());
                println!("{}", "─".repeat(40));
                println!("Instance:  {}", if config.instance_url.is_empty() { "(not set)" } else { &config.instance_url });
                println!("Project:   {}", if config.project_key.is_empty() { "(not set)" } else { &config.project_key });
                println!("Email:     {}", if config.user_email.is_empty() { "(not set)" } else { &config.user_email });
                println!("Token:     {}", if config.effective_token().is_ok() { "configured (AIDA_JIRA_TOKEN)" } else { "not configured" });
                println!("Enabled:   {}", config.enabled);
                return Ok(());
            }

            if *show_mapping {
                println!("{}", "Field Mapping Spec".bold());
                println!("{}", "─".repeat(50));
                println!("\n{}", "Type Mapping (AIDA → Jira):".bold());
                for (aida, jira) in &config.mapping.types {
                    println!("  {:<20} → {}", aida, jira);
                }
                println!("\n{}", "Status Mapping (AIDA → Jira):".bold());
                for (aida, jira) in &config.mapping.statuses {
                    println!("  {:<20} → {}", aida, jira);
                }
                println!("\n{}", "Priority Mapping (AIDA → Jira):".bold());
                for (aida, jira) in &config.mapping.priorities {
                    println!("  {:<20} → {}", aida, jira);
                }
                println!("\n{}", "Reverse Type Mapping (Jira → AIDA):".bold());
                for (jira, aida) in &config.mapping.reverse_types {
                    println!("  {:<20} → {}", jira, aida);
                }
                println!("\n{}", "Reverse Status Mapping (Jira → AIDA):".bold());
                for (jira, aida) in &config.mapping.reverse_statuses {
                    println!("  {:<20} → {}", jira, aida);
                }
                println!("\nEdit mapping at: {}", aida_core::JiraConfig::config_path()?.display());
                return Ok(());
            }

            if let Some(u) = url { config.instance_url = u.clone(); }
            if let Some(p) = project { config.project_key = p.clone(); }
            if let Some(e) = email { config.user_email = e.clone(); }

            config.save()?;
            println!("{} Jira configuration saved.", "✓".green());
        }
        JiraCommand::Test => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config)?;
            let project = rt.block_on(client.test_connection())?;

            println!("{} Connected to Jira", "✓".green());
            println!("  Project: {} ({})", project.name, project.key);
        }
        JiraCommand::List { jql, limit } => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config.clone())?;

            let results = if let Some(query) = jql {
                rt.block_on(client.search(query, *limit))?
            } else {
                rt.block_on(client.list_issues(*limit))?
            };

            if results.issues.is_empty() {
                println!("No issues found.");
            } else {
                println!("{:<12} {:<10} {:<12} {:<10} {}", "Key", "Type", "Status", "Priority", "Summary");
                println!("{}", "─".repeat(75));
                for issue in &results.issues {
                    println!("{:<12} {:<10} {:<12} {:<10} {}",
                        issue.key,
                        truncate_str(issue.issue_type_name(), 9),
                        truncate_str(issue.status_name(), 11),
                        truncate_str(issue.priority_name(), 9),
                        truncate_str(issue.summary(), 40),
                    );
                }
                println!("\n{} issues", results.issues.len());
            }
        }
        JiraCommand::Show { key } => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config)?;
            let issue = rt.block_on(client.get_issue(key))?;

            println!("{}: {}", "Key".bold(), issue.key);
            println!("{}: {}", "Summary".bold(), issue.summary());
            println!("{}: {}", "Type".bold(), issue.issue_type_name());
            println!("{}: {}", "Status".bold(), issue.status_name());
            println!("{}: {}", "Priority".bold(), issue.priority_name());
            if let Some(assignee) = issue.assignee_name() {
                println!("{}: {}", "Assignee".bold(), assignee);
            }
            if !issue.labels().is_empty() {
                println!("{}: {}", "Labels".bold(), issue.labels().join(", "));
            }
            let desc = issue.description_text();
            if !desc.is_empty() {
                println!("\n{}", desc);
            }
        }
        JiraCommand::Push { id } => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config.clone())?;

            let store = storage.load()?;
            let req = store.requirements.iter()
                .find(|r| r.matches_id(id))
                .ok_or_else(|| anyhow::anyhow!("Requirement not found: {}", id))?;

            let display_id = req.display_id();
            let type_name = config.map_type(&format!("{:?}", req.req_type));
            let priority_name = config.map_priority(&req.effective_priority());

            let mut labels = Vec::new();
            labels.push(format!("aida:{}", display_id));
            for tag in &req.tags {
                labels.push(format!("aida:{}", tag));
            }

            let description_text = format!(
                "{}\n\n---\nAIDA: {} | UUID: {}",
                req.description, display_id, req.id
            );

            let request = aida_core::JiraCreateIssueRequest {
                fields: aida_core::JiraCreateIssueFields {
                    project: aida_core::JiraProjectRef { key: config.project_key.clone() },
                    summary: format!("[{}] {}", display_id, req.title),
                    description: Some(aida_core::text_to_adf(&description_text)),
                    issuetype: aida_core::JiraIssueTypeRef { name: type_name },
                    priority: Some(aida_core::JiraPriorityRef { name: priority_name }),
                    assignee: None,
                    labels,
                },
            };

            let created = rt.block_on(client.create_issue(&request))?;
            println!("{} Created Jira issue {} for {}",
                "✓".green(), created.key.white().bold(), display_id);
            println!("  URL: {}/browse/{}", config.instance_url, created.key);
        }
        JiraCommand::Sync { apply } => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config.clone())?;

            let store = storage.load()?;

            // Find AIDA requirements linked to Jira issues
            // Link detection: title starts with [DEV-N] or [PROJ-N], or has jira: tags
            let linked: Vec<(&Requirement, String)> = store.requirements.iter()
                .filter_map(|r| {
                    // Check [KEY-N] prefix in title
                    if r.title.starts_with('[') {
                        if let Some(end) = r.title.find(']') {
                            let key = &r.title[1..end];
                            if key.contains('-') && key.split('-').last().map(|n| n.parse::<u64>().is_ok()).unwrap_or(false) {
                                return Some((r, key.to_string()));
                            }
                        }
                    }
                    // Check jira: tags
                    for tag in &r.tags {
                        if let Some(key) = tag.strip_prefix("jira:key:") {
                            return Some((r, key.to_string()));
                        }
                    }
                    None
                })
                .collect();

            // Also find Jira issues with aida: labels that aren't linked from AIDA side
            // (TODO: tighten JQL to filter on aida:label_prefix once verified)
            let jira_issues = rt.block_on(client.search(
                &format!("project = {} ORDER BY updated DESC", config.project_key),
                50,
            )).unwrap_or_else(|_| aida_core::JiraSearchResults {
                issues: Vec::new(),
                next_page_token: None,
                is_last: Some(true),
                total: 0,
            });

            if linked.is_empty() && jira_issues.issues.is_empty() {
                println!("No linked items found.");
                println!("Link with: aida jira push FR-001 (or aida jira pull)");
                return Ok(());
            }

            println!("{}", "Jira Sync Status".bold());
            println!("{}", "─".repeat(70));

            let mut in_sync = 0;
            let mut drifted = 0;
            let mut errors = 0;

            for (req, jira_key) in &linked {
                match rt.block_on(client.get_issue(jira_key)) {
                    Ok(issue) => {
                        let mut diffs = Vec::new();

                        // Compare title (strip [KEY] prefix for comparison)
                        let aida_title = req.title
                            .strip_prefix(&format!("[{}] ", jira_key))
                            .unwrap_or(&req.title);
                        if aida_title != issue.summary() {
                            diffs.push(format!("  title: AIDA='{}' Jira='{}'",
                                truncate_str(aida_title, 25),
                                truncate_str(issue.summary(), 25)));
                        }

                        // Compare status using mapping
                        let expected_jira_status = config.map_status(&req.effective_status());
                        let actual_jira_status = issue.status_name();
                        if expected_jira_status != actual_jira_status {
                            diffs.push(format!("  status: AIDA={} (→{}) Jira={}",
                                req.effective_status(),
                                expected_jira_status,
                                actual_jira_status));
                        }

                        // Compare priority
                        let expected_priority = config.map_priority(&req.effective_priority());
                        let actual_priority = issue.priority_name();
                        if expected_priority != actual_priority {
                            diffs.push(format!("  priority: AIDA={} (→{}) Jira={}",
                                req.effective_priority(),
                                expected_priority,
                                actual_priority));
                        }

                        let spec_id = req.display_id();
                        if diffs.is_empty() {
                            in_sync += 1;
                            println!("{} {:<12} ↔ {:<10} {} — in sync",
                                "✓".green(), spec_id, jira_key, truncate_str(aida_title, 35));
                        } else {
                            drifted += 1;
                            println!("{} {:<12} ↔ {:<10} {} — DRIFTED",
                                "△".yellow(), spec_id, jira_key, truncate_str(aida_title, 35));
                            for d in &diffs {
                                println!("    {}", d);
                            }
                        }
                    }
                    Err(e) => {
                        errors += 1;
                        println!("{} {} ↔ {} — error: {}",
                            "✗".red(),
                            req.display_id(),
                            jira_key,
                            e);
                    }
                }
            }

            println!();
            println!("{} in sync, {} drifted, {} errors (of {} linked)",
                in_sync, drifted, errors, linked.len());

            if drifted > 0 && !apply {
                println!("\nUse --apply to push AIDA state to Jira.");
            }

            if *apply && drifted > 0 {
                println!("\nApplying changes...");
                for (req, jira_key) in &linked {
                    let aida_title = req.title
                        .strip_prefix(&format!("[{}] ", jira_key))
                        .unwrap_or(&req.title);

                    let fields = serde_json::json!({
                        "summary": aida_title,
                        "priority": { "name": config.map_priority(&req.effective_priority()) },
                    });

                    match rt.block_on(client.update_issue(jira_key, &fields)) {
                        Ok(_) => println!("  {} Updated {}", "✓".green(), jira_key),
                        Err(e) => eprintln!("  {} Failed {}: {}", "✗".red(), jira_key, e),
                    }
                }
            }
        }
        JiraCommand::Pull { jql, limit, dry_run } => {
            let config = aida_core::JiraConfig::load()?;
            let client = aida_core::JiraClient::new(config.clone())?;

            let query = jql.clone().unwrap_or_else(|| {
                format!("project = {} AND status != Done ORDER BY updated DESC", config.project_key)
            });
            let results = rt.block_on(client.search(&query, *limit))?;

            if results.issues.is_empty() {
                println!("No issues found.");
                return Ok(());
            }

            let store = storage.load()?;
            let existing_titles: std::collections::HashSet<String> = store.requirements.iter()
                .map(|r| r.title.clone())
                .collect();

            let mut to_import = Vec::new();
            let mut skipped = 0;

            for issue in &results.issues {
                let jira_prefix = format!("[{}]", issue.key);
                if existing_titles.contains(&issue.fields.summary)
                    || store.requirements.iter().any(|r| r.title.starts_with(&jira_prefix))
                {
                    skipped += 1;
                } else {
                    to_import.push(issue);
                }
            }

            if to_import.is_empty() {
                println!("All {} issues already imported ({} skipped).", results.issues.len(), skipped);
                return Ok(());
            }

            println!("Found {} issues to import ({} already exist):", to_import.len(), skipped);
            for issue in &to_import {
                let aida_type = config.reverse_map_type(issue.issue_type_name());
                println!("  {:<12} {:<10} {}", issue.key, aida_type, truncate_str(issue.summary(), 50));
            }

            if *dry_run {
                println!("\nDry run — no requirements created.");
                return Ok(());
            }

            let mut imported = 0;
            storage.update_atomically(|store| {
                for issue in &to_import {
                    let aida_type_str = config.reverse_map_type(issue.issue_type_name());
                    let aida_status_str = config.reverse_map_status(issue.status_name());

                    let mut req = Requirement::new(
                        format!("[{}] {}", issue.key, issue.fields.summary),
                        issue.description_text(),
                    );
                    req.req_type = parse_requirement_type(&aida_type_str).unwrap_or(RequirementType::Task);
                    req.set_status_from_str(&aida_status_str);

                    for label in issue.labels() {
                        req.tags.insert(format!("jira:{}", label));
                    }

                    let type_prefix = store.get_type_prefix(&req.req_type);
                    store.add_requirement_with_id(req, None, type_prefix.as_deref());
                    imported += 1;
                }
            })?;

            println!("\n{} Imported {} issues as requirements.", "✓".green(), imported);
        }
    }
    Ok(())
}

fn handle_github_command(cmd: &GitHubCommand, storage: &Storage) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;

    match cmd {
        GitHubCommand::Config {
            repo,
            token,
            api_url,
            show,
        } => {
            let mut config = aida_core::GitHubConfig::load()?;

            if *show {
                println!("{}", "GitHub Configuration".bold());
                println!("{}", "─".repeat(40));
                println!("API URL:  {}", config.api_url);
                println!("Repo:     {}", if config.repo.is_empty() { "(not set)" } else { &config.repo });
                println!(
                    "Token:    {}",
                    if config.effective_token().is_ok() {
                        "configured (AIDA_GITHUB_TOKEN)"
                    } else {
                        "not configured"
                    }
                );
                println!("Enabled:  {}", config.enabled);
                return Ok(());
            }

            if let Some(r) = repo {
                config.repo = r.clone();
            }
            if let Some(t) = token {
                std::env::set_var("AIDA_GITHUB_TOKEN", t);
                config.token = Some(t.clone());
                println!("{} Token set for this session. Set AIDA_GITHUB_TOKEN env var for persistence.", "!".yellow());
            }
            if let Some(u) = api_url {
                config.api_url = u.clone();
            }

            config.save()?;
            println!("{} GitHub configuration saved.", "✓".green());
        }
        GitHubCommand::Test => {
            let config = aida_core::GitHubConfig::load()?;
            config.validate()?;

            let client = aida_core::GitHubClient::new(config)?;
            let repo = rt.block_on(client.test_connection())?;

            println!("{} Connected to GitHub", "✓".green());
            println!("  Repository: {}", repo.full_name);
            println!("  URL:        {}", repo.html_url);
            println!("  Default:    {}", repo.default_branch);
            println!("  Private:    {}", repo.is_private);
        }
        GitHubCommand::List { state, labels, limit } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config)?;

            let mut filter = aida_core::GitHubIssueFilter::default();
            filter.state = Some(state.clone());
            filter.per_page = Some(*limit);
            if let Some(l) = labels {
                filter.labels = l.split(',').map(|s| s.trim().to_string()).collect();
            }

            let issues = rt.block_on(client.list_issues(Some(filter)))?;

            if issues.is_empty() {
                println!("No issues found.");
            } else {
                println!(
                    "{:<8} {:<10} {:<40} {}",
                    "#", "State", "Title", "Labels"
                );
                println!("{}", "─".repeat(75));
                for issue in &issues {
                    let labels_str: String = issue
                        .labels
                        .iter()
                        .map(|l| l.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!(
                        "{:<8} {:<10} {:<40} {}",
                        format!("#{}", issue.number),
                        issue.state,
                        truncate_str(&issue.title, 38),
                        labels_str,
                    );
                }
                println!("\n{} issues", issues.len());
            }
        }
        GitHubCommand::Show { number } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config)?;

            // Parse "GH-42" or "42"
            let num: u64 = number
                .trim_start_matches("GH-")
                .trim_start_matches("gh-")
                .trim_start_matches('#')
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid issue number: {}", number))?;

            let issue = rt.block_on(client.get_issue(num))?;

            println!("{}: #{}", "Number".bold(), issue.number);
            println!("{}: {}", "Title".bold(), issue.title);
            println!("{}: {}", "State".bold(), issue.state);
            println!("{}: {}", "Author".bold(), issue.user.login);
            if let Some(ref assignee) = issue.assignee {
                println!("{}: {}", "Assignee".bold(), assignee.login);
            }
            if !issue.labels.is_empty() {
                let labels: Vec<&str> = issue.labels.iter().map(|l| l.name.as_str()).collect();
                println!("{}: {}", "Labels".bold(), labels.join(", "));
            }
            println!("{}: {}", "URL".bold(), issue.html_url);
            println!("{}: {}", "Comments".bold(), issue.comments);
            if let Some(ref body) = issue.body {
                if !body.is_empty() {
                    println!("\n{}", body);
                }
            }
        }
        GitHubCommand::Push { id } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config.clone())?;

            let store = storage.load()?;
            let req = store
                .requirements
                .iter()
                .find(|r| r.matches_id(id))
                .ok_or_else(|| anyhow::anyhow!("Requirement not found: {}", id))?;

            // Build labels from type and priority
            let mut labels = Vec::new();
            let type_str = format!("{:?}", req.req_type);
            if let Some(label) = config.labels.types.get(&type_str) {
                labels.push(label.clone());
            }
            let priority_str = req.effective_priority();
            if let Some(label) = config.labels.priorities.get(&priority_str) {
                labels.push(label.clone());
            }
            let status_str = req.effective_status();
            if let Some(label) = config.labels.statuses.get(&status_str) {
                labels.push(label.clone());
            }

            // Build issue body with AIDA metadata
            let display_id = req.display_id();
            let body = format!(
                "{}\n\n---\n_AIDA: {} | UUID: {}_",
                req.description,
                display_id,
                req.id,
            );

            let request = aida_core::GitHubCreateIssueRequest {
                title: format!("[{}] {}", display_id, req.title),
                body: Some(body),
                labels,
                assignees: if req.owner.is_empty() {
                    Vec::new()
                } else {
                    vec![req.owner.clone()]
                },
                milestone: None,
            };

            let issue = rt.block_on(client.create_issue(&request))?;
            println!(
                "{} Created GitHub issue #{} for {}",
                "✓".green(),
                issue.number,
                display_id
            );
            println!("  URL: {}", issue.html_url);
        }
        GitHubCommand::Sync { linked_only, apply } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config)?;

            let store = storage.load()?;

            // Find AIDA requirements linked to GitHub issues (by [GH-N] prefix or URL)
            let linked: Vec<(&Requirement, u64)> = store
                .requirements
                .iter()
                .filter_map(|r| {
                    // Check [GH-N] prefix
                    if r.title.starts_with("[GH-") {
                        if let Some(end) = r.title.find(']') {
                            if let Ok(n) = r.title[4..end].parse::<u64>() {
                                return Some((r, n));
                            }
                        }
                    }
                    // Check URLs
                    for url in &r.urls {
                        if url.url.contains("github.com") && url.url.contains("/issues/") {
                            if let Some(num_str) = url.url.rsplit('/').next() {
                                if let Ok(n) = num_str.parse::<u64>() {
                                    return Some((r, n));
                                }
                            }
                        }
                    }
                    None
                })
                .collect();

            if linked.is_empty() && *linked_only {
                println!("No linked GitHub issues found.");
                println!("Link with: aida github push FR-001 (or aida github pull)");
                return Ok(());
            }

            println!("{}", "GitHub Sync Status".bold());
            println!("{}", "─".repeat(65));

            let mut drift_count = 0;

            for (req, issue_number) in &linked {
                match rt.block_on(client.get_issue(*issue_number)) {
                    Ok(issue) => {
                        let mut diffs = Vec::new();

                        // Compare title (strip [GH-N] prefix for comparison)
                        let aida_title = req.title
                            .strip_prefix(&format!("[GH-{}] ", issue_number))
                            .unwrap_or(&req.title);
                        if aida_title != issue.title {
                            diffs.push(format!("  title: AIDA='{}' GitHub='{}'",
                                truncate_str(aida_title, 30),
                                truncate_str(&issue.title, 30)));
                        }

                        // Compare state
                        let aida_closed = matches!(req.status, RequirementStatus::Completed | RequirementStatus::Rejected);
                        let gh_closed = issue.state == "closed";
                        if aida_closed != gh_closed {
                            diffs.push(format!("  state: AIDA={} GitHub={}",
                                req.effective_status(), issue.state));
                        }

                        if diffs.is_empty() {
                            println!("{} #{:<5} {} — in sync",
                                "✓".green(),
                                issue_number,
                                truncate_str(aida_title, 45));
                        } else {
                            drift_count += 1;
                            println!("{} #{:<5} {} — DRIFTED",
                                "△".yellow(),
                                issue_number,
                                truncate_str(aida_title, 45));
                            for d in &diffs {
                                println!("    {}", d);
                            }
                        }
                    }
                    Err(e) => {
                        println!("{} #{:<5} — error: {}",
                            "✗".red(), issue_number, e);
                    }
                }
            }

            println!();
            if drift_count == 0 {
                println!("All {} linked items in sync.", linked.len());
            } else {
                println!("{} of {} items have drifted.", drift_count, linked.len());
                if !apply {
                    println!("Use --apply to push AIDA state to GitHub.");
                }
            }

            if *apply && drift_count > 0 {
                println!();
                println!("Applying changes...");
                for (req, issue_number) in &linked {
                    let aida_title = req.title
                        .strip_prefix(&format!("[GH-{}] ", issue_number))
                        .unwrap_or(&req.title);

                    let aida_closed = matches!(req.status, RequirementStatus::Completed | RequirementStatus::Rejected);

                    let update = aida_core::GitHubUpdateIssueRequest {
                        title: Some(aida_title.to_string()),
                        body: Some(req.description.clone()),
                        state: Some(if aida_closed { "closed".into() } else { "open".into() }),
                        labels: None,
                        assignees: None,
                        milestone: None,
                    };

                    match rt.block_on(client.update_issue(*issue_number, &update)) {
                        Ok(_) => println!("  {} Updated #{}", "✓".green(), issue_number),
                        Err(e) => eprintln!("  {} Failed #{}: {}", "✗".red(), issue_number, e),
                    }
                }
            }
        }
        GitHubCommand::Pull {
            labels,
            open_only,
            limit,
            dry_run,
        } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config.clone())?;

            let mut filter = aida_core::GitHubIssueFilter::default();
            filter.state = Some(if *open_only { "open" } else { "all" }.into());
            filter.per_page = Some(*limit);
            if let Some(l) = labels {
                filter.labels = l.split(',').map(|s| s.trim().to_string()).collect();
            }

            let issues = rt.block_on(client.list_issues(Some(filter)))?;

            if issues.is_empty() {
                println!("No issues found to import.");
                return Ok(());
            }

            // Check which issues are already imported (by matching title pattern)
            let store = storage.load()?;
            let existing_titles: std::collections::HashSet<String> = store
                .requirements
                .iter()
                .map(|r| r.title.clone())
                .collect();

            let mut to_import: Vec<&aida_core::GitHubIssue> = Vec::new();
            let mut skipped = 0;

            for issue in &issues {
                // Skip if already imported (check for [GH-N] prefix or exact title match)
                let gh_prefix = format!("[GH-{}]", issue.number);
                let already_exists = existing_titles.contains(&issue.title)
                    || store.requirements.iter().any(|r| r.title.starts_with(&gh_prefix));

                if already_exists {
                    skipped += 1;
                } else {
                    to_import.push(issue);
                }
            }

            if to_import.is_empty() {
                println!("All {} issues already imported ({} skipped).", issues.len(), skipped);
                return Ok(());
            }

            println!(
                "Found {} issues to import ({} already exist):",
                to_import.len(),
                skipped
            );

            for issue in &to_import {
                // Determine type from labels
                let req_type = determine_type_from_labels(&issue.label_names(), &config.labels);
                let priority = determine_priority_from_labels(&issue.label_names(), &config.labels);

                println!(
                    "  #{:<6} {:<12} {:<8} {}",
                    issue.number,
                    format!("{:?}", req_type),
                    format!("{:?}", priority),
                    truncate_str(&issue.title, 50),
                );
            }

            if *dry_run {
                println!("\nDry run — no requirements created.");
                return Ok(());
            }

            // Import
            let mut imported = 0;
            storage.update_atomically(|store| {
                for issue in &to_import {
                    let req_type = determine_type_from_labels(&issue.label_names(), &config.labels);
                    let priority = determine_priority_from_labels(&issue.label_names(), &config.labels);

                    let mut req = Requirement::new(
                        format!("[GH-{}] {}", issue.number, issue.title),
                        issue.body.clone().unwrap_or_default(),
                    );
                    req.req_type = req_type;
                    req.priority = priority;
                    if let Some(ref assignee) = issue.assignee {
                        req.owner = assignee.login.clone();
                    }
                    // Map GitHub state to AIDA status
                    if issue.state == "closed" {
                        req.status = RequirementStatus::Completed;
                    }
                    // Add GitHub labels as tags
                    for label in &issue.labels {
                        req.tags.insert(format!("gh:{}", label.name));
                    }
                    // Add URL link
                    req.urls.push(aida_core::models::UrlLink {
                        id: Uuid::now_v7(),
                        url: issue.html_url.clone(),
                        title: format!("GitHub #{}", issue.number),
                        description: None,
                        open_mode: aida_core::models::UrlOpenMode::NewTab,
                        added_at: chrono::Utc::now(),
                        added_by: "github-import".to_string(),
                        last_verified: None,
                        last_verified_ok: None,
                    });

                    let type_prefix = store.get_type_prefix(&req.req_type);
                    store.add_requirement_with_id(req, None, type_prefix.as_deref());
                    imported += 1;
                }
            })?;

            println!(
                "\n{} Imported {} issues as requirements.",
                "✓".green(),
                imported
            );
        }
        GitHubCommand::Labels { create_missing } => {
            let config = aida_core::GitHubConfig::load()?;
            let client = aida_core::GitHubClient::new(config.clone())?;

            let existing = rt.block_on(client.list_labels())?;
            let existing_names: std::collections::HashSet<String> =
                existing.iter().map(|l| l.name.clone()).collect();

            println!("{}", "Repository Labels".bold());
            println!("{}", "─".repeat(40));
            for label in &existing {
                println!("  {} (#{}) {}", label.name, label.color,
                    label.description.as_deref().unwrap_or(""));
            }
            println!("\n{} labels", existing.len());

            if *create_missing {
                let all_labels: Vec<(&str, &str)> = config
                    .labels
                    .types
                    .values()
                    .map(|v| (v.as_str(), "0e8a16"))
                    .chain(
                        config
                            .labels
                            .priorities
                            .values()
                            .map(|v| (v.as_str(), "d93f0b")),
                    )
                    .chain(
                        config
                            .labels
                            .statuses
                            .values()
                            .map(|v| (v.as_str(), "1d76db")),
                    )
                    .collect();

                let mut created = 0;
                for (name, color) in all_labels {
                    if !existing_names.contains(name) {
                        match rt.block_on(client.create_label(name, color, Some("Created by AIDA")))
                        {
                            Ok(_) => {
                                println!("  {} Created label: {}", "✓".green(), name);
                                created += 1;
                            }
                            Err(e) => {
                                eprintln!("  {} Failed to create {}: {}", "✗".red(), name, e);
                            }
                        }
                    }
                }
                if created == 0 {
                    println!("\nAll AIDA labels already exist.");
                } else {
                    println!("\n{} labels created.", created);
                }
            }
        }
    }

    Ok(())
}

/// Determine AIDA requirement type from GitHub labels.
fn determine_type_from_labels(
    labels: &[&str],
    label_config: &aida_core::GitHubLabelConfig,
) -> RequirementType {
    // Check each label against the type mappings (reverse lookup)
    for label in labels {
        for (type_name, mapped_label) in &label_config.types {
            if label.eq_ignore_ascii_case(mapped_label) {
                return match type_name.as_str() {
                    "Bug" => RequirementType::Bug,
                    "Story" => RequirementType::Story,
                    "Task" => RequirementType::Task,
                    "Epic" => RequirementType::Epic,
                    "Functional" => RequirementType::Functional,
                    "NonFunctional" => RequirementType::NonFunctional,
                    _ => RequirementType::Task,
                };
            }
        }
        // Also check common GitHub labels directly
        let l = label.to_lowercase();
        if l == "bug" { return RequirementType::Bug; }
        if l == "enhancement" || l == "feature" { return RequirementType::Story; }
    }
    RequirementType::Task // default
}

/// Determine AIDA priority from GitHub labels.
fn determine_priority_from_labels(
    labels: &[&str],
    label_config: &aida_core::GitHubLabelConfig,
) -> RequirementPriority {
    for label in labels {
        for (priority_name, mapped_label) in &label_config.priorities {
            if label.eq_ignore_ascii_case(mapped_label) {
                return match priority_name.as_str() {
                    "High" => RequirementPriority::High,
                    "Low" => RequirementPriority::Low,
                    _ => RequirementPriority::Medium,
                };
            }
        }
    }
    RequirementPriority::Medium // default
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
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
            use aida_core::{LinkOrigin, SyncStatus};

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
                let (status_icon, _status_color) = match state.sync_status {
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
