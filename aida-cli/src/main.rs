mod cli;
mod docs;
mod global_queue;
mod history;
mod not_found;
mod session;
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
    BlockCommand, CacheCommand, Cli, Command, CommentCommand, ConfigCommand, DbCommand, DevCommand,
    DocsCommand, FeatureCommand, GitHubCommand, GitLabCommand, JiraCommand, NodeCommand,
    QueueCommand, RelDefCommand, RelationshipCommand, ReportCommand, ReviewCommand, RoleCommand,
    RolePromptCommand, RoleScopeCommand, ScaffoldCommand, ServerCommand, SessionCommand,
    TraceCommand, TypeCommand,
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
        verbose,
        name,
    } = &cli.command
    {
        // Default: distributed (git-canonical) mode per EPIC-1-001.
        // --sibling implies distributed-sibling. --centralized opts into
        // the deprecated SQLite-canonical path.
        // trace:EPIC-1-001 | ai:claude
        if *centralized {
            handle_init_command(*no_skills, agent, *no_hooks, *force, *verbose, name.as_deref())?;
        } else if *sibling {
            handle_init_distributed_sibling(
                registry_remote.as_deref(),
                *force,
                *no_skills,
                agent,
                *no_hooks,
                *verbose,
                name.as_deref(),
            )?;
        } else {
            handle_init_distributed_worktree(
                *force, *no_skills, agent, *no_hooks, *verbose, name.as_deref(),
            )?;
        }
        return Ok(());
    }

    // Handle upgrade before storage resolution — it needs no DB.
    // trace:EPIC-1-001 | ai:claude
    if let Command::Upgrade { check, version, yes, target, diff } = &cli.command {
        return handle_upgrade_command(*check, version.as_deref(), *yes, target.as_deref(), *diff);
    }

    // Handle dev commands before storage resolution — most need no DB.
    // (Dev::Serve does interact with storage but spawns aida-server which
    // handles that itself; the wrapper just supervises the children.)
    // trace:EPIC-1-001 | ai:claude
    if let Command::Dev(dev_cmd) = &cli.command {
        return handle_dev_command(dev_cmd);
    }

    // Doctor commands run before storage init — they may need to operate
    // on broken or partially-migrated stores. trace:EPIC-19 | ai:claude
    if let Command::Doctor(doctor_cmd) = &cli.command {
        return handle_doctor_command(doctor_cmd);
    }

    // Store commands inspect git state, no AIDA storage needed.
    // trace:EPIC-21 | ai:claude
    if let Command::Store(store_cmd) = &cli.command {
        return handle_store_command(store_cmd);
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
    if let Command::Statusline { color } = &cli.command {
        return handle_statusline_command(color);
    }
    // trace:FR-1-043 | ai:claude
    if let Command::Session(session_cmd) = &cli.command {
        return handle_session_command(session_cmd);
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
            force_parent,
            interactive,
        } => {
            // trace:BUG-17 | ai:claude — resolve description from inline,
            // file, or stdin sources before dispatching.
            let resolved_description =
                resolve_description(description, description_from_file, *description_stdin)?;
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
        Command::List {
            status,
            priority,
            r#type,
            feature,
            tags,
            ..
        } => {
            // Legacy SQLite path doesn't honor role scope (deprecated backend).
            list_requirements(&storage, status, priority, r#type, feature, tags)?;
        }
        Command::Show { id, .. } => {
            // Legacy SQLite show_requirement always prints comments inline,
            // so the --comments flag is a no-op here. Git backend honors it.
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
            strict: _, // legacy SQLite path ignores session leases
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
        Command::Node(_) => {
            anyhow::bail!(
                "aida node commands are only available in git-canonical (distributed) mode. \
                 Run `aida init` (defaults to distributed) first."
            );
        }
        Command::Status { no_dev_context } => {
            handle_status_command(*no_dev_context, None, &storage)?;
        }
        Command::Push { .. } => {
            anyhow::bail!(
                "`aida push` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Pull { .. } => {
            anyhow::bail!(
                "`aida pull` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Upgrade { .. } => unreachable!("upgrade is dispatched before storage init"),
        Command::Dev(_) => unreachable!("dev is dispatched before storage init"),
        Command::Doctor(_) => unreachable!("doctor is dispatched before storage init"),
        Command::Store(_) => unreachable!("store is dispatched before storage init"),
        Command::HelpAll => unreachable!("help-all is dispatched before storage init"),
        Command::Role(_) => unreachable!("role is dispatched before storage init"),
        Command::Statusline { .. } => unreachable!("statusline is dispatched before storage init"),
        Command::Session(_) => unreachable!("session is dispatched before storage init"),
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
        Command::Review(review_cmd) => {
            handle_review_command(review_cmd, &storage)?;
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
        Command::Docs(docs_cmd) => {
            // trace:FR-1-077 | ai:claude
            handle_docs_command(docs_cmd, &storage)?;
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
        Command::Init { .. } => {
            // Handled before path resolution above; unreachable
            unreachable!("Init command should be handled before path resolution");
        }
        Command::History { .. } => {
            // History walks the orphan branch; only meaningful in git-canonical
            // mode, which is dispatched via handle_git_backend_command. Falling
            // through to legacy means the user is on a SQLite-only project.
            anyhow::bail!(
                "aida history requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
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

fn handle_init_command(
    no_skills: bool,
    agent: &str,
    no_hooks: bool,
    force: bool,
    verbose: bool,
    _name: Option<&str>,
) -> Result<()> {
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
        verbose,
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
    verbose: bool,
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
        // Hooks (and any executable shell-style scripts) need the exec
        // bit; without it git silently ignores commit-msg hooks and the
        // user gets a confusing "hook was ignored" warning on every
        // commit. trace:BUG-21 | ai:claude
        ensure_executable_if_hook(&artifact.path, &full_path);

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

    // Print post-init message. Brief by default; --verbose for the full
    // file inventory + per-agent hint blocks. trace:BUG-19 | ai:claude
    println!();
    println!("{}", "AIDA initialized ✓".green().bold());

    if verbose {
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
    } else {
        // Brief: one line of storage location + counts.
        println!("  Storage: {}", storage_label.dimmed());
    }

    if skipped_count > 0 {
        println!(
            "  {} files skipped (already exist, use --force to overwrite)",
            skipped_count.to_string().yellow(),
        );
    }
    if updated_count > 0 {
        println!("  {} files updated", updated_count.to_string().blue(),);
    }

    println!();
    println!("  {}:", "Next".bold());
    println!(
        "    {}{}capture project intent",
        "aida add --type vision --title \"...\"".cyan(),
        " ".repeat(2)
    );
    println!(
        "    {}{}see what exists",
        "aida list".cyan(),
        " ".repeat(29)
    );
    println!(
        "    {}{}project as layered docs",
        "aida docs build".cyan(),
        " ".repeat(23)
    );

    if !verbose {
        println!();
        // BUG-38: the prior hint pointed at re-running `aida init
        // --verbose`, but that fails post-init with "already initialized".
        // Steer the user to `aida scaffold status --verbose` instead — it
        // works any time and reports what's actually on disk.
        // trace:BUG-38 | ai:claude
        println!(
            "  {}",
            "(later: `aida scaffold status --verbose` to see every scaffolded file)".dimmed()
        );
    } else if config_for_output.generate_claude_md {
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
        println!(
            "    {}{}See all available skills",
            "/aida-".cyan(),
            " ".repeat(22)
        );
    }
    println!();

    Ok(())
}

/// Detect if the current directory has a distributed store configured.
/// Walks up from CWD looking for `.aida/config.toml` with a store_path.
/// trace:BUG-57 | ai:claude
fn detect_distributed_store() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    detect_distributed_store_from(&cwd)
}

/// Walk-up resolver split out from `detect_distributed_store` so the search
/// path is testable without changing process cwd. Returns the absolute store
/// path on the first ancestor whose `.aida/config.toml` declares one.
/// trace:BUG-57 | ai:claude
fn detect_distributed_store_from(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut current = start;
    loop {
        let config_path = current.join(".aida").join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            // store_path is relative to the directory containing config.toml,
            // not to the original cwd — otherwise `aida edit` from a subdir
            // would resolve the store against the wrong base.
            for line in content.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("store_path") {
                    if let Some(val) = rest.split('=').nth(1) {
                        let val = val.trim().trim_matches('"').trim_matches('\'');
                        let store_path = current.join(val);
                        if store_path.exists() && store_path.is_dir() {
                            return Some(store_path);
                        }
                    }
                }
            }
        }
        match current.parent() {
            Some(p) => current = p,
            None => return None,
        }
    }
}

/// Read the `[id_format] policy` from `.aida/config.toml`. Honors the legacy
/// `use_agreed_blocks` boolean as a fallback when the new section is missing
/// (so existing projects keep working unchanged).
///
/// Resolution order:
///   1. `[id_format] policy = "..."`  → parsed (errors on unknown values)
///   2. legacy `use_agreed_blocks = false`  → `node-aware-only`
///   3. legacy `use_agreed_blocks = true`   → `blocks-then-fallback`
///   4. neither set                          → `blocks-then-fallback` (default)
/// trace:EPIC-1-052 | ai:claude
/// Read the agreed-id counter for a given type from the orphan store's
/// `registry/agreed_counters.toml`. Returns 0 when the file doesn't exist
/// or the type has no entry — both mean "no ids issued yet for this type".
/// Used as the floor when claiming a new block so the block doesn't
/// overlap with already-issued agreed-ids.
/// trace:FR-1-073 | ai:claude
fn read_agreed_counter(store_path: &std::path::Path, type_prefix: &str) -> u32 {
    let path = store_path.join("registry").join("agreed_counters.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return 0;
    };
    let prefix_upper = type_prefix.to_uppercase();
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let (key, val) = match (parts.next(), parts.next()) {
            (Some(k), Some(v)) => (k.trim().trim_matches('"'), v.trim()),
            _ => continue,
        };
        if key.eq_ignore_ascii_case(&prefix_upper) {
            return val.parse::<u32>().unwrap_or(0);
        }
    }
    0
}

fn read_id_format_policy(project_dir: &std::path::Path) -> aida_core::IdFormatPolicy {
    read_id_format_settings(project_dir).0
}

/// Read counter_scope from `.aida/config.toml`. When absent, defaults to
/// PerType (back-compat — flipping a live store would conflate FR-100
/// and BUG-100 numerically). Projects created from 2026-05-09 onwards
/// have `counter_scope = "global"` written explicitly at init.
/// trace:FR-271 | ai:claude
#[allow(dead_code)]
fn read_id_counter_scope(project_dir: &std::path::Path) -> aida_core::IdCounterScope {
    read_id_format_settings(project_dir).1
}

/// Single pass over `.aida/config.toml` returning both the policy and
/// the counter scope. Cheaper than two separate reads when a caller
/// needs both. trace:FR-271 | ai:claude
fn read_id_format_settings(
    project_dir: &std::path::Path,
) -> (aida_core::IdFormatPolicy, aida_core::IdCounterScope) {
    let config_path = project_dir.join(".aida").join("config.toml");
    let default_policy = aida_core::IdFormatPolicy::default();
    let default_scope = aida_core::IdCounterScope::default();
    let Ok(content) = std::fs::read_to_string(&config_path) else {
        return (default_policy, default_scope);
    };

    let mut in_id_format = false;
    let mut policy_explicit: Option<aida_core::IdFormatPolicy> = None;
    let mut scope_explicit: Option<aida_core::IdCounterScope> = None;
    let mut legacy_use_blocks: Option<bool> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_id_format = line == "[id_format]";
            continue;
        }
        if in_id_format && line.starts_with("policy") {
            if let Some(val) = line.split('=').nth(1) {
                let s = val.trim().trim_matches('"').trim_matches('\'');
                match aida_core::IdFormatPolicy::parse(s) {
                    Ok(p) => policy_explicit = Some(p),
                    Err(e) => eprintln!("Warning: {} — using default", e),
                }
            }
        }
        if in_id_format && line.starts_with("counter_scope") {
            if let Some(val) = line.split('=').nth(1) {
                let s = val.trim().trim_matches('"').trim_matches('\'');
                match aida_core::IdCounterScope::parse(s) {
                    Ok(c) => scope_explicit = Some(c),
                    Err(e) => eprintln!("Warning: {} — using default", e),
                }
            }
        }
        if !in_id_format && line.starts_with("use_agreed_blocks") {
            if let Some(val) = line.split('=').nth(1) {
                legacy_use_blocks = Some(val.trim() != "false");
            }
        }
    }

    let policy = policy_explicit.unwrap_or_else(|| match legacy_use_blocks {
        Some(false) => aida_core::IdFormatPolicy::NodeAwareOnly,
        Some(true) => aida_core::IdFormatPolicy::BlocksThenFallback,
        None => default_policy,
    });
    let scope = scope_explicit.unwrap_or(default_scope);
    (policy, scope)
}

/// Read node_id from the store's node.toml; defaults to 1 for unregistered nodes.
fn load_node_id(store_path: &std::path::Path) -> String {
    use aida_core::NodeConfig;
    let node_config_path = store_path.join(".aida").join("node.toml");
    if node_config_path.exists() {
        NodeConfig::load(&node_config_path)
            .map(|c| c.node_id)
            .unwrap_or_else(|_| "1".to_string())
    } else {
        // Fall back to dispenser.toml node_id (still a stringified-numeric
        // for legacy stores written before EPIC-9). trace:STORY-41
        let dispenser_path = store_path.join(".aida").join("dispenser.toml");
        if dispenser_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&dispenser_path) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.starts_with("node_id") {
                        if let Some(val) = line.split('=').nth(1) {
                            let trimmed = val.trim().trim_matches('"');
                            if !trimmed.is_empty() {
                                return trimmed.to_string();
                            }
                        }
                    }
                }
            }
        }
        "1".to_string()
    }
}

/// Get the local hostname for informational block registry labels.
fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
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

    // Load node_id from config, or default to "1" for local-only.
    // trace:STORY-41 | ai:claude
    let node_id: String = if node_config_path.exists() {
        NodeConfig::load(&node_config_path)?.node_id
    } else {
        "1".to_string() // default for unregistered local node
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
    // STORY-43: warn loudly when this clone has been hijacked. Runs once
    // per invocation before any command executes. The marker doesn't block
    // operations — issued ids remain valid — but the user needs to know
    // they're operating with a node id that no longer belongs to them.
    let marker_path = aida_core::node::HijackMarker::path_in_store(store_path);
    if let Ok(Some(marker)) = aida_core::node::HijackMarker::load(&marker_path) {
        eprintln!(
            "{} This clone's node id '{}' was reassigned by another clone on \
             {} at {}",
            "HIJACK WARNING:".red().bold(),
            marker.node_id,
            marker.new_owner_hostname,
            marker.hijacked_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M %Z"),
        );
        if let Some(p) = &marker.new_owner_clone_path {
            eprintln!("  New owner clone: {}", p.display());
        }
        eprintln!(
            "  Continuing to write requirements here will issue ids attributed to \
             the new owner. Run `aida node acquire` to claim a fresh id, or delete \
             {} once you've migrated.",
            marker_path.display()
        );
        eprintln!();
    }

    let dispenser = load_dispenser(store_path)?;
    let inner = aida_core::GitBackend::new(store_path)?
        .with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    match command {
        Command::Cache(cache_cmd) => {
            return handle_cache_command(cache_cmd, &backend);
        }
        Command::Node(node_cmd) => {
            return handle_node_command(node_cmd, store_path);
        }
        Command::Docs(docs_cmd) => {
            // trace:FR-1-077 | ai:claude
            let store = backend.load()?;
            return handle_docs_with_store(docs_cmd, &store);
        }
        Command::Status { no_dev_context } => {
            return handle_status_command_distributed(*no_dev_context, store_path, &backend);
        }
        Command::Push { code_only, store_only, message } => {
            return handle_push_command(store_path, *code_only, *store_only, message.as_deref());
        }
        Command::Pull { code_only, store_only } => {
            return handle_pull_command(store_path, *code_only, *store_only);
        }
        Command::Upgrade { .. } => unreachable!("upgrade is dispatched before storage init"),
        Command::Dev(_) => unreachable!("dev is dispatched before storage init"),
        Command::Doctor(_) => unreachable!("doctor is dispatched before storage init"),
        Command::Store(_) => unreachable!("store is dispatched before storage init"),
        Command::HelpAll => unreachable!("help-all is dispatched before storage init"),
        Command::Role(_) => unreachable!("role is dispatched before storage init"),
        Command::Statusline { .. } => unreachable!("statusline is dispatched before storage init"),
        Command::Session(_) => unreachable!("session is dispatched before storage init"),
        Command::List { status, r#type, feature, tags, no_scope, show_origin, include_meta, parent, .. } => {
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
                .map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                .unwrap_or_default();
            let scope = if *no_scope { None } else { active_role_scope() };
            let (effective_tags, effective_status) = match scope {
                Some((scope_tags, scope_status)) => {
                    let final_tags = if !cli_tags.is_empty() { cli_tags } else { scope_tags };
                    let final_status = status.clone().or(scope_status);
                    (final_tags, final_status)
                }
                None => (cli_tags, status.clone()),
            };
            let filter = aida_core::ListFilter {
                status: effective_status,
                req_type: r#type.clone(),
                feature: feature.clone(),
                tags: effective_tags,
                ..Default::default()
            };
            let mut reqs = backend.list_summaries(&filter)?;

            // STORY-62: --parent <id> restricts to direct children of <id>.
            // We don't materialize a parent->children index in the cache;
            // for one parent it's a single YAML read to grab the
            // relationships array, which is fast enough for the
            // interactive `aida list` cadence.
            // trace:STORY-62 | ai:claude
            if let Some(parent_ref) = parent {
                let parent_req = backend.get_requirement_by_spec_id(parent_ref)?
                    .ok_or_else(|| anyhow::anyhow!(
                        "--parent {}: requirement not found", parent_ref
                    ))?;
                let child_ids: HashSet<Uuid> = parent_req.relationships
                    .iter()
                    .filter(|r| r.rel_type == RelationshipType::Parent)
                    .map(|r| r.target_id)
                    .collect();
                reqs.retain(|r| child_ids.contains(&r.id));
            }

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

            if reqs.is_empty() {
                println!("No requirements found.");
            } else {
                // Default rendering: one ID column (canonical = agreed_id
                // when present, else spec_id). Pass --show-origin to
                // surface the original spec_id alongside as "Origin ID".
                // Replaces the older two-column-by-default layout where
                // both columns were FR-NNN-shaped and confusing to grep
                // against. trace:FR-1-070 | ai:claude
                if *show_origin {
                    println!(
                        "{:<12} {:<14} {:<12} {:<10} {}",
                        "ID", "Origin ID", "Type", "Status", "Title"
                    );
                    println!("{}", "─".repeat(78));
                    for req in &reqs {
                        let display_id = req
                            .agreed_id
                            .as_deref()
                            .or(req.spec_id.as_deref())
                            .unwrap_or("?");
                        let origin = req.spec_id.as_deref().unwrap_or("-");
                        // Pad to visible width FIRST, then color. Otherwise
                        // .dimmed()'s ANSI escapes inflate the string length
                        // and {:<14} ends up padding on byte count, breaking
                        // column alignment. trace:FR-1-070 | ai:claude
                        let origin_padded = format!("{:<14}", origin);
                        let origin_cell = if origin == display_id {
                            origin_padded.dimmed().to_string()
                        } else {
                            origin_padded
                        };
                        println!(
                            "{:<12} {}{:<12} {:<10} {}",
                            display_id,
                            origin_cell,
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
                            .agreed_id
                            .as_deref()
                            .or(req.spec_id.as_deref())
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
            description_from_file,
            description_stdin,
            status,
            priority,
            r#type,
            owner,
            tags,
            prefix,
            parent,
            force_parent,
            interactive,
            ..
        } => {
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
            // trace:BUG-22 | ai:claude
            if let Some(msg) = suspicious_title_signal(&title_resolved) {
                eprintln!("{} {}", "Warning:".yellow().bold(), msg);
            }

            // Type — interactive picker when not provided.
            let interactive_type: Option<String> = if r#type.is_none() && interactive_mode {
                let choices = vec![
                    "functional",
                    "bug",
                    "principle",
                    "vision",
                    "decision",
                    "constraint",
                    "term",
                    "non-functional",
                    "epic",
                    "story",
                    "task",
                    "spike",
                    "sprint",
                    "system",
                    "user",
                    "folder",
                    "meta",
                ];
                let pick = inquire::Select::new("Type:", choices)
                    .with_help_message("functional / bug for everyday work; principle / vision / decision for the docs layer.")
                    .prompt()
                    .context("Type prompt cancelled")?;
                Some(pick.to_string())
            } else {
                None
            };
            let effective_type: Option<String> = r#type.clone().or(interactive_type);

            // Description — open the user's $EDITOR when not provided.
            // trace:BUG-17 | ai:claude
            let resolved_description = if interactive_mode
                && description.is_none()
                && description_from_file.is_none()
                && !*description_stdin
            {
                let body = inquire::Editor::new("Description")
                    .with_help_message("Multi-line. Save + close the editor to continue. Leave empty to skip.")
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
            let interactive_priority: Option<String> = if priority.is_none() && interactive_mode {
                let pick = inquire::Select::new(
                    "Priority:",
                    vec!["medium", "high", "low"],
                )
                .with_help_message("medium covers most things; high for blockers; low for nice-to-haves.")
                .prompt()
                .context("Priority prompt cancelled")?;
                Some(pick.to_string())
            } else {
                None
            };
            let effective_priority: Option<String> = priority.clone().or(interactive_priority);

            let mut req = Requirement::new(
                title_resolved,
                resolved_description.unwrap_or_default(),
            );
            if let Some(s) = status {
                let canonical = validate_status_input(s).map_err(|e| anyhow::anyhow!(e))?;
                req.set_status_from_str(canonical);
            }
            if let Some(p) = &effective_priority {
                let canonical = validate_priority_input(p).map_err(|e| anyhow::anyhow!(e))?;
                req.set_priority_from_str(canonical);
            }
            if let Some(t) = &effective_type {
                // BUG-48: surface the error instead of dropping silently.
                let rt = parse_requirement_type(t).map_err(|e| {
                    anyhow::anyhow!(
                        "{} — expected one of: functional, non-functional, system, user, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term",
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

            // Try to assign an agreed ID from a pre-allocated block (FR-2-005).
            // Behavior is governed by [id_format] policy in .aida/config.toml:
            //   - node-aware-only:      skip block dispense entirely
            //   - blocks-then-fallback: try block; fall through silently if missing
            //   - blocks-only:          require a block; error if missing
            // trace:EPIC-1-052 Phase 2 | ai:claude
            let project_dir = std::env::current_dir().unwrap_or_default();
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

                    if let Ok(mut registry) = aida_core::BlockRegistry::load(&blocks_path) {
                        match registry.find_active_block_or_global(&node_id, &type_prefix) {
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
                                        node_id, type_prefix, type_prefix
                                    );
                                }
                            }
                            Some(idx) if registry.blocks[idx].is_exhausted() => {
                                anyhow::bail!(
                                    "Agreed ID block for {} exhausted on node {}. \
                                     Run `aida db block claim --type {}` to allocate a new block (requires network).",
                                    type_prefix, node_id, type_prefix
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
                                if let Some((agreed_id, is_low)) = dispensed {
                                    if is_low {
                                        eprintln!(
                                            "{} {} block running low ({} remaining). Run `aida db block claim --type {}` soon.",
                                            "WARNING:".yellow().bold(),
                                            type_prefix,
                                            registry.find_active_block(&node_id, &type_prefix)
                                                .map(|i| registry.blocks[i].remaining())
                                                .unwrap_or(0),
                                            type_prefix
                                        );
                                    }
                                    // Persist the updated next pointer
                                    if let Err(e) = registry.save(&blocks_path) {
                                        eprintln!("Warning: could not save blocks.yaml: {}", e);
                                    } else {
                                        // Commit the pointer advance to the store
                                        let _ = aida_core::git_ops::add(store_path, &["registry/blocks.yaml"]);
                                    }
                                    req.agreed_id = Some(agreed_id.clone());
                                    // Use the agreed ID as the spec_id so it is immediately
                                    // visible as the primary identifier.
                                    req.spec_id = Some(agreed_id);
                                }
                            }
                        }
                    }
                }
            }

            // BUG-62: pre-resolve --parent BEFORE writing the child. Two
            // gaps in the original post-hoc flow: (1) the lookup only
            // consulted spec_id/agreed_id, silently rejecting UUID input
            // even though FR-215's acceptance says "accepts SPEC-ID or
            // UUID"; (2) when the lookup failed, the child file was
            // already on disk with no parent edge, leaving an orphan.
            // Resolve here (UUID → spec_id/agreed_id), and run the
            // STORY-48 lease enforcement up-front too — both gates fire
            // before any write so a blocked or invalid --parent leaves
            // the store untouched. The actual relationship insert still
            // happens post-write because the child has to exist before
            // it can receive a Child edge. trace:BUG-62, FR-215 | ai:claude
            let parent_req: Option<aida_core::models::Requirement> = if let Some(parent_str) = parent {
                let resolved = if let Ok(uuid) = uuid::Uuid::parse_str(parent_str) {
                    backend.get_requirement(&uuid)?
                } else {
                    backend.get_requirement_by_spec_id(parent_str)?
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
            // BUG-64: refuse `--parent <X>` when X is in a terminal status
            // (Completed or Rejected). Filing a new child under a closed
            // epic produces a confusing graph (closed parent with open
            // children) that surfaces later in `aida list --parent` and
            // `aida show --tree`. `--force-parent` bypasses for the
            // legitimate backfill case. trace:BUG-64 | ai:claude
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
            }
            if let Some(ref pr) = parent_req {
                if let Ok(project_root) = find_project_root() {
                    if !list_leases(&project_root).is_empty() {
                        let store = backend.load()?;
                        enforce_session_lease(
                            &project_root,
                            pr,
                            &store,
                            "aida add --parent",
                            false,
                        )?;
                    }
                }
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
                    if let Ok(project_root) = find_project_root() {
                        if let Some(lease) = std::env::current_dir()
                            .ok()
                            .and_then(|cwd| active_lease_for_cwd(&project_root, &cwd))
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
                    let mut child = last.clone();
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
            }
        }
        Command::Show { id, comments, tree, depth } => {
            record_role_activity(id, "show");
            // STORY-62: --tree replaces the detail view with an indented
            // hierarchy walk. Children are read via rel_type:Parent edges
            // on the parent's record; recursion descends until depth is
            // exhausted or no more children. Hidden tradeoff: each level
            // hits one more YAML read per visited node, but for typical
            // EPIC trees (~50 nodes max) that's fine, and the user opts
            // in. trace:STORY-62 | ai:claude
            if *tree {
                match backend.get_requirement_by_spec_id(id)? {
                    Some(root) => render_tree(&backend, &root, *depth)?,
                    None => {
                        eprintln!(
                            "{}",
                            not_found::requirement_not_found(id, Some(store_path))
                        );
                    }
                }
                return Ok(());
            }
            match backend.get_requirement_by_spec_id(id)? {
                Some(req) => {
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
                    if *comments && !req.comments.is_empty() {
                        println!("\n{}:", "Comments".green().bold());
                        for c in &req.comments {
                            print_comment(c, 0);
                        }
                    }
                }
                None => {
                    eprintln!("{}", not_found::requirement_not_found(id, Some(store_path)));
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
            strict,
            ..
        } => {
            record_role_activity(id, "edit");
            let mut req = backend
                .get_requirement_by_spec_id(id)?
                .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;

            // STORY-48: lease enforcement. Find leases relative to the git
            // project root (parent of the orphan store), load the full
            // store once for ancestor walking, and consult the
            // [session].enforcement knob. Best-effort — if we can't even
            // find a project root, skip enforcement rather than break edit.
            // trace:STORY-48 | ai:claude
            if let Ok(project_root) = find_project_root() {
                if !list_leases(&project_root).is_empty() {
                    let store = backend.load()?;
                    enforce_session_lease(&project_root, &req, &store, "aida edit", *strict)?;
                }
            }

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
                let canonical = validate_status_input(s).map_err(|e| anyhow::anyhow!(e))?;
                req.set_status_from_str(canonical);
                changed = true;
            }
            if let Some(p) = priority {
                let canonical = validate_priority_input(p).map_err(|e| anyhow::anyhow!(e))?;
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
                        "{} — expected one of: functional, non-functional, system, user, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term",
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
                    return Ok(());
                }
            }

            backend.delete_requirement(&req.id)?;
            println!("Deleted: {}", id);
        }
        Command::Search { query, status, limit, .. } => {
            // Cache-backed FTS5 search (EPIC-1-001 Phase 2). Replaces a
            // full-store load + in-memory substring scan.
            // trace:EPIC-1-001 | ai:claude
            let mut results = backend.search(query, *limit)?;
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
            content_positional,
            author,
            ..
        }) => {
            record_role_activity(req_id, "comment");
            let mut req = backend
                .get_requirement_by_spec_id(req_id)?
                .ok_or_else(|| not_found::requirement_not_found(req_id, Some(store_path)))?;

            // `aida comment add <REQ> "text"` — the text comes through as
            // `content_positional`. Earlier the git-backend dispatch only
            // looked at `--content`, so positional invocations silently
            // wrote empty comments. trace:BUG-28 | ai:claude
            let body = content
                .clone()
                .or_else(|| content_positional.clone())
                .unwrap_or_default();
            if body.trim().is_empty() {
                anyhow::bail!(
                    "comment body required: pass it positionally `aida comment add {} \"...\"` \
                     or via `--content`",
                    req_id
                );
            }

            let now = chrono::Utc::now();
            let comment = aida_core::Comment {
                id: Uuid::now_v7(),
                content: body,
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
        Command::Comment(CommentCommand::List { id }) => {
            // trace:TASK-1-020 | ai:claude
            record_role_activity(id, "show");
            let req = backend
                .get_requirement_by_spec_id(id)?
                .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;
            println!("{}: {}", "Requirement".cyan(), req.title);
            println!();
            if req.comments.is_empty() {
                println!("{}", "No comments yet".dimmed());
            } else {
                println!("{}:", "Comments".green().bold());
                for c in &req.comments {
                    print_comment(c, 0);
                }
            }
        }
        // SPIKE-2: edit/delete an existing comment on a git-backed store.
        // Uses prefix-friendly resolution so users can pass the leading 8
        // chars of the UUID instead of the full 36-char form.
        // trace:SPIKE-2 | ai:claude
        Command::Comment(CommentCommand::Edit {
            req_id,
            comment_id,
            content,
            interactive,
        }) => {
            record_role_activity(req_id, "comment");
            let mut req = backend
                .get_requirement_by_spec_id(req_id)?
                .ok_or_else(|| not_found::requirement_not_found(req_id, Some(store_path)))?;
            let comment_uuid = resolve_comment_uuid(&req, comment_id)?;

            let new_content = if *interactive || content.is_none() {
                let existing = req
                    .find_comment_mut(&comment_uuid)
                    .map(|c| c.content.clone())
                    .unwrap_or_default();
                inquire::Editor::new("Edit comment")
                    .with_predefined_text(&existing)
                    .prompt()
                    .context("Editor cancelled")?
            } else {
                content.clone().unwrap()
            };

            let comment = req
                .find_comment_mut(&comment_uuid)
                .context("Comment not found (vanished between resolve and edit?)")?;
            comment.content = new_content;
            comment.touch();
            req.modified_at = chrono::Utc::now();
            backend.update_requirement(&req)?;
            println!("{} comment {} on {}", "Updated".green(), comment_uuid, req_id);
        }
        Command::Comment(CommentCommand::Delete { req_id, comment_id }) => {
            record_role_activity(req_id, "comment");
            let mut req = backend
                .get_requirement_by_spec_id(req_id)?
                .ok_or_else(|| not_found::requirement_not_found(req_id, Some(store_path)))?;
            let comment_uuid = resolve_comment_uuid(&req, comment_id)?;
            req.delete_comment(&comment_uuid)?;
            backend.update_requirement(&req)?;
            println!("{} comment {} from {}", "Deleted".green(), comment_uuid, req_id);
        }
        Command::Db(DbCommand::Path) => {
            // trace:FR-1-076 | ai:claude
            println!("{}", store_path.display());
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
            from_pos,
            to_pos,
            from_flag,
            to_flag,
            r#type,
            bidirectional,
            force_parent,
        }) => {
            let from = from_pos.as_deref().or(from_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing FROM (positional or --from)"))?;
            let to = to_pos.as_deref().or(to_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing TO (positional or --to)"))?;
            let mut from_req = backend
                .get_requirement_by_spec_id(from)?
                .ok_or_else(|| not_found::requirement_not_found(from, Some(store_path)))?;

            let to_req = backend
                .get_requirement_by_spec_id(to)?
                .ok_or_else(|| not_found::requirement_not_found(to, Some(store_path)))?;

            let rel_type = match r#type.to_lowercase().as_str() {
                "parent" => RelationshipType::Parent,
                "child" => RelationshipType::Child,
                "duplicate" => RelationshipType::Duplicate,
                "verifies" => RelationshipType::Verifies,
                "verified-by" | "verifiedby" => RelationshipType::VerifiedBy,
                "references" => RelationshipType::References,
                other => RelationshipType::Custom(other.to_string()),
            };

            // BUG-64: same terminal-status guard as `aida add --parent`,
            // applied here when the user is hand-rolling a parent edge
            // via `aida rel add X Y --type child` (X is child of Y, so Y
            // is the parent being checked) or its inverse `--type parent`
            // (X is parent of Y). `--force-parent` bypasses for backfill
            // cases. trace:BUG-64 | ai:claude
            if !*force_parent {
                let parent_for_guard = match &rel_type {
                    RelationshipType::Child => Some(&to_req),
                    RelationshipType::Parent => Some(&from_req),
                    _ => None,
                };
                if let Some(p) = parent_for_guard {
                    if is_terminal_status(&p.status) {
                        anyhow::bail!(
                            "parent {} is {} — adding new children to a closed parent is usually a mistake. \
                             Pass `--force-parent` to override.",
                            p.spec_id.as_deref().unwrap_or("?"),
                            p.status,
                        );
                    }
                }
            }

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
        Command::Rel(RelationshipCommand::Remove {
            from_pos,
            to_pos,
            from_flag,
            to_flag,
            ..
        }) => {
            let from = from_pos.as_deref().or(from_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing FROM (positional or --from)"))?;
            let to = to_pos.as_deref().or(to_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing TO (positional or --to)"))?;
            let mut from_req = backend
                .get_requirement_by_spec_id(from)?
                .ok_or_else(|| not_found::requirement_not_found(from, Some(store_path)))?;

            // Look up target UUID
            let to_req = backend
                .get_requirement_by_spec_id(to)?
                .ok_or_else(|| not_found::requirement_not_found(to, Some(store_path)))?;

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
        Command::Rel(RelationshipCommand::List { id }) => {
            // trace:TASK-1-020 | ai:claude
            record_role_activity(id, "show");
            let req = backend
                .get_requirement_by_spec_id(id)?
                .ok_or_else(|| not_found::requirement_not_found(id, Some(store_path)))?;

            println!("{}: {}", "Requirement".blue(), req.title);
            if let Some(spec_id) = &req.spec_id {
                println!("{}: {}", "SPEC-ID".blue(), spec_id);
            }
            println!("{}: {}", "UUID".blue(), req.id);
            println!();

            if req.relationships.is_empty() {
                println!("{}", "No relationships found.".yellow());
            } else {
                println!("{}:", "Relationships".green());
                for relationship in &req.relationships {
                    let target = backend.get_requirement(&relationship.target_id)?;
                    let description = match &relationship.rel_type {
                        RelationshipType::Parent => "is parent of".to_string(),
                        RelationshipType::Child => "is child of".to_string(),
                        RelationshipType::Duplicate => "is duplicate of".to_string(),
                        RelationshipType::Verifies => "verifies".to_string(),
                        RelationshipType::VerifiedBy => "is verified by".to_string(),
                        RelationshipType::References => "references".to_string(),
                        RelationshipType::Custom(name) => name.clone(),
                    };
                    if let Some(target_req) = target {
                        let target_spec = target_req.spec_id.as_deref().unwrap_or("N/A");
                        // BUG-53: tag rejected targets so a dangling-looking
                        // edge is recognizable as still-resolvable rather
                        // than removed. trace:BUG-53 | ai:claude
                        if matches!(target_req.status, aida_core::RequirementStatus::Rejected) {
                            println!(
                                "  {} {} {} - {}",
                                description.cyan(),
                                target_spec.yellow(),
                                "[REJECTED]".red().bold(),
                                target_req.title
                            );
                        } else {
                            println!(
                                "  {} {} - {}",
                                description.cyan(),
                                target_spec.yellow(),
                                target_req.title
                            );
                        }
                    } else {
                        // BUG-53: a relationship pointing at a uuid with no
                        // backing object means the target was deleted (its
                        // YAML is gone, mapping back to spec_id is lost). We
                        // show a short uuid + "(removed)" instead of the
                        // full 36-char uuid + "(not found)" so the line
                        // reads as a tombstone rather than a phantom.
                        // trace:BUG-53 | ai:claude
                        let uuid_str = relationship.target_id.to_string();
                        let short = &uuid_str[..uuid_str.len().min(8)];
                        println!(
                            "  {} {} {}",
                            description.cyan(),
                            short.dimmed(),
                            "(removed — run `aida doctor verify-relationships --repair` to clean up)".red()
                        );
                    }
                }
            }
        }

        // Phase 1: Sync command
        Command::Db(DbCommand::Sync { pull, push, message }) => {
            if !aida_core::git_ops::is_git_repo(store_path) {
                anyhow::bail!("Not a git repository: {}", store_path.display());
            }

            let branch = aida_core::git_ops::current_branch(store_path)
                .unwrap_or_else(|_| "main".to_string());

            // ── Order matters: commit local first, THEN pull --rebase ──
            // Rebase requires a clean working tree, so we have to commit
            // any pending edits (typically from `aida edit` paths that
            // didn't auto-commit, or manual file edits) before pulling.
            // Old order was pull → commit, which failed when there were
            // unstaged changes — git rebase refuses, and the follow-up
            // commit ran on a partial-rebase state with a confusing
            // empty error. trace:BUG-1-051 | ai:claude

            // Step 1: stage and commit any pending changes.
            //
            // Stage everything in the worktree (`git add -A .`) instead
            // of cherry-picking specific subdirs like `objects/`. The
            // orphan branch's own .gitignore already excludes runtime
            // artifacts (cache.db, lock files); whatever's left modified
            // is canonical state — including `.aida/dispenser.toml`,
            // which gets dirtied by ID dispensing and would otherwise be
            // skipped by an objects-only stage. Without this, has_changes
            // could report "yes" while nothing gets staged, and the
            // follow-up `git commit` would fail with an empty error.
            // trace:BUG-1-051 | ai:claude
            let has_changes = aida_core::git_ops::has_changes(store_path)?;
            if has_changes {
                let msg = message.as_deref().unwrap_or("chore: sync pending changes");
                aida_core::git_ops::add_all(store_path, ".")?;
                aida_core::git_ops::commit(store_path, msg)?;
                println!("Committed: {}", msg);
            } else if !*pull && !*push {
                println!("Nothing to commit.");
            }

            // Step 2: pull --rebase. Bare `git pull` fails on divergent
            // branches when the user has no `pull.rebase` / `pull.ff`
            // config — the orphan-store model wants linear history
            // anyway (replay local commits on top of remote), so
            // rebase is the right default.
            if *pull {
                // Snapshot local state before pull for conflict detection
                let local_reqs = backend.load()
                    .map(|s| s.requirements)
                    .unwrap_or_default();

                println!("Pulling from origin/{}...", branch);
                match aida_core::git_ops::pull_rebase(store_path, "origin", &branch) {
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
                    Err(e) => {
                        // A failed rebase leaves the repo in a partial
                        // state. Bail with a recovery hint so the user
                        // doesn't end up with weirder downstream errors.
                        anyhow::bail!(
                            "Pull failed: {}\n\
                             The orphan store may be mid-rebase. To recover:\n  \
                                 cd {} && git rebase --abort\n\
                             Then re-run `aida db sync --pull`.",
                            e,
                            store_path.display()
                        );
                    }
                }
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

        // trace:FR-2-005 | ai:claude
        Command::Db(DbCommand::Block { subcommand }) => {
            handle_block_command(subcommand, store_path)?;
        }

        // trace:FR-1-071 | ai:claude
        Command::Db(DbCommand::RetireLegacyIds { dry_run }) => {
            handle_retire_legacy_ids(&backend, store_path, *dry_run)?;
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
        Command::Review(review_cmd) => {
            // STORY-67: review-prompt generation reads requirements via the
            // same Storage façade as Queue above. Pure read path — no
            // mutation of the store.
            let storage = Storage::new(store_path);
            handle_review_command(review_cmd, &storage)?;
        }
        // STORY-44: `aida config user` is a global op against
        // ~/.aida/preferences.toml — no store needed. Route it through the
        // git-backend path so it works in modern projects too.
        Command::Config(ConfigCommand::User { node_id, email, toml: emit_toml }) => {
            handle_config_user(node_id.as_deref(), email.as_deref(), *emit_toml)?;
        }
        Command::Scaffold(scaffold_cmd) => {
            // Scaffold apply / status / preview / extract — same pattern.
            // Storage façade now handles directory paths via GitBackend.load().
            let storage = Storage::new(store_path);
            handle_scaffold_command(scaffold_cmd, &storage, store_path)?;
        }
        Command::History {
            limit,
            max_commits,
            events,
            id,
            r#type,
            author,
            since,
            until,
            status_changes,
            comments,
            oneline,
        } => {
            // trace:FR-1-037 | ai:claude
            // Default max_commits scales differently per mode: digest only
            // touches each commit once (cheap, scan deeper), events shells
            // to git per file per commit (expensive, scan shallow).
            let default_max = if *events { (*limit * 5).max(50) } else { 250 };
            let max = max_commits.unwrap_or(default_max);
            let opts = history::HistoryOpts {
                limit: *limit,
                max_commits: max.max(*limit),
                events_mode: *events,
                id_filter: id.clone(),
                type_filter: r#type.clone(),
                author_filter: author.clone(),
                since: since.clone(),
                until: until.clone(),
                status_changes_only: *status_changes,
                comments_only: *comments,
                oneline: *oneline,
            };
            history::run(store_path, &opts)?;
        }
        _ => {
            eprintln!(
                "Command not yet supported for git backend.\n\
                 Supported: list, add, show, edit, del, search, comment add/list,\n\
                 queue list/add/remove/move/clear,\n\
                 rel add/remove/list, db info/status/sync/merge-gate/export-git/workspace-init"
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

/// Validate a status string against the canonical set. Accepts case-
/// insensitive matches and common spelling variants (`in-progress`,
/// `inprogress`, `in_progress`). Returns Ok with the canonical form, or
/// Err with a list-of-valid-values message. Use at the CLI layer before
/// calling `Requirement::set_status_from_str` to prevent typos like
/// `approvedxxx` from silently landing as a `custom_status`. trace:BUG-47
pub fn validate_status_input(raw: &str) -> Result<&'static str, String> {
    let normalized: String = raw
        .chars()
        .filter_map(|c| match c {
            ' ' | '-' | '_' => None,
            c if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
            c => Some(c),
        })
        .collect();
    match normalized.as_str() {
        "draft" => Ok("Draft"),
        "approved" => Ok("Approved"),
        "planned" => Ok("Planned"),
        "inprogress" => Ok("InProgress"),
        "completed" | "done" => Ok("Completed"),
        "rejected" => Ok("Rejected"),
        _ => Err(format!(
            "invalid status `{}` — expected one of: draft, approved, planned, in-progress, completed, rejected",
            raw
        )),
    }
}

/// Same shape, for priority. trace:BUG-47
pub fn validate_priority_input(raw: &str) -> Result<&'static str, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "high" => Ok("High"),
        "medium" | "med" => Ok("Medium"),
        "low" => Ok("Low"),
        _ => Err(format!(
            "invalid priority `{}` — expected one of: high, medium, low",
            raw
        )),
    }
}

/// Detect signs that a `--title` was mangled by shell command-substitution
/// (backticks the user forgot to escape, an unmatched quote that lost the
/// rest of the string). Returns Some(message) if suspicious. Caller should
/// print as a warning — never reject — since false positives are possible.
/// trace:BUG-22 | ai:claude
fn suspicious_title_signal(title: &str) -> Option<String> {
    if title.contains('`') {
        return Some(format!(
            "title contains a backtick — if you meant a literal `, escape it (\\\\`) \
             or quote the whole title in single quotes; otherwise the shell may have \
             mangled it"
        ));
    }
    // An odd count of unescaped double-quotes is a strong signal of broken quoting.
    let dq_count = title.chars().filter(|c| *c == '"').count();
    if dq_count % 2 == 1 {
        return Some("title contains an unbalanced double-quote — likely a shell-quoting artifact".to_string());
    }
    None
}

/// Ensure git/claude hook files are executable. Called from scaffolder
/// write paths so freshly-scaffolded hooks don't trigger git's "hook was
/// ignored because not executable" warning.
/// trace:BUG-21 | ai:claude
fn ensure_executable_if_hook(rel_path: &std::path::Path, full_path: &std::path::Path) {
    let s = rel_path.to_string_lossy();
    let is_hook = s.starts_with(".git/hooks/")
        || s.starts_with(".claude/hooks/")
        || s.ends_with(".sh");
    if !is_hook {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(full_path) {
            let mut p = meta.permissions();
            p.set_mode(0o755);
            let _ = std::fs::set_permissions(full_path, p);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = full_path; // no-op on non-unix
    }
}

/// Resolve the description from one of three sources: inline `--description`,
/// `--description-from-file PATH`, or `--description-stdin`. The CLI struct
/// already enforces mutual exclusion via `conflicts_with_all`; here we just
/// fetch the content from the right source. Returns `Ok(None)` when no
/// source is set (caller falls back to empty / interactive prompt).
/// trace:BUG-17 | ai:claude
fn resolve_description(
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
        // Docs-layer types (FR-1-074). Aliases match the type prefix used
        // in agreed-id format (`PRIN`, `VIS`, `CON`, `ADR`, `TERM`).
        // trace:FR-1-074 | ai:claude
        "principle" | "prin" => Ok(RequirementType::Principle),
        "vision" | "vis" => Ok(RequirementType::Vision),
        "constraint" | "con" => Ok(RequirementType::Constraint),
        "decision" | "adr" => Ok(RequirementType::Decision),
        "term" | "glossary" => Ok(RequirementType::Term),
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
    verbose: bool,
    name: Option<&str>,
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

    // Post-clone detection (EPIC-1-052 Phase 4): if origin already has the
    // aida-store branch and the local worktree is missing, the user just
    // cloned an existing AIDA project. Bootstrap as a clone — fetch the
    // orphan, set up the worktree, run scaffolding, prompt for node id.
    // This wins over the "already initialized" check below because some
    // projects (notably AIDA itself) track .aida/config.toml in main.
    // trace:EPIC-1-052 Phase 4 | ai:claude
    let worktree_present = cwd.join(worktree_dir).exists();
    if !force
        && !worktree_present
        && git_ops::remote_branch_exists(&cwd, "origin", branch_name)
    {
        return handle_init_post_clone(
            &cwd, worktree_dir, branch_name, no_skills, agent, no_hooks, verbose,
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
    // Project name: explicit --name wins; otherwise default to the cwd
    // basename ("/home/joe/projects/tzconv" → "tzconv"). trace:BUG-25
    let project_name = name
        .map(|s| s.to_string())
        .or_else(|| {
            cwd.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();
    if !project_name.is_empty() {
        store.name = project_name.clone();
        store.title = project_name;
    }
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

    // Auto-push to origin/<branch_name> if origin exists, so subsequent
    // node-acquire / db-sync work without a manual `git push -u`. Skipped
    // silently when there's no origin (single-machine projects) or when
    // the push fails (offline, auth, etc.) — failure isn't fatal here.
    // trace:BUG-23 | ai:claude
    let mut origin_pushed = false;
    if git_ops::has_remote(&cwd, "origin") {
        let push_result = std::process::Command::new("git")
            .arg("-C")
            .arg(&store_path)
            .args(["push", "-u", "origin", branch_name])
            .output();
        match push_result {
            Ok(out) if out.status.success() => {
                origin_pushed = true;
                println!("  {} pushed orphan branch to origin/{}", "Done".green(), branch_name);
            }
            _ => {
                eprintln!(
                    "  {} could not push orphan branch to origin/{} \
                     (run `git -C {} push -u origin {}` later)",
                    "Note:".dimmed(),
                    branch_name,
                    worktree_dir,
                    branch_name
                );
            }
        }
    }

    // Auto-acquire a node id for this clone so nodes.toml has the entry
    // from day one (instead of relying on the implicit-node-1 fallback).
    // Runs whether or not origin exists: when there's no remote, the
    // registration commit lives on the local orphan branch and gets
    // uploaded by the next `aida push`. Without this the solo user can't
    // get their preferred node id (e.g. JM from prefs) without first
    // adding a remote — exactly the kind of friction kernel should hide.
    // trace:EPIC-9 Story 1, BUG-40 | ai:claude
    let has_origin = git_ops::has_remote(&cwd, "origin");
    {
        let hn = hostname();
        // Prefer email from `git config user.email`; fall back to
        // `~/.aida/preferences.toml`. trace:STORY-44 | ai:claude
        let prefs = aida_core::UserPreferences::load().unwrap_or_default();
        let email = git_ops::git_config_get("user.email")
            .ok()
            .or_else(|| prefs.email.clone());

        // STORY-44: try the preferred id first; on collision the kernel's
        // suggest_free_node_id helper picks the next free `<pref><N>` and
        // we auto-accept (init is non-interactive and the alternative is a
        // bad first impression). Without a preference, fall back to the
        // pre-EPIC-9 sequential numeric path.
        let requested_id: Option<String> = match prefs.preferred_node_id.as_deref() {
            Some(pref) => match git_ops::suggest_free_node_id(&store_path, pref) {
                Ok(git_ops::NodeIdProbe::Free) => Some(pref.to_string()),
                Ok(git_ops::NodeIdProbe::Taken { suggested }) => {
                    println!(
                        "  {} preferred node id '{}' is already taken — using '{}' instead",
                        "Note:".dimmed(),
                        pref,
                        suggested
                    );
                    Some(suggested)
                }
                Err(_) => None,
            },
            None => None,
        };

        match git_ops::register_node_full(&store_path, requested_id, 1, &hn, email.clone()) {
            Ok(new_id) => {
                let suffix = if has_origin { "" } else { " (local; will sync on next `aida push`)" };
                println!(
                    "  {} acquired node id {} (hostname={}, email={}){}",
                    "Done".green(),
                    new_id,
                    hn,
                    email.as_deref().unwrap_or("-"),
                    suffix
                );
                // FR-271: at init time, force the new-project default
                // (Global) explicitly. Reading config.toml here would
                // return PerType because we haven't written the config
                // yet (it's written further down in the init flow).
                if let Ok(blocks) =
                    auto_allocate_initial_blocks_with_scope(
                        &store_path,
                        &new_id,
                        &hn,
                        email.as_deref(),
                        aida_core::IdCounterScope::Global,
                    )
                {
                    if !blocks.is_empty() {
                        println!(
                            "  {} auto-allocated {} initial block{}",
                            "Done".green(),
                            blocks.len(),
                            if blocks.len() == 1 { "" } else { "s" }
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "  {} could not auto-acquire node id: {}. Run `aida node acquire` later.",
                    "Note:".dimmed(),
                    e
                );
            }
        }
    }

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
         branch = \"{}\"\n\
         \n\
         # trace:EPIC-1-052 Phase 2 | ai:claude\n\
         # How `aida add` chooses between agreed-id blocks and node-aware ids:\n\
         #   node-aware-only      — never use blocks; always FR-<NODE>-<SEQ>\n\
         #   blocks-then-fallback — try block first; fall through silently (default)\n\
         #   blocks-only          — error if no block is allocated for the type\n\
         #\n\
         # counter_scope (FR-271):\n\
         #   global               — single counter shared across all types (default for new projects)\n\
         #                          → FR-1, BUG-2, EPIC-3, ... ids globally unique by number\n\
         #   per-type             — separate counter per type prefix (legacy default)\n\
         #                          → FR-1, BUG-1, EPIC-1, ... each type starts fresh\n\
         [id_format]\n\
         policy = \"blocks-then-fallback\"\n\
         counter_scope = \"global\"\n",
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
        verbose,
    )?;

    println!();
    println!("  {}:", "Push code + store together".bold());
    println!("    {}                        push your branch and the orphan store in one go", "aida push".cyan());
    println!();
    println!("  {}:", "Onboard a teammate".bold());
    println!("    {}    they clone normally", "git clone <repo>".cyan());
    println!("    {}            then `aida init` notices the orphan branch and attaches", "aida init".cyan());
    println!();

    Ok(())
}

/// Bootstrap an AIDA clone: the user just `git clone`d a repo whose origin
/// already has the `aida-store` orphan branch, and they're running `aida init`
/// to set the project up locally. We fetch the orphan, attach a worktree,
/// run scaffolding, and prompt for node-id acquisition.
/// trace:EPIC-1-052 Phase 4 | ai:claude
fn handle_init_post_clone(
    cwd: &std::path::Path,
    worktree_dir: &str,
    branch_name: &str,
    no_skills: bool,
    agent: &str,
    no_hooks: bool,
    verbose: bool,
) -> Result<()> {
    use aida_core::git_ops;

    println!(
        "{} Detected existing AIDA store on {}/{} — bootstrapping clone...",
        "".cyan().bold(),
        "origin",
        branch_name
    );

    // Fetch + create local tracking branch for the orphan
    git_ops::fetch_branch_into_local(cwd, "origin", branch_name)?;
    println!(
        "  {} fetched origin/{} into local {}",
        "Done".green(),
        branch_name,
        branch_name
    );

    // Create the worktree pointing at the existing branch
    let store_path = git_ops::create_store_worktree(cwd, worktree_dir, branch_name)?;
    println!(
        "  {} worktree at {} → {}",
        "Done".green(),
        worktree_dir,
        branch_name
    );

    // Configure git user in the worktree (so future commits attribute correctly)
    let git_name = git_ops::git_config_get("user.name")
        .unwrap_or_else(|_| "AIDA User".to_string());
    let git_email = git_ops::git_config_get("user.email")
        .unwrap_or_else(|_| "aida@localhost".to_string());
    git_ops::configure_user(&store_path, &git_name, &git_email)?;

    // Add .aida-store/ to root .gitignore (idempotent)
    let gitignore_path = cwd.join(".gitignore");
    let gitignore_entry = format!("\n# AIDA distributed store (orphan branch worktree)\n{}/\n", worktree_dir);
    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        if !content.contains(worktree_dir) {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new().append(true).open(&gitignore_path)?;
            file.write_all(gitignore_entry.as_bytes())?;
            println!("  {} updated {}", "Done".green(), ".gitignore".white().bold());
        }
    } else {
        std::fs::write(&gitignore_path, gitignore_entry)?;
    }

    // Write .aida/config.toml — same contents as a fresh init
    let aida_dir = cwd.join(".aida");
    std::fs::create_dir_all(&aida_dir)?;
    let config_content = format!(
        "# AIDA distributed mode configuration\n\
         [deployment]\n\
         mode = \"distributed\"\n\
         store_path = \"{}\"\n\
         store_type = \"worktree\"\n\
         branch = \"{}\"\n\
         \n\
         # trace:EPIC-1-052 Phase 2 | ai:claude\n\
         # How `aida add` chooses between agreed-id blocks and node-aware ids:\n\
         #   node-aware-only      — never use blocks; always FR-<NODE>-<SEQ>\n\
         #   blocks-then-fallback — try block first; fall through silently (default)\n\
         #   blocks-only          — error if no block is allocated for the type\n\
         #\n\
         # counter_scope (FR-271):\n\
         #   global               — single counter shared across all types (default for new projects)\n\
         #                          → FR-1, BUG-2, EPIC-3, ... ids globally unique by number\n\
         #   per-type             — separate counter per type prefix (legacy default)\n\
         #                          → FR-1, BUG-1, EPIC-1, ... each type starts fresh\n\
         [id_format]\n\
         policy = \"blocks-then-fallback\"\n\
         counter_scope = \"global\"\n",
        worktree_dir, branch_name
    );
    std::fs::write(aida_dir.join("config.toml"), &config_content)?;
    println!(
        "  {} {}",
        "Done".green(),
        ".aida/config.toml".white().bold()
    );

    // docs/plans/ for plan archive
    std::fs::create_dir_all(cwd.join("docs/plans"))?;

    // Run scaffolding (CLAUDE.md, .claude/, hooks, etc.)
    // Load the store via GitBackend just for scaffolding metadata.
    let backend = aida_core::GitBackend::new(&store_path)?;
    let store = backend.load().unwrap_or_else(|_| aida_core::models::RequirementsStore::new());
    let storage_label = format!(
        "{}{}Git-canonical store ({}, orphan branch '{}')",
        worktree_dir.white().bold(),
        " ".repeat(20),
        worktree_dir,
        branch_name
    );
    complete_init_scaffolding(
        cwd,
        &store,
        agent,
        no_skills,
        no_hooks,
        false, // force
        std::path::PathBuf::from(worktree_dir),
        &storage_label,
        verbose,
    )?;

    // Prompt the user to acquire a node id. Auto-allocate happens inside
    // `aida node acquire` per Phase 3; we just wire up the same code path.
    println!();
    println!(
        "{}",
        "Node identity setup".cyan().bold()
    );
    println!(
        "  This clone needs a unique node id to issue requirement IDs without colliding"
    );
    println!(
        "  with other clones. Acquire one now? (Recommended.)"
    );

    let acquire_now = prompt_yes_no("Acquire a node id for this clone? [Y/n] ", true)?;
    if !acquire_now {
        println!();
        println!(
            "  {} You can run {} later — until then, this clone will share node id 1's namespace.",
            "Note:".yellow().bold(),
            "aida node acquire".cyan()
        );
        return Ok(());
    }

    let hn = hostname();
    // Prefer email from `git config user.email`; fall back to user prefs.
    // trace:STORY-44 | ai:claude
    let prefs = aida_core::UserPreferences::load().unwrap_or_default();
    let email = git_ops::git_config_get("user.email")
        .ok()
        .or_else(|| prefs.email.clone());
    let requested_id: Option<String> = match prefs.preferred_node_id.as_deref() {
        Some(pref) => match git_ops::suggest_free_node_id(&store_path, pref) {
            Ok(git_ops::NodeIdProbe::Free) => Some(pref.to_string()),
            Ok(git_ops::NodeIdProbe::Taken { suggested }) => {
                // Leading newline because this often fires immediately after
                // the user types `y` to the "Acquire? [Y/n] " prompt — without
                // it the Note butts up against the prompt's trailing space.
                println!(
                    "\n  {} preferred node id '{}' is taken — using '{}' instead",
                    "Note:".dimmed(),
                    pref,
                    suggested
                );
                Some(suggested)
            }
            Err(_) => None,
        },
        None => None,
    };
    let id_label = match &requested_id {
        Some(id) => format!("id={}", id),
        None => "next available id".to_string(),
    };
    println!();
    println!(
        "Acquiring node ({}; hostname={}, email={})...",
        id_label,
        hn,
        email.as_deref().unwrap_or("-")
    );
    let new_id = git_ops::register_node_full(
        &store_path,
        requested_id,
        1, // user_id placeholder — see Phase 1 commit message
        &hn,
        email.clone(),
    )?;
    println!(
        "  {} Acquired node id {} for this clone.",
        "".green().bold(),
        new_id
    );

    // Auto-allocate initial blocks for the common types (Phase 3).
    // trace:FR-1-073 | ai:claude
    match auto_allocate_initial_blocks(&store_path, &new_id, &hn, email.as_deref()) {
        Ok(blocks) if !blocks.is_empty() => {
            println!(
                "  Auto-allocated {} block{}: {}",
                blocks.len(),
                if blocks.len() == 1 { "" } else { "s" },
                blocks.join(", ")
            );
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!(
                "  {} Could not auto-allocate initial blocks: {}. \
                 Run `aida db block claim --type <T> --size 100` per type to retry.",
                "Warning:".yellow().bold(),
                e
            );
        }
    }

    println!();
    println!(
        "{} AIDA clone bootstrap complete.",
        "".green().bold()
    );
    Ok(())
}

/// Read y/n from stdin with a default. Treats empty input as the default,
/// any 'y'/'yes' as true, anything else as false.
fn prompt_yes_no(prompt: &str, default_yes: bool) -> Result<bool> {
    use std::io::Write;
    print!("{}", prompt);
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let trimmed = answer.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Ok(default_yes);
    }
    Ok(matches!(trimmed.as_str(), "y" | "yes"))
}

/// Initialize distributed mode using a sibling repo.
/// For multi-repo workspaces where multiple code repos share one store.
fn handle_init_distributed_sibling(
    registry_remote: Option<&str>,
    force: bool,
    no_skills: bool,
    agent: &str,
    no_hooks: bool,
    verbose: bool,
    name: Option<&str>,
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
    // Project name from --name or cwd basename. trace:BUG-25 | ai:claude
    let project_name = name
        .map(|s| s.to_string())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        })
        .unwrap_or_default();
    if !project_name.is_empty() {
        store.name = project_name.clone();
        store.title = project_name;
    }
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
    let config_content = "# AIDA distributed mode configuration\n\
         [deployment]\n\
         mode = \"distributed\"\n\
         store_path = \"aida-store\"\n\
         \n\
         # trace:EPIC-1-052 Phase 2 | ai:claude\n\
         # How `aida add` chooses between agreed-id blocks and node-aware ids:\n\
         #   node-aware-only      — never use blocks; always FR-<NODE>-<SEQ>\n\
         #   blocks-then-fallback — try block first; fall through silently (default)\n\
         #   blocks-only          — error if no block is allocated for the type\n\
         [id_format]\n\
         policy = \"blocks-then-fallback\"\n";
    std::fs::write(aida_dir.join("config.toml"), config_content)?;

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
        verbose,
    )?;

    println!();
    println!("  {}:", "Push code + store together".bold());
    println!("    {}                        push your branch and the orphan store in one go", "aida push".cyan());
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
        RequirementType::Principle => "Principle",
        RequirementType::Vision => "Vision",
        RequirementType::Constraint => "Constraint",
        RequirementType::Decision => "Decision",
        RequirementType::Term => "Term",
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


/// True when a requirement's status means "this work is done — no new
/// children should be filed under it without explicit override". Used by
/// the BUG-64 guard on `aida add --parent` and `aida rel add --type
/// child` to refuse parenting under closed work, and to keep `aida show
/// --tree` / `aida list --parent` views from accumulating mixed-status
/// trees. trace:BUG-64 | ai:claude
fn is_terminal_status(status: &RequirementStatus) -> bool {
    matches!(
        status,
        RequirementStatus::Completed | RequirementStatus::Rejected
    )
}

/// Parse requirement ID - accepts either UUID or SPEC-ID. Used by the legacy
/// SQLite path; the git-canonical dispatch resolves IDs directly via
/// `get_requirement_by_spec_id` and uses `not_found::requirement_not_found`
/// at the call site (with the actual store path).
///
/// trace:FR-1-011 | ai:claude
fn parse_requirement_id(id_str: &str, store: &RequirementsStore) -> Result<Uuid> {
    // Try parsing as UUID first
    if let Ok(uuid) = Uuid::parse_str(id_str) {
        return Ok(uuid);
    }

    // Try as SPEC-ID
    if let Some(req) = store.get_requirement_by_spec_id(id_str) {
        return Ok(req.id);
    }

    // The legacy helper doesn't have the storage path threaded through, so
    // we use the None-path variant which inspects cwd and reports "no aida
    // store found" / "cd into project root" — the right hint for the most
    // common failure mode (running aida from outside any AIDA project).
    Err(not_found::requirement_not_found(id_str, None))
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
        "folder" => Ok(RequirementType::Folder),
        "meta" => Ok(RequirementType::Meta),
        // Docs-layer types (FR-1-074). trace:FR-1-074 | ai:claude
        "principle" | "prin" => Ok(RequirementType::Principle),
        "vision" | "vis" => Ok(RequirementType::Vision),
        "constraint" | "con" => Ok(RequirementType::Constraint),
        "decision" | "adr" => Ok(RequirementType::Decision),
        "term" | "glossary" => Ok(RequirementType::Term),
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
        ConfigCommand::User { node_id, email, toml: emit_toml } => {
            // trace:STORY-44 | ai:claude
            handle_config_user(node_id.as_deref(), email.as_deref(), *emit_toml)?;
        }
    }

    Ok(())
}

/// Handle `aida config user` — show or update `~/.aida/preferences.toml`.
/// trace:STORY-44 | ai:claude
fn handle_config_user(
    node_id: Option<&str>,
    email: Option<&str>,
    emit_toml: bool,
) -> Result<()> {
    let mut prefs = aida_core::UserPreferences::load()?;
    let mut changed = false;

    if let Some(id) = node_id {
        if id.is_empty() {
            if prefs.preferred_node_id.is_some() {
                prefs.preferred_node_id = None;
                changed = true;
            }
        } else {
            aida_core::node::validate_node_id(id)
                .map_err(|m| anyhow::anyhow!("invalid node id: {}", m))?;
            if prefs.preferred_node_id.as_deref() != Some(id) {
                prefs.preferred_node_id = Some(id.to_string());
                changed = true;
            }
        }
    }

    if let Some(em) = email {
        if em.is_empty() {
            if prefs.email.is_some() {
                prefs.email = None;
                changed = true;
            }
        } else if prefs.email.as_deref() != Some(em) {
            prefs.email = Some(em.to_string());
            changed = true;
        }
    }

    if changed {
        let path = prefs.save()?;
        println!("{} Saved preferences to {}", "".green(), path.display());
    }

    if emit_toml {
        print!("{}", toml::to_string_pretty(&prefs).unwrap_or_default());
        return Ok(());
    }

    let path_display = aida_core::UserPreferences::path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unknown home dir>".to_string());
    println!("User preferences ({})", path_display);
    if prefs.is_empty() {
        println!("  (no preferences set — try `aida config user --node-id JM`)");
    } else {
        println!(
            "  preferred_node_id: {}",
            prefs.preferred_node_id.as_deref().unwrap_or("(unset)")
        );
        println!(
            "  email:             {}",
            prefs.email.as_deref().unwrap_or("(unset)")
        );
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
fn handle_retire_legacy_ids(
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
            "✓".green()
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
        "✓".green().bold(),
        renames.len(),
        if renames.len() == 1 { "" } else { "s" }
    );
    Ok(())
}

fn handle_block_command(cmd: &BlockCommand, store_path: &std::path::Path) -> Result<()> {
    use aida_core::BlockRegistry;

    let blocks_path = store_path.join("registry").join("blocks.yaml");
    let node_id = load_node_id(store_path);

    match cmd {
        BlockCommand::Claim { r#type, size } => {
            let type_prefix = r#type.to_uppercase();

            // Load or create registry, push-wins loop
            let max_retries = 3;
            for attempt in 0..max_retries {
                // Pull before modifying to reduce races
                let branch = aida_core::git_ops::current_branch(store_path)
                    .unwrap_or_else(|_| "aida-store".to_string());
                if attempt > 0 {
                    let _ = aida_core::git_ops::pull_rebase(store_path, "origin", &branch);
                }

                let mut registry = BlockRegistry::load(&blocks_path)?;
                // Honor the agreed-id counter floor so a fresh block can't
                // start at a number already issued in past merge-gate runs
                // or retire-legacy-ids migrations.
                // trace:FR-1-073 | ai:claude
                let counter_floor = read_agreed_counter(store_path, &type_prefix);
                let block = registry.claim_block_with_floor(
                    node_id.clone(),
                    std::env::var("USER").unwrap_or_else(|_| "unknown".into()),
                    hostname(),
                    type_prefix.clone(),
                    *size,
                    counter_floor,
                );
                registry.save(&blocks_path)?;

                // Commit the new block
                aida_core::git_ops::add(store_path, &["registry/blocks.yaml"])?;
                aida_core::git_ops::commit(
                    store_path,
                    &format!(
                        "chore: claim {}-{}..{} for node {}",
                        type_prefix, block.range_start, block.range_end, node_id
                    ),
                )?;

                // Push — retry on rejection
                match aida_core::git_ops::push(store_path, "origin", &branch) {
                    Ok(true) => {
                        println!(
                            "{} Claimed {}-{}..{} for node {} ({})",
                            "".green().bold(),
                            type_prefix,
                            block.range_start,
                            block.range_end,
                            node_id,
                            block.owner
                        );
                        return Ok(());
                    }
                    Ok(false) => {
                        // Push rejected — undo commit, pull, retry
                        let _ = std::process::Command::new("git")
                            .args(["reset", "--soft", "HEAD~1"])
                            .current_dir(store_path)
                            .output();
                        if attempt + 1 == max_retries {
                            anyhow::bail!(
                                "Could not push block claim after {} attempts. Run `aida db sync --pull` and retry.",
                                max_retries
                            );
                        }
                        eprintln!(
                            "{} Push rejected (concurrent claim), retrying ({}/{})...",
                            "!".yellow(), attempt + 1, max_retries
                        );
                    }
                    Err(e) => anyhow::bail!("Push failed: {}", e),
                }
            }
            anyhow::bail!("Failed to claim block after {} attempts", max_retries);
        }

        BlockCommand::List => {
            let registry = BlockRegistry::load(&blocks_path)?;
            if registry.blocks.is_empty() {
                println!("No blocks claimed yet. Run `aida db block claim` to allocate one.");
                return Ok(());
            }
            println!(
                "{:<8}  {:<6}  {:<12}  {:<12}  {:<10}  {}",
                "Node", "Type", "Range", "Next", "Remaining", "Owner"
            );
            println!("{}", "─".repeat(70));
            for b in &registry.blocks {
                let remaining = b.remaining();
                let remaining_str = if b.is_exhausted() {
                    "exhausted".red().to_string()
                } else if b.is_low() {
                    format!("{}", remaining).yellow().to_string()
                } else {
                    format!("{}", remaining)
                };
                println!(
                    "{:<8}  {:<6}  {}-{}..{}  {:<12}  {:<10}  {}",
                    b.node_id,
                    b.type_prefix,
                    b.type_prefix,
                    b.range_start,
                    b.range_end,
                    format!("{}-{}", b.type_prefix, b.next),
                    remaining_str,
                    b.owner
                );
            }
        }

        BlockCommand::Status => {
            let registry = BlockRegistry::load(&blocks_path)?;
            let my_blocks: Vec<_> = registry.blocks.iter().filter(|b| b.node_id == node_id).collect();
            if my_blocks.is_empty() {
                println!(
                    "No blocks for node {}. Run `aida db block claim` to allocate one.",
                    node_id
                );
                return Ok(());
            }
            println!("Blocks for node {}:", node_id);
            println!("{}", "─".repeat(50));
            for b in my_blocks {
                let remaining = b.remaining();
                if b.is_exhausted() {
                    println!(
                        "  {} {}: {} (exhausted — run `aida db block claim --type {}`)",
                        "".red(),
                        b.type_prefix,
                        format!("{}-{}..{}", b.type_prefix, b.range_start, b.range_end),
                        b.type_prefix
                    );
                } else if b.is_low() {
                    println!(
                        "  {} {}: {} remaining ({}-{}..{}) — {} Low, claim soon",
                        "".yellow(),
                        b.type_prefix,
                        remaining,
                        b.type_prefix,
                        b.range_start,
                        b.range_end,
                        "WARNING:".yellow().bold()
                    );
                } else {
                    println!(
                        "  {} {}: {} remaining ({}-{}..{})",
                        "".green(),
                        b.type_prefix,
                        remaining,
                        b.type_prefix,
                        b.range_start,
                        b.range_end
                    );
                }
            }
        }
        // FR-281: cross-check nodes.toml against blocks.yaml.
        // trace:FR-281 | ai:claude
        BlockCommand::Verify => {
            use aida_core::NodeRegistry;
            let nodes_path = store_path.join("registry").join("nodes.toml");
            let blocks_registry = BlockRegistry::load(&blocks_path).unwrap_or_default();
            let nodes_registry = NodeRegistry::load(&nodes_path).unwrap_or_default();

            // Only active (non-exhausted) blocks count as "owning" a range.
            // Tombstoned blocks (post `aida doctor repair-stale-blocks`)
            // intentionally have an unregistered owner — that's the repair
            // outcome, not a problem to flag.
            let block_owners: std::collections::HashSet<String> = blocks_registry
                .blocks
                .iter()
                .filter(|b| !b.is_exhausted())
                .map(|b| b.node_id.clone())
                .collect();
            let registered: std::collections::HashSet<String> = nodes_registry
                .nodes
                .iter()
                .map(|n| n.id.clone())
                .collect();

            let blocks_without_node: Vec<&str> = block_owners
                .iter()
                .filter(|id| !registered.contains(*id))
                .map(|s| s.as_str())
                .collect();
            let nodes_without_block: Vec<&str> = registered
                .iter()
                .filter(|id| !block_owners.contains(*id))
                .map(|s| s.as_str())
                .collect();

            // Whether to flag nodes_without_block as a problem depends on
            // policy. Under blocks-only it's a hard error; under blocks-
            // then-fallback it's just informational.
            let project_dir = std::env::current_dir().unwrap_or_default();
            let policy = read_id_format_policy(&project_dir);
            let nodes_without_block_is_error = policy.requires_block();

            let mut had_error = false;
            println!("{}", "Block registry consistency check".bold());
            println!("  policy: {}", policy.as_str());
            println!("  registered nodes: {}", registered.len());
            println!("  block-owning nodes: {}", block_owners.len());
            println!();

            if blocks_without_node.is_empty() && nodes_without_block.is_empty() {
                println!("{} consistent — every block has a registered node, every node has a block.", "✓".green().bold());
                return Ok(());
            }

            if !blocks_without_node.is_empty() {
                had_error = true;
                println!(
                    "{} {} block-owning node{} not in registry/nodes.toml:",
                    "✗".red().bold(),
                    blocks_without_node.len(),
                    if blocks_without_node.len() == 1 { "" } else { "s" }
                );
                let mut sorted = blocks_without_node.clone();
                sorted.sort();
                for id in &sorted {
                    let blocks_for_id: Vec<String> = blocks_registry
                        .blocks
                        .iter()
                        .filter(|b| &b.node_id.as_str() == id)
                        .map(|b| {
                            format!(
                                "{}-{}..{}",
                                b.type_prefix, b.range_start, b.range_end
                            )
                        })
                        .collect();
                    println!(
                        "    - node `{}` owns {} block(s): {}",
                        id,
                        blocks_for_id.len(),
                        blocks_for_id.join(", ")
                    );
                }
                println!(
                    "  {} run `aida node acquire --id {}` from that clone, OR \
                     `aida node release {}` to free the orphaned blocks.",
                    "Fix:".yellow().bold(),
                    sorted[0],
                    sorted[0]
                );
                println!();
            }

            if !nodes_without_block.is_empty() {
                if nodes_without_block_is_error {
                    had_error = true;
                    println!(
                        "{} {} registered node{} with no claimed block (blocks-only policy):",
                        "✗".red().bold(),
                        nodes_without_block.len(),
                        if nodes_without_block.len() == 1 { "" } else { "s" }
                    );
                } else {
                    println!(
                        "{} {} registered node{} with no claimed block (allowed under `{}`):",
                        "·".dimmed(),
                        nodes_without_block.len(),
                        if nodes_without_block.len() == 1 { "" } else { "s" },
                        policy.as_str()
                    );
                }
                let mut sorted = nodes_without_block.clone();
                sorted.sort();
                for id in &sorted {
                    println!("    - node `{}`", id);
                }
                if nodes_without_block_is_error {
                    println!(
                        "  {} run `aida db block claim --type FR --size 100` from each \
                         affected clone (per type) to allocate.",
                        "Fix:".yellow().bold()
                    );
                }
                println!();
            }

            if had_error {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

/// Handle `aida node` subcommands. Operates on the orphan-store worktree
/// at `store_path` (typically `.aida-store/`).
/// trace:EPIC-1-052 | ai:claude
/// Render the docs tree, called from the legacy (Storage facade) dispatch.
/// trace:FR-1-077 | ai:claude
fn handle_docs_command(cmd: &DocsCommand, storage: &Storage) -> Result<()> {
    let store = storage.load()?;
    handle_docs_with_store(cmd, &store)
}

/// Shared implementation — both the legacy Storage path and the git-canonical
/// path call this with a loaded store.
/// trace:FR-1-077 | ai:claude
fn handle_docs_with_store(cmd: &DocsCommand, store: &RequirementsStore) -> Result<()> {
    match cmd {
        DocsCommand::Build { output, dry_run } => {
            let out = output
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("docs/aida"));
            let report = docs::build(store, &out, *dry_run)?;

            let total_planned =
                report.written.len() + report.unchanged.len() + report.drifted.len();
            println!(
                "{} {} layer file{} ({} written, {} updated, {} unchanged)",
                if *dry_run { "→ dry-run:".cyan() } else { "✓".green() },
                total_planned,
                if total_planned == 1 { "" } else { "s" },
                report.written.len(),
                report.drifted.len(),
                report.unchanged.len(),
            );
            for p in &report.written {
                println!("    + {}", p.display().to_string().green());
            }
            for p in &report.drifted {
                println!("    ↑ {}", p.display().to_string().yellow());
            }
            for p in &report.deleted {
                println!(
                    "    {} stale (decision deleted from graph): {}",
                    "?".yellow(),
                    p.display()
                );
            }
            // BUG-41: signpost the README so the user has an entry point
            // into the projected layers without browsing the directory.
            // trace:BUG-41 | ai:claude
            if !*dry_run {
                let readme = out.join("README.md");
                if readme.exists() {
                    println!(
                        "\n  → Open {} to navigate the layers.",
                        readme.display().to_string().cyan()
                    );
                }
            }
        }
        DocsCommand::Check { output } => {
            let out = output
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("docs/aida"));
            let report = docs::build(store, &out, /* dry_run */ true)?;
            if report.has_drift() || !report.written.is_empty() {
                eprintln!(
                    "{} docs tree differs from graph projection:",
                    "drift:".yellow().bold()
                );
                for p in &report.written {
                    eprintln!("    missing: {}", p.display());
                }
                for p in &report.drifted {
                    eprintln!("    drifted: {}", p.display());
                }
                eprintln!("\n    Run `aida docs build` to regenerate.");
                std::process::exit(1);
            }
            println!("{} docs tree matches graph projection.", "✓".green());
        }
    }
    Ok(())
}

fn handle_node_command(cmd: &NodeCommand, store_path: &std::path::Path) -> Result<()> {
    use aida_core::node::NodeRegistry;

    let registry_path = store_path.join("registry").join("nodes.toml");
    let node_config_path = store_path.join(".aida").join("node.toml");

    match cmd {
        NodeCommand::List => {
            let registry = NodeRegistry::load(&registry_path).unwrap_or_default();
            let current_id = if node_config_path.exists() {
                aida_core::NodeConfig::load(&node_config_path).ok().map(|c| c.node_id)
            } else {
                None
            };

            if registry.nodes.is_empty() {
                println!("No nodes registered yet. Run `aida node acquire` to claim id 1.");
                return Ok(());
            }

            println!(
                "{:<2}  {:<6}  {:<8}  {:<28}  {:<22}  {}",
                "", "Node", "User", "Email", "Hostname", "Registered"
            );
            println!("{}", "─".repeat(100));
            for n in &registry.nodes {
                let marker = if current_id.as_deref() == Some(n.id.as_str()) { "*" } else { " " };
                let email = n.email.clone().unwrap_or_else(|| "-".into());
                let when = n.registered.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string();
                println!(
                    "{:<2}  {:<6}  {:<8}  {:<28}  {:<22}  {}",
                    marker, n.id, n.user_id, truncate(&email, 28), truncate(&n.hostname, 22), when
                );
            }
        }

        NodeCommand::Show { id } => {
            let registry = NodeRegistry::load(&registry_path).unwrap_or_default();
            let target_id: String = match id {
                Some(i) => i.clone(),
                None => {
                    if !node_config_path.exists() {
                        anyhow::bail!(
                            "No current node — `.aida/node.toml` does not exist. \
                             Run `aida node acquire` to claim a node id, or pass an id explicitly."
                        );
                    }
                    aida_core::NodeConfig::load(&node_config_path)?.node_id
                }
            };

            let entry = registry.get(&target_id).ok_or_else(|| {
                anyhow::anyhow!("Node {} is not in the shared registry", target_id)
            })?;

            println!("Node {}", entry.id);
            println!("  User ID:    {}", entry.user_id);
            println!("  Hostname:   {}", entry.hostname);
            println!("  Email:      {}", entry.email.as_deref().unwrap_or("-"));
            println!("  Registered: {}", entry.registered.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S %Z"));
            if node_config_path.exists() {
                let local = aida_core::NodeConfig::load(&node_config_path)?;
                if local.node_id == entry.id {
                    println!("  Active on this clone: yes (.aida/node.toml matches)");
                }
            }
        }

        NodeCommand::Acquire { id: requested_id, hostname: hn_override, email: email_override, force, yes, hijack } => {
            // STORY-43 hijack path: re-claim an existing node id.
            if let Some(target_id) = hijack {
                let hn = hn_override.clone().unwrap_or_else(hostname);
                let email = email_override.clone().or_else(|| {
                    aida_core::git_ops::git_config_get("user.email").ok()
                });
                let user_id = 1;
                println!(
                    "Hijacking node id '{}' for this clone (hostname={}, email={})...",
                    target_id, hn, email.as_deref().unwrap_or("-")
                );
                let outcome = aida_core::git_ops::hijack_node(
                    store_path, target_id, user_id, &hn, email.clone(),
                )?;
                match &outcome {
                    aida_core::git_ops::HijackOutcome::MarkedInPlace { marker_path } => {
                        println!(
                            "{} Hijacked node id '{}'. Marker dropped at {}",
                            "".green().bold(),
                            target_id,
                            marker_path.display()
                        );
                        println!(
                            "  Next `aida` invocation in the old clone will warn the user."
                        );
                    }
                    aida_core::git_ops::HijackOutcome::Reattributed { reason } => {
                        println!(
                            "{} Hijacked node id '{}' (re-attributed: {}).",
                            "".green().bold(),
                            target_id,
                            reason
                        );
                    }
                }
                return Ok(());
            }

            if node_config_path.exists() && !*force {
                let existing = aida_core::NodeConfig::load(&node_config_path)?;
                anyhow::bail!(
                    "This clone already has node id {} (registered {}). \
                     Pass --force to re-acquire (will allocate a new id).",
                    existing.node_id,
                    existing.registered_at.with_timezone(&chrono::Local).format("%Y-%m-%d")
                );
            }

            let hn = hn_override.clone().unwrap_or_else(hostname);
            let email = email_override.clone().or_else(|| {
                aida_core::git_ops::git_config_get("user.email").ok()
            });

            // user_id resolution: for now we use a placeholder of 1 if we
            // can't find a UserRegistry entry. Phase 1 deliberately doesn't
            // touch user identity — that's out of scope. The email stamp
            // in the node entry is the actual identity carrier.
            let user_id = 1;

            // STORY-42 collision UX: when the user requested a specific id
            // (e.g., `--id JM`), probe the registry first. If it's taken,
            // suggest `JM2` and prompt — or auto-accept under `--yes`.
            // trace:STORY-42 | ai:claude
            let effective_id: Option<String> = match requested_id {
                Some(req) => {
                    use aida_core::git_ops::{suggest_free_node_id, NodeIdProbe};
                    match suggest_free_node_id(store_path, req)? {
                        NodeIdProbe::Free => Some(req.clone()),
                        NodeIdProbe::Taken { suggested } => {
                            if *yes {
                                println!(
                                    "Node id '{}' is taken — auto-accepting suggested '{}' (--yes).",
                                    req, suggested
                                );
                                Some(suggested)
                            } else if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                                anyhow::bail!(
                                    "Node id '{}' is taken. Try `aida node acquire --id {}` \
                                     (or pass --yes to auto-accept the suggestion).",
                                    req, suggested
                                );
                            } else {
                                use std::io::Write;
                                println!("Node id '{}' is already taken in this store.", req);
                                print!("Use suggested '{}' instead? [Y/n/<other-id>] ", suggested);
                                std::io::stdout().flush()?;
                                let mut answer = String::new();
                                std::io::stdin().read_line(&mut answer)?;
                                let answer = answer.trim();
                                let chosen = if answer.is_empty()
                                    || answer.eq_ignore_ascii_case("y")
                                    || answer.eq_ignore_ascii_case("yes")
                                {
                                    suggested
                                } else if answer.eq_ignore_ascii_case("n")
                                    || answer.eq_ignore_ascii_case("no")
                                {
                                    println!("Aborted.");
                                    return Ok(());
                                } else {
                                    answer.to_string()
                                };
                                if let Err(msg) = aida_core::node::validate_node_id(&chosen) {
                                    anyhow::bail!("Invalid node id: {}", msg);
                                }
                                Some(chosen)
                            }
                        }
                    }
                }
                None => None,
            };

            let id_label = match &effective_id {
                Some(n) => format!("id={}", n),
                None => "next available id".to_string(),
            };
            println!(
                "Acquiring node ({}; hostname={}, email={}, user_id={})...",
                id_label,
                hn,
                email.as_deref().unwrap_or("-"),
                user_id
            );

            let new_id = aida_core::git_ops::register_node_full(
                store_path,
                effective_id,
                user_id,
                &hn,
                email.clone(),
            )?;

            println!(
                "{} Acquired node id {} for this clone. Per-clone identity at {}",
                "".green().bold(),
                new_id,
                node_config_path.display()
            );

            // Phase 3: auto-allocate initial blocks (FR + BUG + TASK +
            // EPIC + STORY + SPIKE) when the project's id_format policy
            // uses blocks. Skipped for `node-aware-only` since that
            // policy never dispenses from blocks.
            // trace:EPIC-1-052 Phase 3 | ai:claude
            // trace:FR-1-073 | ai:claude
            let project_dir = std::env::current_dir().unwrap_or_default();
            let id_policy = read_id_format_policy(&project_dir);
            if id_policy.uses_blocks() {
                match auto_allocate_initial_blocks(store_path, &new_id, &hn, email.as_deref()) {
                    Ok(blocks) if !blocks.is_empty() => {
                        println!(
                            "  Auto-allocated {} block{}: {}",
                            blocks.len(),
                            if blocks.len() == 1 { "" } else { "s" },
                            blocks.join(", ")
                        );
                    }
                    Ok(_) => {
                        // every type already had a block — nothing to do
                    }
                    Err(e) => {
                        eprintln!(
                            "{} Could not auto-allocate initial blocks: {}. \
                             Run `aida db block claim --type <T> --size 100` per type to retry.",
                            "Warning:".yellow().bold(),
                            e
                        );
                    }
                }
            } else {
                println!(
                    "  id_format policy is `{}` — skipping block allocation.",
                    id_policy.as_str()
                );
            }
        }

        NodeCommand::Release { id, yes } => {
            let registry = NodeRegistry::load(&registry_path).unwrap_or_default();
            let entry = registry.get(id).ok_or_else(|| {
                anyhow::anyhow!("Node {} is not in the shared registry", id)
            })?;

            println!("About to release node {}:", entry.id);
            println!("  Hostname: {}", entry.hostname);
            println!("  Email:    {}", entry.email.as_deref().unwrap_or("-"));
            println!(
                "  Note: any IDs already issued by this node remain valid. \
                 The node id is NOT recycled."
            );

            if !*yes {
                use std::io::Write;
                print!("Continue? [y/N] ");
                std::io::stdout().flush()?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    println!("Aborted.");
                    return Ok(());
                }
            }

            let removed = aida_core::git_ops::unregister_node(store_path, id)?;
            if removed {
                println!("{} Node {} removed from registry.", "".green(), id);
            } else {
                println!("Node {} was not in the registry (already removed?).", id);
            }
        }
    }

    Ok(())
}

/// Common requirement types that get auto-allocated blocks on `aida node
/// acquire` (Phase 3 of EPIC-1-052). Without these, new reqs of these
/// types fall through to the node-aware form (`TASK-1-019`) and require
/// `aida db merge-gate` to promote them — friction the user shouldn't
/// have to think about. Includes the five docs-layer types from FR-1-074
/// so new clones get short ADR-1, PRIN-1, VIS-1, etc., out of the box.
/// trace:FR-1-073 | ai:claude
/// trace:FR-1-074 | ai:claude
const PHASE3_AUTO_ALLOC_TYPES: &[&str] = &[
    "FR", "BUG", "TASK", "EPIC", "STORY", "SPIKE",
    "PRIN", "VIS", "CON", "ADR", "TERM",
];

/// Auto-allocate initial blocks for a freshly-acquired node. Claims one
/// block per common type that doesn't already have one for this node.
/// Returns a vector of allocated range labels (e.g., `["FR-101..200",
/// "BUG-1..100"]`), or an empty vec if every type already had a block
/// (idempotent — safe to re-run).
///
/// Default block size is 100. Each block claim goes through its own CAS
/// push loop so a stray contention on one type doesn't block the others.
/// trace:EPIC-1-052 Phase 3 | ai:claude
/// trace:FR-1-073 | ai:claude
fn auto_allocate_initial_blocks(
    store_path: &std::path::Path,
    node_id: &str,
    hn: &str,
    email: Option<&str>,
) -> Result<Vec<String>> {
    // Read counter_scope from config.toml. When called from `aida init`,
    // the config might not exist yet (init writes it later in the flow);
    // the explicit-scope variant below is the right call there.
    let project_dir = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let scope = read_id_counter_scope(&project_dir);
    auto_allocate_initial_blocks_with_scope(store_path, node_id, hn, email, scope)
}

/// Same as `auto_allocate_initial_blocks` but with an explicit scope —
/// for use by `aida init` which decides scope before writing config.toml.
/// trace:FR-271 | ai:claude
fn auto_allocate_initial_blocks_with_scope(
    store_path: &std::path::Path,
    node_id: &str,
    hn: &str,
    email: Option<&str>,
    scope: aida_core::IdCounterScope,
) -> Result<Vec<String>> {
    let mut allocated = Vec::new();
    if scope == aida_core::IdCounterScope::Global {
        // One shared block per node, size 1000. Dispense formats with
        // the caller-requested type prefix at dispense time.
        if let Some(label) = auto_allocate_block_with_size(
            store_path,
            node_id,
            hn,
            email,
            aida_core::IdCounterScope::GLOBAL_TYPE_PREFIX,
            1000,
        )? {
            allocated.push(label);
        }
        return Ok(allocated);
    }

    for type_prefix in PHASE3_AUTO_ALLOC_TYPES {
        if let Some(label) =
            auto_allocate_block_for_type(store_path, node_id, hn, email, type_prefix)?
        {
            allocated.push(label);
        }
    }
    Ok(allocated)
}

/// Like `auto_allocate_block_for_type` but with an explicit size — used
/// by the Global counter-scope path which wants a larger shared block.
/// trace:FR-271 | ai:claude
fn auto_allocate_block_with_size(
    store_path: &std::path::Path,
    node_id: &str,
    hn: &str,
    email: Option<&str>,
    type_prefix: &str,
    size: u32,
) -> Result<Option<String>> {
    auto_allocate_block_inner(store_path, node_id, hn, email, type_prefix, size)
}

/// Allocate a single block for the given (node_id, type_prefix) if one
/// doesn't already exist. Returns Some("<TYPE>-<start>..<end>") on a fresh
/// claim, None if the node already had a block for that type.
/// trace:FR-1-073 | ai:claude
fn auto_allocate_block_for_type(
    store_path: &std::path::Path,
    node_id: &str,
    hn: &str,
    email: Option<&str>,
    type_prefix: &str,
) -> Result<Option<String>> {
    auto_allocate_block_inner(store_path, node_id, hn, email, type_prefix, 100)
}

/// Shared CAS-loop allocator. Size differs by caller: per-type defaults
/// to 100; global scope uses 1000. The label format `<TYPE>-<start>..<end>`
/// is preserved verbatim so existing user-facing output looks unchanged
/// under per-type, and the global label reads as `*-1..1000` (a clear
/// visual signal that this is the shared block).
/// trace:FR-271 | ai:claude
fn auto_allocate_block_inner(
    store_path: &std::path::Path,
    node_id: &str,
    hn: &str,
    email: Option<&str>,
    type_prefix: &str,
    size: u32,
) -> Result<Option<String>> {
    use aida_core::BlockRegistry;

    let blocks_path = store_path.join("registry").join("blocks.yaml");
    let owner = email
        .map(|e| e.to_string())
        .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "unknown".into()));

    // BUG-40: same local-only short-circuit as register_node_full —
    // claim the block on the local orphan branch and let the next
    // `aida push` upload it.
    let local_only = !aida_core::git_ops::has_remote(store_path, "origin");

    let max_retries = 3;
    for attempt in 0..max_retries {
        if attempt > 0 && !local_only {
            let branch = aida_core::git_ops::current_branch(store_path)
                .unwrap_or_else(|_| "aida-store".to_string());
            let _ = aida_core::git_ops::pull_rebase(store_path, "origin", &branch);
        }

        let mut registry = BlockRegistry::load(&blocks_path)?;
        if registry.find_active_block(node_id, type_prefix).is_some() {
            return Ok(None);
        }

        let counter_floor = read_agreed_counter(store_path, type_prefix);
        let block = registry.claim_block_with_floor(
            node_id.to_string(),
            owner.clone(),
            hn.to_string(),
            type_prefix.to_string(),
            size,
            counter_floor,
        );
        registry.save(&blocks_path)?;

        aida_core::git_ops::add(store_path, &["registry/blocks.yaml"])?;
        aida_core::git_ops::commit(
            store_path,
            &format!(
                "chore(registry): auto-allocate {}-{}..{} for node {} on acquire",
                type_prefix, block.range_start, block.range_end, node_id
            ),
        )?;

        if local_only {
            return Ok(Some(format!(
                "{}-{}..{}",
                type_prefix, block.range_start, block.range_end
            )));
        }

        let branch = aida_core::git_ops::current_branch(store_path)
            .unwrap_or_else(|_| "aida-store".to_string());
        match aida_core::git_ops::push(store_path, "origin", &branch) {
            Ok(true) => {
                return Ok(Some(format!(
                    "{}-{}..{}",
                    type_prefix, block.range_start, block.range_end
                )));
            }
            Ok(false) => {
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(store_path)
                    .output();
                continue;
            }
            Err(e) => anyhow::bail!("Push failed: {}", e),
        }
    }
    anyhow::bail!(
        "could not push block claim for {} after {} attempts",
        type_prefix,
        max_retries
    );
}

/// Truncate a string for table display, with an ellipsis when shortened.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let clipped: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", clipped)
    }
}

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

    /// Phase 3 scope filter: tags AND'd into the default filter for
    /// `aida list` and `aida queue list/next` while this role is active.
    /// Empty = no tag scope. Override on a single command with explicit
    /// --tags or --no-scope.
    /// trace:TASK-1-021 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    scope_tags: Vec<String>,

    /// Phase 3 scope filter: status auto-applied while this role is active.
    /// None = no status scope. Override on a single command with explicit
    /// --status or --no-scope.
    /// trace:TASK-1-021 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope_status: Option<String>,

    /// Phase 3 system-prompt addendum: free-form text injected into Claude
    /// Code's context at SessionStart (via the aida-role-context.sh hook)
    /// when this role is active. Lets you keep role-specific instructions
    /// to the model alongside the role itself.
    /// trace:TASK-1-022 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    system_prompt: Option<String>,
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

    // STORY-56: when this shell is inside a session lease's worktree,
    // route the per-spec activity to the session-local log so concurrent
    // sessions don't clobber each other's @SPEC. The project-level role
    // file still gets `last_active_at` bumped so `aida role list --recent`
    // keeps working as a "what role was used most recently" view, but its
    // `activity` stream stays at whatever the last non-session shell saw
    // until session-end flattens the session log into it.
    // trace:STORY-56 | ai:claude
    let lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(&project, &cwd));

    if let Some(lease) = lease {
        let _ = append_session_activity(&project, &lease.id, &role_name, spec_id, action);
        if let Ok((mut state, path)) = load_role(&project, &role_name) {
            state.last_active_at = chrono::Utc::now();
            let _ = save_role_at(&state, &path);
        }
        return;
    }

    let (mut state, path) = match load_role(&project, &role_name) {
        Ok(t) => t,
        Err(_) => return,
    };
    // BUG-65: LRU-by-(spec_id, action). Drop any prior entry with the
    // same key, then insert at the front — interleaved sequences like
    // [show A, edit B, show A] no longer leave a stale duplicate behind.
    // trace:BUG-65 | ai:claude
    let entry = RoleActivity {
        spec_id: spec_id.to_string(),
        action: action.to_string(),
        at: chrono::Utc::now(),
    };
    state
        .activity
        .retain(|prev| !(prev.spec_id == entry.spec_id && prev.action == entry.action));
    state.activity.insert(0, entry);
    state.activity.truncate(ACTIVITY_MAX);
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
        RoleCommand::Active => handle_role_active(),
        RoleCommand::End => handle_role_end(),
        RoleCommand::Delete { name, yes } => handle_role_delete(&project_root, name, *yes),
        RoleCommand::Scaffold => handle_role_scaffold(),
        RoleCommand::Scope(scope_cmd) => handle_role_scope(&project_root, scope_cmd),
        RoleCommand::Prompt(prompt_cmd) => handle_role_prompt(&project_root, prompt_cmd),
    }
}

/// trace:TASK-1-022 | ai:claude
fn handle_role_prompt(project_root: &std::path::Path, cmd: &RolePromptCommand) -> Result<()> {
    match cmd {
        RolePromptCommand::Set {
            name,
            content,
            content_flag,
            stdin,
        } => {
            let role_name = resolve_role_name(name.as_deref())?;
            let (mut state, path) = load_role(project_root, &role_name)?;

            let text = if *stdin {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf.trim_end().to_string()
            } else if let Some(c) = content_flag.as_deref().or(content.as_deref()) {
                c.to_string()
            } else {
                anyhow::bail!(
                    "No content given. Pass it as a positional argument, via --content, \
                     or pipe with --stdin."
                );
            };

            if text.trim().is_empty() {
                anyhow::bail!(
                    "Empty addendum. Use `aida role prompt clear` to remove an existing one."
                );
            }
            state.system_prompt = Some(text);
            save_role_at(&state, &path)?;
            print_role_prompt(&state);
        }
        RolePromptCommand::Show { name } => {
            let role_name = resolve_role_name(name.as_deref())?;
            let (state, _) = load_role(project_root, &role_name)?;
            print_role_prompt(&state);
        }
        RolePromptCommand::Clear { name } => {
            let role_name = resolve_role_name(name.as_deref())?;
            let (mut state, path) = load_role(project_root, &role_name)?;
            state.system_prompt = None;
            save_role_at(&state, &path)?;
            println!("Role: {}", state.name.cyan());
            println!("  {}", "Addendum cleared.".dimmed());
        }
    }
    Ok(())
}

fn print_role_prompt(state: &RoleState) {
    println!("Role: {}", state.name.cyan());
    match &state.system_prompt {
        None => println!("  {}", "No system-prompt addendum set.".dimmed()),
        Some(text) => {
            println!("  {} ({} chars):", "Addendum".bold(), text.len());
            for line in text.lines() {
                println!("    {}", line);
            }
        }
    }
}

/// Resolve a role name from --name or AIDA_SESSION_ROLE; error if neither.
/// trace:TASK-1-021 | ai:claude
fn resolve_role_name(name: Option<&str>) -> Result<String> {
    if let Some(n) = name {
        return Ok(n.to_string());
    }
    match std::env::var("AIDA_SESSION_ROLE") {
        Ok(n) if !n.is_empty() => Ok(n),
        _ => anyhow::bail!(
            "No role active and no --name given. Either `aida role enter <name>` first \
             or pass --name to target a specific role."
        ),
    }
}

/// Read the active role's scope (tags, status), if any. Returns None when
/// no role is active or the role file is unreadable. Used by `aida list`
/// and `aida queue list/next` to compose default filters.
/// trace:TASK-1-021 | ai:claude
fn active_role_scope() -> Option<(Vec<String>, Option<String>)> {
    let role_name = std::env::var("AIDA_SESSION_ROLE").ok().filter(|s| !s.is_empty())?;
    let project = std::env::var("AIDA_SESSION_PROJECT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| statusline_project_root());
    let (state, _) = load_role(&project, &role_name).ok()?;
    if state.scope_tags.is_empty() && state.scope_status.is_none() {
        return None;
    }
    Some((state.scope_tags, state.scope_status))
}

fn handle_role_scope(project_root: &std::path::Path, cmd: &RoleScopeCommand) -> Result<()> {
    match cmd {
        RoleScopeCommand::Set {
            name,
            tags,
            status,
        } => {
            if tags.is_none() && status.is_none() {
                anyhow::bail!(
                    "At least one of --tags or --status is required.\n\
                     Use `aida role scope clear` to remove an existing scope."
                );
            }
            let role_name = resolve_role_name(name.as_deref())?;
            let (mut state, path) = load_role(project_root, &role_name)?;
            if let Some(t) = tags {
                state.scope_tags = t
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            if let Some(s) = status {
                state.scope_status = Some(s.clone());
            }
            save_role_at(&state, &path)?;
            print_role_scope(&state);
        }
        RoleScopeCommand::Show { name } => {
            let role_name = resolve_role_name(name.as_deref())?;
            let (state, _) = load_role(project_root, &role_name)?;
            print_role_scope(&state);
        }
        RoleScopeCommand::Clear {
            name,
            tags,
            status,
        } => {
            let role_name = resolve_role_name(name.as_deref())?;
            let (mut state, path) = load_role(project_root, &role_name)?;
            // No flags = clear everything; otherwise clear only the specified field(s).
            let clear_all = !*tags && !*status;
            if clear_all || *tags {
                state.scope_tags.clear();
            }
            if clear_all || *status {
                state.scope_status = None;
            }
            save_role_at(&state, &path)?;
            print_role_scope(&state);
        }
    }
    Ok(())
}

fn print_role_scope(state: &RoleState) {
    println!("{} {}", "Role:".bold(), state.name.cyan());
    if state.scope_tags.is_empty() && state.scope_status.is_none() {
        println!("  {}", "No scope filters set.".dimmed());
        return;
    }
    if !state.scope_tags.is_empty() {
        println!("  {}: {}", "tags".bold(), state.scope_tags.join(", "));
    }
    if let Some(s) = &state.scope_status {
        println!("  {}: {}", "status".bold(), s);
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
        scope_tags: Vec::new(),
        scope_status: None,
        system_prompt: None,
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
    // Strip any prior role prefix from PS1 (anywhere in the string, not just
    // at the front — `aida dev activate` may have prepended its own prefix
    // since this role was entered). Keyed off the prior AIDA_SESSION_ROLE
    // so the match is a literal substring. ${VAR/pat/} works in bash and zsh.
    println!("if [ -n \"${{PS1+x}}\" ] && [ -n \"${{AIDA_SESSION_ROLE:-}}\" ]; then");
    println!("    PS1=\"${{PS1/(role:$AIDA_SESSION_ROLE) /}}\"");
    println!("fi");
    println!("export AIDA_SESSION_ROLE='{}'", state.name);
    if let Some(p) = &state.purpose {
        println!("export AIDA_SESSION_PURPOSE='{}'", p.replace('\'', "'\\''"));
    } else {
        println!("unset AIDA_SESSION_PURPOSE");
    }
    println!("export AIDA_SESSION_PROJECT='{}'", project_root.display());
    println!("if [ -n \"${{PS1+x}}\" ]; then");
    println!("    export PS1=\"(role:{}) $PS1\"", state.name);
    println!("fi");
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

/// `aida role active` — one-line stub that prints just the active role
/// name, scriptable counterpart to `git branch --show-current` and
/// `git config --get user.email`. Pure read of `$AIDA_SESSION_ROLE` so
/// it never loads the project store; exits 1 with empty stdout when no
/// role is active so shell guards like `[ -n "$(aida role active)" ]`
/// work without parsing. trace:TASK-42 | ai:claude
fn handle_role_active() -> Result<()> {
    match std::env::var("AIDA_SESSION_ROLE") {
        Ok(role) if !role.is_empty() => {
            println!("{}", role);
            Ok(())
        }
        _ => std::process::exit(1),
    }
}

fn handle_role_end() -> Result<()> {
    // Use a uniquely-named env var rather than `local` so the eval works
    // both at the shell top level and inside a wrapper function.
    println!("# aida role end");
    println!("__AIDA_ROLE_END_PREV=\"${{AIDA_SESSION_ROLE:-}}\"");
    // Strip the role's PS1 prefix before unsetting, while we still know
    // the role name to match against.
    println!("if [ -n \"${{PS1+x}}\" ] && [ -n \"$__AIDA_ROLE_END_PREV\" ]; then");
    println!("    PS1=\"${{PS1/(role:$__AIDA_ROLE_END_PREV) /}}\"");
    println!("fi");
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
    // trace:TASK-1-021 | ai:claude
    if !state.scope_tags.is_empty() || state.scope_status.is_some() {
        let mut parts: Vec<String> = Vec::new();
        if !state.scope_tags.is_empty() {
            parts.push(format!("tags={}", state.scope_tags.join(",")));
        }
        if let Some(s) = &state.scope_status {
            parts.push(format!("status={}", s));
        }
        println!("Scope:       {}", parts.join(" "));
    }
    // trace:TASK-1-022 | ai:claude
    if let Some(text) = &state.system_prompt {
        let preview: String = text.lines().next().unwrap_or("").chars().take(80).collect();
        let suffix = if text.len() > preview.len() { "…" } else { "" };
        println!("Addendum:    {} chars — {}{}", text.len(), preview, suffix);
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
            scope_tags: Vec::new(),
            scope_status: None,
            system_prompt: None,
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

// ----------------------------------------------------------------------------
// EPIC-21 — `aida store`: code↔store commit pairing.
// trace:EPIC-21 | ai:claude
// ----------------------------------------------------------------------------

fn handle_store_command(cmd: &cli::StoreCommand) -> Result<()> {
    match cmd {
        cli::StoreCommand::Status => store_status(),
        cli::StoreCommand::InstallHook { force } => store_install_hook(*force),
    }
}

/// Print the alignment between this commit's paired store SHA (from the
/// `Aida-Store:` trailer) and the current orphan store HEAD. Reports
/// "aligned" when they match, "drift" with commit count otherwise.
/// trace:EPIC-21 | ai:claude
fn store_status() -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");

    // Current code HEAD.
    let code_head = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });
    let code_head = match code_head {
        Some(s) if !s.is_empty() => s,
        _ => anyhow::bail!("not in a git repo or no commits yet"),
    };

    // Paired store SHA — read the Aida-Store trailer from HEAD's message.
    let head_msg = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args(["log", "-1", "--format=%B"])
        .output()?;
    let head_msg = String::from_utf8_lossy(&head_msg.stdout).to_string();
    let trailers = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args(["interpret-trailers", "--parse"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    {
        use std::io::Write;
        if let Some(mut stdin) = trailers.stdin.as_ref().take() {
            let _ = stdin.write_all(head_msg.as_bytes());
        }
    }
    let trailer_output = trailers.wait_with_output()?;
    let trailer_text = String::from_utf8_lossy(&trailer_output.stdout).to_string();
    let paired_store_sha: Option<String> = trailer_text
        .lines()
        .find_map(|l| l.strip_prefix("Aida-Store:").map(|s| s.trim().to_string()));

    // Current orphan-store HEAD.
    let store_head: Option<String> = if store_path.exists() {
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
    } else {
        None
    };

    println!("{}", "Code ↔ store pairing".bold());
    println!("  code HEAD:        {}", short_sha(&code_head).cyan());
    match &paired_store_sha {
        Some(p) => println!("  paired store SHA: {}", short_sha(p).cyan()),
        None => {
            println!(
                "  paired store SHA: {} {}",
                "(none)".dimmed(),
                "— commit was made before the prepare-commit-msg hook was installed".dimmed()
            );
        }
    }
    match &store_head {
        Some(s) => println!("  current store:    {}", short_sha(s).cyan()),
        None => println!("  current store:    {} (no .aida-store/)", "(missing)".yellow()),
    }
    println!();

    match (paired_store_sha.as_deref(), store_head.as_deref()) {
        (Some(p), Some(c)) if p == c => {
            println!("{} aligned — code commit was paired with the current store HEAD.", "✓".green());
        }
        (Some(p), Some(c)) => {
            // Compute drift: how many commits is store HEAD ahead of/behind paired.
            let drift = std::process::Command::new("git")
                .arg("-C")
                .arg(&store_path)
                .args(["rev-list", "--left-right", "--count", &format!("{}...{}", p, c)])
                .output()
                .ok();
            let (behind, ahead) = match drift {
                Some(o) if o.status.success() => {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    let parts: Vec<&str> = s.split_whitespace().collect();
                    let b: i32 = parts.first().and_then(|x| x.parse().ok()).unwrap_or(0);
                    let a: i32 = parts.get(1).and_then(|x| x.parse().ok()).unwrap_or(0);
                    (b, a)
                }
                _ => (-1, -1),
            };
            if behind < 0 {
                println!(
                    "{} drift — store moved since this commit was made (could not count commits, paired SHA may not exist locally).",
                    "·".yellow()
                );
            } else {
                println!(
                    "{} drift — store moved {} commit(s) since this code commit was made.",
                    "·".yellow(),
                    ahead
                );
                if behind > 0 {
                    println!(
                        "  Note: {} commit(s) on the paired SHA are not on the current store HEAD — possible store-side history rewrite.",
                        behind
                    );
                }
            }
            println!(
                "  v2 will offer: {}",
                "aida store checkout HEAD   # rewind .aida-store/ to the paired SHA".dimmed()
            );
        }
        (None, _) => {
            println!(
                "{} no Aida-Store trailer on this commit — install the hook with: {}",
                "·".dimmed(),
                "aida store install-hook".cyan()
            );
        }
        (Some(_), None) => {
            println!(
                "{} commit has a paired SHA but no .aida-store/ in the worktree to compare against.",
                "·".yellow()
            );
        }
    }
    Ok(())
}

fn short_sha(sha: &str) -> String {
    if sha.len() >= 7 {
        sha[..7].to_string()
    } else {
        sha.to_string()
    }
}

/// Install the prepare-commit-msg hook from EMBEDDED_TEMPLATES into
/// `.git/hooks/prepare-commit-msg`. Idempotent. trace:EPIC-21 | ai:claude
fn store_install_hook(force: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let hooks_dir = project_root.join(".git").join("hooks");
    if !hooks_dir.exists() {
        anyhow::bail!(
            "{} doesn't exist — is this a git repo?",
            hooks_dir.display()
        );
    }
    let target = hooks_dir.join("prepare-commit-msg");
    if target.exists() && !force {
        anyhow::bail!(
            "{} already exists — pass --force to overwrite, or inspect with `cat {}`",
            target.display(),
            target.display()
        );
    }

    let body = aida_core::templates::EMBEDDED_TEMPLATES
        .get("hooks/aida-store-pair.sh")
        .copied()
        .ok_or_else(|| anyhow::anyhow!("aida-store-pair.sh missing from embedded templates"))?;

    std::fs::write(&target, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&target)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&target, perms)?;
    }

    println!(
        "{} installed prepare-commit-msg hook at {}",
        "✓".green().bold(),
        target.display().to_string().cyan()
    );
    println!();
    println!("Every future code commit will get an `Aida-Store: <sha>` trailer pinning the");
    println!("orphan-store HEAD at commit time. Inspect alignment with: {}", "aida store status".cyan());
    Ok(())
}

// ----------------------------------------------------------------------------
// EPIC-19 — `aida doctor`: maintenance + migration commands.
// ----------------------------------------------------------------------------

fn handle_doctor_command(cmd: &cli::DoctorCommand) -> Result<()> {
    match cmd {
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

/// STORY-70: a STORY or BUG description should carry an acceptance
/// section that STORY-67's review-prompt generator can lift verbatim.
/// Returns true when the requirement's type is in the lint scope and
/// its description doesn't carry one of the recognized headings.
/// trace:STORY-70 | ai:claude
fn requirement_missing_acceptance(req: &aida_core::Requirement) -> bool {
    matches!(
        req.req_type,
        aida_core::RequirementType::Story | aida_core::RequirementType::Bug
    ) && extract_acceptance_section(&req.description).is_none()
}

/// STORY-70: walk the orphan store, flag STORY/BUG requirements whose
/// descriptions don't contain a recognized acceptance heading. Output
/// shape mirrors the other doctor commands (per-finding rows + a final
/// summary). Exits non-zero on findings so CI/scripts can gate on it.
/// trace:STORY-70 | ai:claude
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
            "✓".green(),
            total_in_scope
        );
        return Ok(());
    }

    if !quiet {
        for (id, title) in &missing {
            println!(
                "{} {}  no `## Acceptance` / `## Verify` section  {}",
                "⚠".yellow(),
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
        "run `aida edit <id>` to add — STORY-67 will pick it up automatically"
            .dimmed()
    );
    std::process::exit(1);
}

/// Walk every YAML in objects/, collect every `relationships[*].target_id`
/// reference, and verify each resolves to an existing req's UUID. Reports
/// dangling references, optionally repairs by stripping the bad entries.
/// trace:EPIC-19 | ai:claude
fn doctor_verify_relationships(repair: bool, yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    let objects_root = store_path.join("objects");
    if !objects_root.exists() {
        println!("(no objects/ tree — nothing to check)");
        return Ok(());
    }

    let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
    walk_yamls(&objects_root, &mut yaml_files);

    // First pass: collect every uuid present in the store.
    let mut all_uuids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut spec_by_uuid: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for path in &yaml_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut uuid = String::new();
        let mut spec_id = String::new();
        for raw in content.lines() {
            let line = raw.trim_start();
            if uuid.is_empty() {
                if let Some(v) = line.strip_prefix("id:") {
                    uuid = v.trim().trim_matches('"').trim_matches('\'').to_string();
                }
            }
            if spec_id.is_empty() {
                if let Some(v) = line.strip_prefix("spec_id:") {
                    spec_id = v.trim().trim_matches('"').trim_matches('\'').to_string();
                }
            }
            if !uuid.is_empty() && !spec_id.is_empty() {
                break;
            }
        }
        if !uuid.is_empty() {
            all_uuids.insert(uuid.clone());
            if !spec_id.is_empty() {
                spec_by_uuid.insert(uuid, spec_id);
            }
        }
    }

    // Second pass: collect (source_uuid, target_uuid, rel_type) triples
    // and check each target.
    #[derive(Debug)]
    struct Dangling {
        source_path: std::path::PathBuf,
        source_spec: String,
        target_uuid: String,
        rel_type: String,
    }
    let mut dangling: Vec<Dangling> = Vec::new();

    for path in &yaml_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        // Find the source spec_id once for nicer reporting.
        let source_spec = content
            .lines()
            .find_map(|l| l.trim_start().strip_prefix("spec_id:"))
            .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
            .unwrap_or_else(|| "?".to_string());

        // Scan for relationships block. The shape (in serde-flavored YAML):
        //   relationships:
        //   - rel_type: parent
        //     target_id: <uuid>
        //     ...
        let mut in_rel_block = false;
        let mut current_rel_type = String::new();
        for raw in content.lines() {
            let trimmed = raw.trim_start();
            if !raw.starts_with(' ') && trimmed.starts_with("relationships:") {
                in_rel_block = true;
                continue;
            }
            // Leaving the relationships block — heuristic: any non-indented
            // top-level YAML key.
            if in_rel_block && !raw.starts_with(' ') && !trimmed.is_empty() && trimmed.contains(':')
            {
                in_rel_block = false;
                continue;
            }
            if !in_rel_block {
                continue;
            }
            if let Some(v) = trimmed.strip_prefix("rel_type:") {
                current_rel_type =
                    v.trim().trim_matches('"').trim_matches('\'').to_string();
            }
            if let Some(v) = trimmed.strip_prefix("target_id:") {
                let target_uuid =
                    v.trim().trim_matches('"').trim_matches('\'').to_string();
                if !target_uuid.is_empty() && !all_uuids.contains(&target_uuid) {
                    dangling.push(Dangling {
                        source_path: path.clone(),
                        source_spec: source_spec.clone(),
                        target_uuid,
                        rel_type: current_rel_type.clone(),
                    });
                }
            }
        }
    }

    if dangling.is_empty() {
        println!(
            "{} every relationship target resolves to an existing requirement.",
            "✓".green()
        );
        return Ok(());
    }

    println!(
        "{} {} dangling relationship reference(s):",
        "✗".red().bold(),
        dangling.len()
    );
    println!();
    for d in &dangling {
        println!(
            "  {} → {}: target uuid {} not found",
            d.source_spec.bold(),
            d.rel_type.dimmed(),
            d.target_uuid.yellow()
        );
        println!("    in {}", d.source_path.display().to_string().dimmed());
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

    // Repair: load each affected req via aida-core, filter relationships,
    // save back. trace:EPIC-19 | ai:claude
    let store = aida_core::GitBackend::new(&store_path)?.load()?;
    let dangling_uuids: std::collections::HashSet<String> =
        dangling.iter().map(|d| d.target_uuid.clone()).collect();
    let mut fixed = 0usize;
    for mut req in store.requirements.into_iter() {
        let before = req.relationships.len();
        req.relationships
            .retain(|r| !dangling_uuids.contains(&r.target_id.to_string()));
        if req.relationships.len() != before {
            aida_core::object_store::write_object(&store_path.join("objects"), &req)?;
            fixed += 1;
        }
    }
    let _ = aida_core::git_ops::add(&store_path, &["objects"]);
    let _ = aida_core::git_ops::commit(
        &store_path,
        &format!("chore(repair): strip {} dangling relationship target(s)", dangling.len()),
    );
    println!("{} repaired {} requirement(s).", "✓".green().bold(), fixed);
    println!("  Push with: {}", "aida push".cyan());
    Ok(())
}

/// Walk source files under the project root for `trace:<SPEC-ID>`
/// patterns and verify each spec_id resolves to a requirement in the
/// store. With `strip_dangling`, rewrites source files to remove the
/// dangling trace markers. trace:EPIC-19 | ai:claude
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
    let mut known_specs: std::collections::HashSet<String> =
        std::collections::HashSet::new();
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
    let trace_re =
        regex::Regex::new(r"trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)").unwrap();
    let mut by_spec: std::collections::HashMap<String, Vec<(std::path::PathBuf, usize)>> =
        std::collections::HashMap::new();

    walk_source_for_traces(&project_root, &project_root, &trace_re, &mut by_spec);

    let mut orphan_specs: Vec<&String> = by_spec
        .keys()
        .filter(|s| !known_specs.contains(*s))
        .collect();
    orphan_specs.sort();

    if orphan_specs.is_empty() {
        println!(
            "{} every `trace:<SPEC-ID>` in source resolves to a requirement ({} unique spec_ids referenced from {} location(s)).",
            "✓".green(),
            by_spec.len(),
            by_spec.values().map(|v| v.len()).sum::<usize>()
        );
        return Ok(());
    }

    println!(
        "{} {} trace comment(s) reference unknown spec_ids:",
        "✗".red().bold(),
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
        println!("To strip these in-place: {}", "aida doctor validate-trace-comments --strip-dangling".cyan());
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
        "✓".green().bold(),
        stats.lines_deleted + stats.lines_modified,
        stats.lines_deleted,
        stats.lines_modified,
        stats.files_changed
    );
    println!(
        "  Review the diff: {}",
        "git diff".cyan()
    );
    Ok(())
}

#[derive(Default)]
struct StripStats {
    files_changed: usize,
    lines_deleted: usize,
    lines_modified: usize,
}

/// Walk every text source file under `root` and either delete or modify
/// any line containing `trace:<DANGLING>` per the dangling_ids set.
/// When `dry_run`, returns counts without writing. trace:EPIC-19
fn strip_dangling_traces(
    root: &std::path::Path,
    dangling_ids: &std::collections::HashSet<String>,
    dry_run: bool,
) -> Result<StripStats> {
    let mut stats = StripStats::default();
    strip_dangling_walk(root, root, dangling_ids, dry_run, &mut stats);
    Ok(stats)
}

fn strip_dangling_walk(
    root: &std::path::Path,
    project_root: &std::path::Path,
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
                ".git" | ".aida-store" | ".aida" | "target" | "node_modules"
                    | "dist" | "build" | ".cache" | ".venv" | "venv"
            ) {
                continue;
            }
            strip_dangling_walk(&path, project_root, dangling_ids, dry_run, stats);
            continue;
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let probably_text = matches!(
            ext,
            "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "java"
                | "c" | "cpp" | "h" | "hpp" | "cs" | "rb" | "sh" | "md"
                | "toml" | "yaml" | "yml" | "json"
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
/// fragment is excised and the line is "modified". trace:EPIC-19
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
                    caps.get(0).map(|m| m.as_str().to_string()).unwrap_or_default()
                }
            })
            .into_owned();

        // Decide: delete the whole line if what remains is just a
        // comment marker (no content), or modify (keep the line with
        // the fragment removed).
        let trimmed = stripped.trim();
        let is_just_marker = matches!(
            trimmed,
            "" | "//" | "///" | "//!" | "/*" | "*/" | "*" | "#"
        ) || trimmed.starts_with("// ") && trimmed.trim_end_matches(' ').len() <= 3;

        if is_just_marker {
            deleted += 1;
            // skip — don't push this line
        } else {
            modified += 1;
            // Clean up double-spaces left behind by the strip.
            let cleaned = stripped
                .replace("  ", " ")
                .trim_end()
                .to_string();
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

/// Recurse into source files looking for trace comments. Skips the
/// usual "don't grep here" directories. trace:EPIC-19 | ai:claude
fn walk_source_for_traces(
    root: &std::path::Path,
    project_root: &std::path::Path,
    re: &regex::Regex,
    out: &mut std::collections::HashMap<String, Vec<(std::path::PathBuf, usize)>>,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        // Skip noise dirs.
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
            walk_source_for_traces(&path, project_root, re, out);
            continue;
        }
        // Read text-like files only. trace:EPIC-19 | ai:claude
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let probably_text = matches!(
            ext,
            "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "java"
                | "c" | "cpp" | "h" | "hpp" | "cs" | "rb" | "sh" | "md"
                | "toml" | "yaml" | "yml" | "json"
        );
        if !probably_text {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (lineno, line) in content.lines().enumerate() {
            for cap in re.captures_iter(line) {
                if let Some(m) = cap.get(1) {
                    out.entry(m.as_str().to_string())
                        .or_default()
                        .push((path.clone(), lineno + 1));
                }
            }
        }
    }
}

/// Recursively collect every `*.yaml` file under `root` into `out`.
/// Hand-rolled to avoid adding a walkdir dep just for the doctor ops.
/// The orphan store's objects/ tree is shallow (3 levels) so a simple
/// recursive read_dir is fine. trace:EPIC-19 | ai:claude
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
/// collision). trace:EPIC-19 | ai:claude
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
        println!("{} no stale blocks — every active block has a registered node.", "✓".green());
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
        &format!("chore(registry): tombstone {} stale block(s) (no node owner)", count),
    );

    println!("{} tombstoned {} block(s).", "✓".green().bold(), count);
    println!("  Push with: {}", "aida push".cyan());
    Ok(())
}

/// Walk the orphan store's objects tree, group requirements by their
/// `spec_id` field, and report any spec_id claimed by more than one
/// requirement. v1 reports only — auto-renumber is dangerous (would
/// orphan trace comments + commit refs). trace:EPIC-19 | ai:claude
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
            "✓".green()
        );
        return Ok(());
    }

    println!(
        "{} {} spec_id collision(s) found:",
        "✗".red().bold(),
        collisions.len()
    );
    println!();
    for (spec, entries) in &collisions {
        println!("{}:", spec.bold());
        for (uuid, title, path) in entries.iter() {
            let title_disp = if title.is_empty() { "(no title)".dimmed().to_string() } else { title.clone() };
            println!(
                "  {} {} — {}",
                uuid.yellow(),
                title_disp,
                path.dimmed()
            );
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
/// problem so it can gate CI. trace:EPIC-19 | ai:claude
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
            println!("  {} every active block has a registered node owner.", "✓".green());
        } else {
            had_problem = true;
            println!(
                "  {} {} block-owning node(s) not in nodes.toml: {}",
                "✗".red(),
                orphan_blocks.len(),
                orphan_blocks.join(", ")
            );
            println!(
                "    fix: {}",
                "aida doctor repair-stale-blocks".cyan()
            );
        }
    } else {
        println!("  {} no blocks.yaml — skipping (project may be node-aware-only).", "·".dimmed());
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
            println!("  {} every spec_id maps to one requirement.", "✓".green());
        } else {
            had_problem = true;
            println!(
                "  {} {} spec_id(s) claimed by multiple requirements: {}",
                "✗".red(),
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
        println!("  {} cache exists at {} (run `aida cache status` for HEAD-vs-store check).", "·".dimmed(), cache_path.display());
    } else {
        println!("  {} cache missing — run `aida cache rebuild` if list/search are slow.", "·".dimmed());
    }
    println!();

    // --- Check 4: relationship targets resolve ---
    println!("{}", "── relationships ──".bold());
    if objects_root.exists() {
        let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
        walk_yamls(&objects_root, &mut yaml_files);
        let mut all_uuids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
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
            let Ok(content) = std::fs::read_to_string(path) else { continue; };
            let mut in_rel = false;
            for raw in content.lines() {
                let trimmed = raw.trim_start();
                if !raw.starts_with(' ') && trimmed.starts_with("relationships:") {
                    in_rel = true;
                    continue;
                }
                if in_rel
                    && !raw.starts_with(' ')
                    && !trimmed.is_empty()
                    && trimmed.contains(':')
                {
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
            println!("  {} every relationship target resolves.", "✓".green());
        } else {
            had_problem = true;
            println!(
                "  {} {} dangling relationship reference(s).",
                "✗".red(),
                dangling
            );
            println!("    fix: {}", "aida doctor verify-relationships --repair".cyan());
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
        let mut known_specs: std::collections::HashSet<String> =
            std::collections::HashSet::new();
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
        let mut by_spec: std::collections::HashMap<
            String,
            Vec<(std::path::PathBuf, usize)>,
        > = std::collections::HashMap::new();
        walk_source_for_traces(&project_root, &project_root, &trace_re, &mut by_spec);
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
                "✓".green(),
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
            println!("  {} config=global, blocks have a `*` block. Consistent.", "✓".green());
        }
        (aida_core::IdCounterScope::Global, false) => {
            had_problem = true;
            println!(
                "  {} config says global but no `*` block exists. New `aida add` would fall back to per-type.",
                "✗".red()
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
            println!("  {} config=per-type, no `*` block. Consistent.", "✓".green());
        }
    }
    println!();

    if had_problem {
        println!("{}", "fsck found problems — see above.".red().bold());
        std::process::exit(1);
    } else {
        println!("{}", "✓ fsck clean.".green().bold());
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
            "✓".green()
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
        .map(|b| b.clone())
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
    println!("  - new requirements use the global counter (FR-{}, BUG-{}, etc.)", new_start, new_start + 1);
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
    println!("{} migration complete.", "✓".green().bold());
    println!(
        "  new global block: {}",
        format!("*-{}..{}", new_start, new_end).cyan()
    );
    println!("  next `aida add` will dispense {}", format!("<TYPE>-{}", new_start).cyan());
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

/// trace:FR-1-043 | ai:claude
fn handle_session_command(cmd: &SessionCommand) -> Result<()> {
    match cmd {
        SessionCommand::List { limit, no_color, all } => session::list(*limit, *no_color, *all),
        SessionCommand::Resume { id, limit } => session::resume(id.clone(), *limit),
        SessionCommand::New {
            title,
            permission_mode,
            role,
        } => session::new_session(title.clone(), permission_mode, role.clone()),
        SessionCommand::Start {
            owns,
            branch,
            base,
            path,
            forge,
            launch,
            title,
            permission_mode,
            role,
        } => session_start(
            owns,
            branch.as_deref(),
            base.as_deref(),
            path.as_deref(),
            forge.as_deref(),
            *launch,
            title.clone(),
            permission_mode,
            role.clone(),
        ),
        SessionCommand::End { id, yes } => session_end(id.as_deref(), *yes),
        SessionCommand::Leases => session_leases(),
        SessionCommand::Prune { days, dry_run, yes } => {
            session_prune(*days, *dry_run, *yes)
        }
    }
}

// ----------------------------------------------------------------------------
// EPIC-20 v1 — scoped session leases.
// trace:EPIC-20 | ai:claude
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SessionLease {
    /// 12-char hex id derived from the time-ordered uuid v7, for short refs.
    id: String,
    /// Raw scope string the user passed to `--owns`.
    scope: String,
    /// Resolved scope slug, used for branch + dir naming.
    slug: String,
    /// Owner email/user (best-effort from git config).
    owner: String,
    /// Worktree path (canonicalized).
    worktree_path: std::path::PathBuf,
    /// Branch the worktree is on.
    branch: String,
    /// ISO-8601 UTC.
    started_at: chrono::DateTime<chrono::Utc>,
    /// Hostname when started.
    hostname: String,
    /// Parent project's `target/` dir, captured so the session shell can
    /// share its cargo build cache with the parent worktree (avoids a full
    /// rebuild on first `cargo build` inside the session). `None` when the
    /// project doesn't have a `target/` (e.g., non-Rust project, or fresh
    /// checkout that's never been built). Sourced via `.aida/session-env.sh`.
    /// trace:STORY-52 | ai:claude
    #[serde(default)]
    cargo_target_dir: Option<std::path::PathBuf>,
    /// Parent project root (the worktree where `aida session start` ran).
    /// Recorded so cross-worktree views like `aida session list` can walk
    /// the parent's Claude Code session directory in addition to the
    /// session worktree's own. `None` for leases written before STORY-58.
    /// trace:STORY-58 | ai:claude
    #[serde(default)]
    parent_project_root: Option<std::path::PathBuf>,
    /// STORY-71: for PR/MR review sessions (--owns PR-N / MR-N), the
    /// PR head commit SHA captured via `gh pr view` / `glab mr view` at
    /// session-start time. `None` when the scope isn't a PR/MR or when
    /// the forge CLI wasn't available. Surfaced by `aida session show`.
    /// trace:STORY-71 | ai:claude
    #[serde(default)]
    pr_head_sha: Option<String>,
    /// STORY-71: PR base commit SHA at session-start time (companion to
    /// pr_head_sha). Lets the reviewer recompute the diff range later
    /// without round-tripping to the forge.
    /// trace:STORY-71 | ai:claude
    #[serde(default)]
    pr_base_sha: Option<String>,
    /// STORY-71: PR base ref name (e.g. `main`). Mostly informational —
    /// for reporting in `aida session show`.
    /// trace:STORY-71 | ai:claude
    #[serde(default)]
    pr_base_ref: Option<String>,
}

fn leases_dir(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida").join("sessions")
}

/// Max display width for the @SCOPE statusline segment (matches @SPEC budget).
/// trace:STORY-55 | ai:claude
const SCOPE_LABEL_MAX: usize = 12;

fn lease_path(project_root: &std::path::Path, id: &str) -> std::path::PathBuf {
    leases_dir(project_root).join(format!("{}.toml", id))
}

fn list_leases(project_root: &std::path::Path) -> Vec<SessionLease> {
    let dir = leases_dir(project_root);
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("toml") {
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

/// Find the active session lease whose worktree contains `cwd`, if any.
/// Used by statusline + enforcement to identify which session "owns" the
/// shell the user is operating from.
/// trace:STORY-55 | ai:claude
fn active_lease_for_cwd(
    project_root: &std::path::Path,
    cwd: &std::path::Path,
) -> Option<SessionLease> {
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    list_leases(project_root)
        .into_iter()
        .find(|l| canon == l.worktree_path || canon.starts_with(&l.worktree_path))
}

/// STORY-58: from inside a session worktree, return the parent project's
/// root path so cross-worktree views (currently just `aida session list`)
/// can also surface sessions launched from the parent. `None` when:
///   - cwd isn't covered by an active lease (not in a session), OR
///   - the lease was written before STORY-58 (no parent recorded), OR
///   - we can't locate any project root to read leases from.
/// trace:STORY-58 | ai:claude
pub(crate) fn parent_project_root_for_session(
    cwd: &std::path::Path,
) -> Option<std::path::PathBuf> {
    // The lease dir is `<root>/.aida/sessions/`. Inside a session worktree
    // that dir is a symlink back to the parent project's, so `list_leases`
    // returns the same set either way. Walk up from cwd looking for any
    // ancestor that has `.aida/sessions/`, then ask for its lease set.
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut probe = canon.as_path();
    loop {
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
struct SessionActivityLog {
    #[serde(default)]
    entries: Vec<SessionActivityEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SessionActivityEntry {
    /// Role active when this entry was recorded — needed at session-end to
    /// know which project-level role file to fold the entry back into.
    role: String,
    spec_id: String,
    action: String,
    at: chrono::DateTime<chrono::Utc>,
}

/// Hard cap on session activity entries — keeps the file small for long
/// sessions. Older-than-cap entries fall off the back (so the newest cap
/// entries are kept, matching the project-level role's stream behavior).
const SESSION_ACTIVITY_MAX: usize = 200;

fn session_activity_path(project_root: &std::path::Path, id: &str) -> std::path::PathBuf {
    leases_dir(project_root).join(format!("{}.activity.toml", id))
}

fn load_session_activity(
    project_root: &std::path::Path,
    id: &str,
) -> SessionActivityLog {
    let path = session_activity_path(project_root, id);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return SessionActivityLog::default();
    };
    toml::from_str(&content).unwrap_or_default()
}

fn save_session_activity(
    project_root: &std::path::Path,
    id: &str,
    log: &SessionActivityLog,
) -> Result<()> {
    let path = session_activity_path(project_root, id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(log)?;
    std::fs::write(&path, content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Append an activity entry to the given session's log. Dedupes against
/// the most recent entry the same way project-level activity does:
/// consecutive (role, spec_id, action) collapse into one entry whose
/// timestamp ticks forward.
/// trace:STORY-56 | ai:claude
fn append_session_activity(
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
        !(prev.role == entry.role
            && prev.spec_id == entry.spec_id
            && prev.action == entry.action)
    });
    log.entries.insert(0, entry);
    log.entries.truncate(SESSION_ACTIVITY_MAX);
    save_session_activity(project_root, session_id, &log)
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
fn entry_scope_session_match(
    entry: &aida_core::QueueEntry,
    self_lease: Option<&SessionLease>,
    bypass: bool,
) -> bool {
    if bypass {
        return true;
    }
    if let Some(want_scope) = entry.for_scope.as_deref() {
        let Some(lease) = self_lease else {
            return false;
        };
        if !lease.scope.eq_ignore_ascii_case(want_scope) {
            return false;
        }
    }
    if let Some(want_sess) = entry.for_session.as_deref() {
        let Some(lease) = self_lease else {
            return false;
        };
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

/// Fold a closed session's activity entries back into each participating
/// role's project-level `activity` stream. Called from `session_end` so
/// long-running views (`aida role show`, `aida statusline` outside any
/// session) still surface what was worked on under the session. Does NOT
/// delete the activity log file — `session_end` handles that after this
/// returns successfully.
///
/// Per role: take the newest session entry per spec_id, merge in front of
/// the project role's existing activity, dedupe by spec_id, truncate to
/// ACTIVITY_MAX. Best-effort — malformed/missing role files are skipped
/// (the project role might have been deleted while the session ran).
/// trace:STORY-56 | ai:claude
fn aggregate_session_activity_into_roles(
    project_root: &std::path::Path,
    session_id: &str,
) {
    let log = load_session_activity(project_root, session_id);
    if log.entries.is_empty() {
        return;
    }
    // Group newest-first by role; within each role the first entry per
    // spec_id wins (newest, since `entries` is newest-first by design).
    use std::collections::{BTreeMap, BTreeSet};
    let mut per_role: BTreeMap<String, Vec<RoleActivity>> = BTreeMap::new();
    for entry in &log.entries {
        let bucket = per_role.entry(entry.role.clone()).or_default();
        if !bucket.iter().any(|a| a.spec_id == entry.spec_id) {
            bucket.push(RoleActivity {
                spec_id: entry.spec_id.clone(),
                action: entry.action.clone(),
                at: entry.at,
            });
        }
    }
    for (role_name, mut new_entries) in per_role {
        let Ok((mut state, path)) = load_role(project_root, &role_name) else {
            continue;
        };
        // Merge: prepend session-newest entries, then append project's
        // existing entries skipping any spec_id already brought forward.
        let promoted: BTreeSet<String> =
            new_entries.iter().map(|e| e.spec_id.clone()).collect();
        new_entries.extend(
            state
                .activity
                .iter()
                .filter(|e| !promoted.contains(&e.spec_id))
                .cloned(),
        );
        new_entries.truncate(ACTIVITY_MAX);
        state.activity = new_entries;
        state.last_active_at = chrono::Utc::now();
        let _ = save_role_at(&state, &path);
    }
}

/// Lease-enforcement mode for cross-session writes.
/// Configured via `[session] enforcement = "warn"|"block"|"off"` in
/// `.aida/config.toml`; default is `Warn`.
/// trace:STORY-48 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionEnforcement {
    Off,
    Warn,
    Block,
}

impl SessionEnforcement {
    fn from_config_str(s: &str) -> Self {
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
fn session_enforcement(project_root: &std::path::Path) -> SessionEnforcement {
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
fn lease_owning_spec(
    leases: &[SessionLease],
    self_lease: Option<&SessionLease>,
    target_uuid: Uuid,
    target_spec_id: Option<&str>,
    store: &RequirementsStore,
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
/// trace:STORY-48 | ai:claude
fn enforce_session_lease(
    project_root: &std::path::Path,
    target: &Requirement,
    store: &RequirementsStore,
    operation: &str,
    force_block: bool,
) -> Result<()> {
    let leases = list_leases(project_root);
    if leases.is_empty() {
        return Ok(());
    }
    let self_lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(project_root, &cwd));
    let owner = lease_owning_spec(
        &leases,
        self_lease.as_ref(),
        target.id,
        target.spec_id.as_deref(),
        store,
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

fn slugify(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_dash = false;
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Walk up from cwd looking for a .git directory — the project root. We
/// don't use git_ops::is_git_repo here because we need the *path*, not
/// just a yes/no.
fn find_project_root() -> Result<std::path::PathBuf> {
    let mut cur = std::env::current_dir()?;
    loop {
        if cur.join(".git").exists() {
            return Ok(cur);
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => anyhow::bail!("not inside a git repository"),
        }
    }
}

/// STORY-61: forge-specific PR/MR scope. When `aida session start
/// --owns PR-1` or `--owns MR-42` is invoked, we route through a
/// review-branch fetch + worktree-on-existing-branch flow instead of
/// the default new-branch flow.
/// trace:STORY-61 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewForge {
    GitHub,
    GitLab,
}

impl ReviewForge {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "github" | "gh" => Some(Self::GitHub),
            "gitlab" | "glab" => Some(Self::GitLab),
            _ => None,
        }
    }

    /// Standard refspec for the head of a PR/MR (works for fork PRs
    /// too because both forges expose the head ref on origin).
    fn pr_head_ref(&self, n: u64) -> String {
        match self {
            Self::GitHub => format!("pull/{}/head", n),
            Self::GitLab => format!("merge-requests/{}/head", n),
        }
    }

    fn local_branch_for(&self, n: u64) -> String {
        match self {
            Self::GitHub => format!("pr-{}", n),
            Self::GitLab => format!("mr-{}", n),
        }
    }

    /// STORY-71: forge CLI binary name used to enrich the lease with the
    /// PR's head/base SHAs (and to surface a clear stderr note when it's
    /// not on PATH).
    /// trace:STORY-71 | ai:claude
    fn cli_name(&self) -> &'static str {
        match self {
            Self::GitHub => "gh",
            Self::GitLab => "glab",
        }
    }

    fn cli_install_url(&self) -> &'static str {
        match self {
            Self::GitHub => "https://cli.github.com",
            Self::GitLab => "https://gitlab.com/gitlab-org/cli",
        }
    }
}

/// STORY-71: PR/MR head + base metadata captured at session-start time.
/// Recorded into the lease so `aida session show` can display it without
/// re-querying the forge, and so a reviewer can recompute the diff range
/// later even after the PR has moved on.
/// trace:STORY-71 | ai:claude
#[derive(Debug, Clone, Default)]
struct PrMetadata {
    head_sha: Option<String>,
    base_sha: Option<String>,
    base_ref: Option<String>,
}

/// STORY-71: query the forge CLI for a PR/MR's head/base SHAs + base ref.
/// Returns `Err(reason)` when the CLI isn't installed or the query fails
/// (caller turns that into a stderr note + Default fallback) and
/// `Ok(None)` when the JSON parsed but key fields were missing.
/// trace:STORY-71 | ai:claude
fn query_pr_metadata(
    forge: ReviewForge,
    n: u64,
    project_root: &std::path::Path,
) -> std::result::Result<PrMetadata, String> {
    let cli = forge.cli_name();
    let n_str = n.to_string();
    let args: Vec<&str> = match forge {
        ReviewForge::GitHub => vec![
            "pr",
            "view",
            n_str.as_str(),
            "--json",
            "headRefOid,baseRefOid,baseRefName",
        ],
        ReviewForge::GitLab => vec!["mr", "view", n_str.as_str(), "--output", "json"],
    };
    // gh/glab inherit the working directory; both honor cwd to find the
    // matching repo for the PR/MR number, so we set current_dir rather
    // than relying on `-C` which neither CLI accepts.
    let out = std::process::Command::new(cli)
        .current_dir(project_root)
        .args(&args)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "`{}` not on PATH ({} not installed?)",
                    cli,
                    forge.cli_install_url()
                )
            } else {
                format!("`{}` failed to spawn: {}", cli, e)
            }
        })?;
    if !out.status.success() {
        return Err(format!(
            "`{} {}` exited {}",
            cli,
            args.join(" "),
            out.status
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("`{}` JSON parse failed: {}", cli, e))?;
    Ok(parse_pr_metadata_json(forge, &json))
}

/// STORY-71: extract the head SHA / base SHA / base ref out of the JSON
/// the forge CLI returned. Pulled out of `query_pr_metadata` so unit
/// tests can pin the parsing without invoking the CLI.
/// trace:STORY-71 | ai:claude
fn parse_pr_metadata_json(forge: ReviewForge, json: &serde_json::Value) -> PrMetadata {
    let s = |v: &serde_json::Value| -> Option<String> {
        v.as_str().map(|s| s.to_string()).filter(|s| !s.is_empty())
    };
    match forge {
        ReviewForge::GitHub => PrMetadata {
            head_sha: s(&json["headRefOid"]),
            base_sha: s(&json["baseRefOid"]),
            base_ref: s(&json["baseRefName"]),
        },
        ReviewForge::GitLab => {
            // glab's mr view --output json field names mirror GitLab's
            // REST API: `sha` (head), `diff_refs.base_sha`, `target_branch`.
            // Source-of-truth list: `glab api projects/:id/merge_requests/N`.
            PrMetadata {
                head_sha: s(&json["sha"]),
                base_sha: s(&json["diff_refs"]["base_sha"]),
                base_ref: s(&json["target_branch"]),
            }
        }
    }
}

/// STORY-61: parse `PR-N` / `MR-N` scope strings (case-insensitive).
/// Returns the implied forge + PR number when the scope matches; None
/// otherwise (lets normal scope handling proceed).
/// trace:STORY-61 | ai:claude
fn parse_review_scope(scope: &str) -> Option<(ReviewForge, u64)> {
    let trimmed = scope.trim();
    let (prefix, rest) = trimmed.split_once('-')?;
    let n: u64 = rest.parse().ok()?;
    match prefix.to_ascii_uppercase().as_str() {
        "PR" => Some((ReviewForge::GitHub, n)),
        "MR" => Some((ReviewForge::GitLab, n)),
        _ => None,
    }
}

/// STORY-61: detect the project's forge by inspecting `origin`'s URL.
/// Returns None for hosts we don't recognize (Bitbucket, self-hosted
/// without telltale domain, etc.) — caller can require `--forge`.
/// trace:STORY-61 | ai:claude
fn detect_forge_from_origin(project_root: &std::path::Path) -> Option<ReviewForge> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
    if url.contains("github.com") {
        Some(ReviewForge::GitHub)
    } else if url.contains("gitlab.com") || url.contains("/gitlab/") {
        Some(ReviewForge::GitLab)
    } else {
        None
    }
}

fn session_start(
    owns: &str,
    branch: Option<&str>,
    base: Option<&str>,
    explicit_path: Option<&str>,
    forge_override: Option<&str>,
    launch_claude: bool,
    launch_title: Option<String>,
    launch_permission_mode: &str,
    launch_role: Option<String>,
) -> Result<()> {
    let project_root = find_project_root()?;
    let slug = slugify(owns);
    if slug.is_empty() {
        anyhow::bail!("scope `{}` slugifies to empty — pick something with letters/digits", owns);
    }

    // STORY-61: review-mode dispatch. When the scope is `PR-N` / `MR-N`
    // and the forge resolves (via override or origin-URL detection), we
    // hand off to the review flow which fetches the PR head ref into a
    // local branch and creates the worktree on that existing branch
    // (rather than `git worktree add -b <new-branch>`).
    let review_target: Option<(ReviewForge, u64)> = parse_review_scope(owns)
        .map(|(f, n)| {
            let resolved_forge = match forge_override.and_then(ReviewForge::parse) {
                Some(f) => f,
                None => detect_forge_from_origin(&project_root).unwrap_or(f),
            };
            (resolved_forge, n)
        });

    let branch_name = match (&review_target, branch) {
        (Some((forge, n)), None) => forge.local_branch_for(*n),
        _ => branch.map(|s| s.to_string()).unwrap_or_else(|| slug.clone()),
    };
    let repo_name = project_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string();
    let worktree_path = match explicit_path {
        Some(p) => std::path::PathBuf::from(p),
        None => project_root
            .parent()
            .ok_or_else(|| anyhow::anyhow!("project root has no parent"))?
            .join(format!("{}-{}", repo_name, slug)),
    };
    if worktree_path.exists() {
        anyhow::bail!(
            "{} already exists — pick a different --path or remove it first",
            worktree_path.display()
        );
    }

    // Check the lease dir exists; create if not.
    let leases = leases_dir(&project_root);
    std::fs::create_dir_all(&leases)?;

    // Don't double-claim the same scope from this project root.
    for existing in list_leases(&project_root) {
        if existing.scope.eq_ignore_ascii_case(owns) {
            anyhow::bail!(
                "scope `{}` is already owned by session {} ({}). \
                 Run `aida session end {}` first.",
                owns,
                existing.id,
                existing.worktree_path.display(),
                existing.id
            );
        }
    }

    // STORY-71: PR metadata captured from `gh`/`glab` for review sessions.
    // Populated below and stitched into the lease so `aida session show`
    // can display head/base SHAs without a round-trip to the forge.
    // trace:STORY-71 | ai:claude
    let mut pr_metadata: PrMetadata = PrMetadata::default();

    if let Some((forge, n)) = review_target {
        // STORY-61 review flow:
        //   1. fetch the PR/MR head ref from origin into the local branch
        //      <forge>-<N>; safe to re-run (creates or fast-forwards).
        //   2. `git worktree add <path> <local-branch>` checks out the
        //      existing branch — does NOT create a new one. Reviewer can
        //      then commit feedback / try fixes locally without disturbing
        //      the contributor's branch on origin.
        let head_ref = forge.pr_head_ref(n);
        let refspec = format!("+{}:{}", head_ref, branch_name);
        let fetch = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args(["fetch", "origin", refspec.as_str()])
            .status()?;
        if !fetch.success() {
            anyhow::bail!(
                "`git fetch origin {}` failed — is {} #{} valid? \
                 (For self-hosted forges, override with --forge github|gitlab.)",
                refspec,
                match forge {
                    ReviewForge::GitHub => "PR",
                    ReviewForge::GitLab => "MR",
                },
                n
            );
        }
        let res = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args([
                "worktree",
                "add",
                worktree_path.to_str().unwrap(),
                branch_name.as_str(),
            ])
            .status()?;
        if !res.success() {
            anyhow::bail!("`git worktree add` failed");
        }

        // STORY-71: enrich the lease with PR metadata via gh/glab. The
        // worktree is already on the PR's code (the fetch above did the
        // real work) — this pass is just for the head/base SHAs the
        // reviewer wants to see in `aida session show`. CLI-not-installed
        // is a soft failure: the session start succeeds and we print one
        // stderr line so the user knows what they're missing.
        // trace:STORY-71 | ai:claude
        match query_pr_metadata(forge, n, &project_root) {
            Ok(meta) => pr_metadata = meta,
            Err(reason) => {
                eprintln!(
                    "{} {}",
                    "ℹ".cyan(),
                    format!(
                        "skipped PR metadata capture: {} \u{2014} install {} from {} for richer `aida session show` output",
                        reason,
                        forge.cli_name(),
                        forge.cli_install_url()
                    )
                    .dimmed()
                );
            }
        }
    } else {
        // Default flow: create worktree on a NEW branch (the original
        // EPIC-20 behavior — work-in-progress sessions, not reviews).
        let mut args = vec![
            "worktree",
            "add",
            "-b",
            branch_name.as_str(),
            worktree_path.to_str().unwrap(),
        ];
        if let Some(b) = base {
            args.push(b);
        }
        let res = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args(&args)
            .status()?;
        if !res.success() {
            anyhow::bail!("`git worktree add` failed");
        }
    }

    // Make AIDA state from the parent visible inside the new worktree.
    // .aida-store/ is gitignored so a whole-directory symlink works.
    // .aida/ is partially tracked (config.toml, setup.sh, docker-compose,
    // etc. live in main's tree), so a whole-dir symlink would skip when
    // git checks out those tracked files. Instead, ensure .aida/ exists
    // and symlink only the gitignored runtime subdirs into it.
    // trace:BUG-52 | ai:claude
    #[cfg(unix)]
    {
        let store_src = project_root.join(".aida-store");
        let store_dst = worktree_path.join(".aida-store");
        if store_src.exists() && !store_dst.exists() {
            std::os::unix::fs::symlink(&store_src, &store_dst).with_context(|| {
                format!(
                    "symlink {} -> {} failed",
                    store_dst.display(),
                    store_src.display()
                )
            })?;
        }

        let parent_aida = project_root.join(".aida");
        let worktree_aida = worktree_path.join(".aida");
        if parent_aida.exists() {
            std::fs::create_dir_all(&worktree_aida)?;
            for runtime in &[
                "sessions",
                "roles",
                "cache.db",
                "cache.db-shm",
                "cache.db-wal",
                "pgdata",
            ] {
                let src = parent_aida.join(runtime);
                let dst = worktree_aida.join(runtime);
                if src.exists() && !dst.exists() {
                    std::os::unix::fs::symlink(&src, &dst).with_context(|| {
                        format!(
                            "symlink {} -> {} failed",
                            dst.display(),
                            src.display()
                        )
                    })?;
                }
            }
        }
    }

    // STORY-52: share parent's cargo target/ with the new worktree so the
    // first `cargo build` inside the session reuses the existing build
    // cache instead of rebuilding from scratch (~2min for aida-cli). We
    // detect a parent target/, write it into the lease, and drop a
    // `.aida/session-env.sh` shim the user sources after `cd`.
    // trace:STORY-52 | ai:claude
    let cargo_target_dir = detect_cargo_target_dir(&project_root);
    if let Some(target) = &cargo_target_dir {
        write_session_env_file(&worktree_path, target).with_context(|| {
            format!(
                "writing session env shim under {}",
                worktree_path.display()
            )
        })?;
    }

    // Compose lease.
    let id_long = uuid::Uuid::now_v7().to_string();
    let id = id_long.replace('-', "")[..12].to_string();
    let owner = aida_core::git_ops::git_config_get("user.email")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_string());
    let lease = SessionLease {
        id: id.clone(),
        scope: owns.to_string(),
        slug: slug.clone(),
        owner,
        worktree_path: worktree_path
            .canonicalize()
            .unwrap_or_else(|_| worktree_path.clone()),
        branch: branch_name.clone(),
        started_at: chrono::Utc::now(),
        hostname: hostname(),
        cargo_target_dir: cargo_target_dir.clone(),
        // STORY-58: record the parent project root so `aida session list`
        // run from inside the new worktree can also walk the parent's
        // Claude Code session storage and present a merged view.
        // trace:STORY-58 | ai:claude
        parent_project_root: Some(
            project_root
                .canonicalize()
                .unwrap_or_else(|_| project_root.clone()),
        ),
        // STORY-71: PR/MR head/base metadata when this is a review session.
        // trace:STORY-71 | ai:claude
        pr_head_sha: pr_metadata.head_sha.clone(),
        pr_base_sha: pr_metadata.base_sha.clone(),
        pr_base_ref: pr_metadata.base_ref.clone(),
    };
    let lease_file = lease_path(&project_root, &id);
    std::fs::write(&lease_file, toml::to_string_pretty(&lease)?)?;

    println!(
        "{} session {} started",
        "✓".green().bold(),
        id.yellow()
    );
    println!("  {}: {}", "scope".bold(), owns.cyan());
    println!("  {}: {}", "branch".bold(), branch_name.cyan());
    println!(
        "  {}: {}",
        "worktree".bold(),
        worktree_path.display().to_string().cyan()
    );
    // STORY-71: surface captured PR head/base in the summary so the user
    // sees what the reviewer flow recorded (matches `aida session show`).
    // trace:STORY-71 | ai:claude
    if let Some(head) = pr_metadata.head_sha.as_deref() {
        let head_short = &head[..head.len().min(12)];
        let base_disp = match (
            pr_metadata.base_ref.as_deref(),
            pr_metadata.base_sha.as_deref(),
        ) {
            (Some(r), Some(b)) => format!("{} ({})", r, &b[..b.len().min(12)]),
            (Some(r), None) => r.to_string(),
            (None, Some(b)) => b[..b.len().min(12)].to_string(),
            (None, None) => "-".to_string(),
        };
        println!("  {}: {}", "pr-head".bold(), head_short.cyan());
        println!("  {}: {}", "pr-base".bold(), base_disp.cyan());
    }
    println!("  {}: {}", "lease".bold(), lease_file.display().to_string().dimmed());

    if launch_claude {
        // STORY-54: --launch collapses "start → cd → session new" into one
        // command. We chdir into the new worktree (so claude inherits it
        // and the launch-log records the worktree path, not the parent),
        // then delegate to session::new_session for the title prompt,
        // launch-log append, and `exec claude --permission-mode <mode>`.
        // exec replaces this process; control doesn't return here on
        // success.
        // trace:STORY-54 | ai:claude
        if cargo_target_dir.is_some() {
            println!();
            println!(
                "{} {}",
                "ℹ".cyan(),
                "tip: source .aida/session-env.sh in your shell to share \
                 the parent's cargo target/ between sessions"
                    .dimmed()
            );
        }
        println!();
        println!(
            "{} {}",
            "▶".green().bold(),
            format!(
                "launching claude in {} (permission-mode {})",
                worktree_path.display(),
                launch_permission_mode
            )
            .cyan()
        );
        std::env::set_current_dir(&worktree_path).with_context(|| {
            format!("failed to chdir into {}", worktree_path.display())
        })?;
        return session::new_session(launch_title, launch_permission_mode, launch_role);
    }

    println!();
    println!("Next:");
    println!(
        "  {}",
        format!("cd {}", worktree_path.display()).cyan()
    );
    if cargo_target_dir.is_some() {
        println!(
            "  {}    {}",
            "source .aida/session-env.sh".cyan(),
            "# share parent's cargo target/".dimmed()
        );
    }
    println!(
        "  {}",
        format!("aida session end {}    # when done", &id[..8]).dimmed()
    );
    Ok(())
}

/// STORY-52: locate the parent project's cargo `target/` directory so a
/// session worktree can reuse its build cache. Returns the canonicalized
/// path when `target/` exists (Rust project that has been built), `None`
/// otherwise. Pure-function over the filesystem so callers can test the
/// session-start flow with a temp dir.
/// trace:STORY-52 | ai:claude
fn detect_cargo_target_dir(project_root: &std::path::Path) -> Option<std::path::PathBuf> {
    let target = project_root.join("target");
    if !target.is_dir() {
        return None;
    }
    Some(target.canonicalize().unwrap_or(target))
}

/// STORY-52: write the worktree-local `.aida/session-env.sh` that the user
/// sources after `cd`-ing into the session worktree. Sourcing it sets
/// `CARGO_TARGET_DIR` to the parent's `target/` so cargo reuses that build
/// cache instead of rebuilding from scratch. The file is written into the
/// worktree's `.aida/` (created here if it doesn't already exist), which
/// lives alongside the symlinked runtime subdirs (sessions/, roles/,
/// cache.db, etc.) that `session_start` set up moments earlier.
/// trace:STORY-52 | ai:claude
fn write_session_env_file(
    worktree_path: &std::path::Path,
    cargo_target_dir: &std::path::Path,
) -> Result<()> {
    let aida_dir = worktree_path.join(".aida");
    std::fs::create_dir_all(&aida_dir)?;
    let env_path = aida_dir.join("session-env.sh");
    let body = render_session_env_file(cargo_target_dir);
    std::fs::write(&env_path, body)?;
    Ok(())
}

/// STORY-52: build the body of `.aida/session-env.sh`. Split out so unit
/// tests can assert the export shape without touching the filesystem.
/// trace:STORY-52 | ai:claude
fn render_session_env_file(cargo_target_dir: &std::path::Path) -> String {
    format!(
        "# Generated by `aida session start` — source after cd-ing into\n\
         # this worktree to share the parent project's cargo build cache.\n\
         # trace:STORY-52 | ai:claude\n\
         export CARGO_TARGET_DIR={}\n",
        shell_single_quote(&cargo_target_dir.display().to_string())
    )
}

/// Wrap a string in POSIX single quotes for safe inclusion in shell source.
/// `'` inside the value is escaped via the standard `'\''` close-reopen trick.
/// trace:STORY-52 | ai:claude
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn session_end(id_query: Option<&str>, yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let leases = list_leases(&project_root);
    if leases.is_empty() {
        println!("(no active sessions)");
        return Ok(());
    }

    // Resolve the lease to end. If no id given, match by cwd.
    let cwd = std::env::current_dir().ok();
    let target = match id_query {
        Some(q) => {
            let q = q.to_lowercase();
            let matches: Vec<&SessionLease> = leases
                .iter()
                .filter(|l| l.id.to_lowercase().starts_with(&q))
                .collect();
            match matches.len() {
                0 => anyhow::bail!("no session matching `{}`", q),
                1 => matches[0].clone(),
                n => anyhow::bail!(
                    "ambiguous session id `{}` — matches {} sessions, use a longer prefix",
                    q,
                    n
                ),
            }
        }
        None => {
            let Some(cwd) = cwd else {
                anyhow::bail!("cwd unavailable; pass an explicit session id");
            };
            let canon_cwd = cwd.canonicalize().unwrap_or(cwd);
            leases
                .iter()
                .find(|l| {
                    let p = &l.worktree_path;
                    p == &canon_cwd
                        || canon_cwd.starts_with(p)
                })
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no session lease covers cwd ({}). Pass an id explicitly.",
                        canon_cwd.display()
                    )
                })?
        }
    };

    println!(
        "About to end session {}:",
        target.id.yellow()
    );
    println!("  scope:    {}", target.scope);
    println!("  branch:   {}", target.branch);
    println!("  worktree: {}", target.worktree_path.display());
    println!();
    println!("Effects:");
    println!(
        "  - delete lease at {}",
        lease_path(&project_root, &target.id).display()
    );
    println!(
        "  - run `git worktree remove {}` (branch {} kept; merge/discard manually)",
        target.worktree_path.display(),
        target.branch
    );

    if !yes {
        use std::io::Write;
        print!("\nContinue? [y/N] ");
        std::io::stdout().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // STORY-56: flatten the session's activity log into each
    // participating role's project-level activity stream BEFORE we delete
    // the lease — once the lease is gone the activity file is orphaned.
    // For each role, take the newest entry per spec_id from the session
    // log, merge into the project role's `activity` (newest first, dedupe
    // by spec_id, truncate to ACTIVITY_MAX) so post-session views like
    // `aida role show` still surface what was worked on under the closed
    // session. trace:STORY-56 | ai:claude
    aggregate_session_activity_into_roles(&project_root, &target.id);
    let activity_file = session_activity_path(&project_root, &target.id);
    let canonical_activity = activity_file.canonicalize().ok();
    let activity_target: std::path::PathBuf = canonical_activity
        .clone()
        .unwrap_or_else(|| activity_file.clone());

    // Snapshot the lease file's authoritative on-disk location BEFORE we
    // touch the worktree's symlinks. When session_end runs from inside the
    // worktree (the natural flow), `project_root` IS the worktree path —
    // and `<worktree>/.aida/sessions/<id>.toml` traverses the symlink
    // installed at session_start to reach the parent's real file. Once we
    // strip that symlink (or git removes the whole worktree), the
    // worktree-relative path stops resolving. canonicalize() follows the
    // symlink now and gives us a stable target for the unlink later.
    // trace:BUG-56 | ai:claude
    let lease_file_via_symlink = lease_path(&project_root, &target.id);
    let canonical_lease = lease_file_via_symlink.canonicalize().ok();

    // Clean the symlinks we created at session start before git tries to
    // remove the worktree — they count as untracked files and `git worktree
    // remove` would otherwise refuse without --force.
    //   - .aida-store/ is a whole-directory symlink (top-level)
    //   - .aida/ itself is a real dir (with tracked content) — leave it
    //     alone, but strip the runtime symlinks inside it that
    //     session_start created. trace:BUG-52 | ai:claude
    let store_link = target.worktree_path.join(".aida-store");
    if store_link.is_symlink() {
        let _ = std::fs::remove_file(&store_link);
    }
    let aida_dir = target.worktree_path.join(".aida");
    for runtime in &[
        "sessions",
        "roles",
        "cache.db",
        "cache.db-shm",
        "cache.db-wal",
        "pgdata",
    ] {
        let p = aida_dir.join(runtime);
        if p.is_symlink() {
            let _ = std::fs::remove_file(&p);
        }
    }

    // Delete the lease file BEFORE removing the worktree. The canonical
    // path resolved above points at the parent's sessions dir, so the
    // unlink succeeds regardless of whether the worktree's symlink chain
    // is still intact. Reporting is honest: success printed only on
    // actual success; missing file is a quiet no-op (already-released
    // lease from a previous partial run is fine); other errors warn so
    // the user can clean up manually instead of trusting a stale "✓".
    // trace:BUG-56 | ai:claude
    let lease_target: &std::path::Path = canonical_lease
        .as_deref()
        .unwrap_or(&lease_file_via_symlink);
    match std::fs::remove_file(lease_target) {
        Ok(_) => println!("{} lease deleted", "✓".green()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Already gone — keep quiet.
        }
        Err(e) => {
            eprintln!(
                "{} could not delete lease at {}: {} — remove manually",
                "Warning:".yellow().bold(),
                lease_target.display(),
                e
            );
        }
    }

    // STORY-56: drop the session activity log. Quiet on missing — short
    // sessions that never recorded any activity won't have one. We've
    // already aggregated entries into the project-level role(s) above.
    // trace:STORY-56 | ai:claude
    if activity_target.exists() {
        match std::fs::remove_file(&activity_target) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!(
                    "{} could not delete session activity log at {}: {}",
                    "Warning:".yellow().bold(),
                    activity_target.display(),
                    e
                );
            }
        }
    }

    let res = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args([
            "worktree",
            "remove",
            target.worktree_path.to_str().unwrap_or_default(),
        ])
        .status();
    match res {
        Ok(s) if s.success() => {
            println!("{} worktree removed", "✓".green());
        }
        _ => {
            eprintln!(
                "{} `git worktree remove` failed; you may need to run it manually with --force",
                "Warning:".yellow().bold()
            );
        }
    }
    println!(
        "  branch {} retained — merge or `git branch -D {}` when ready",
        target.branch.cyan(),
        target.branch
    );

    // STORY-66: when the just-ended session's branch has an open PR, file a
    // Story-typed review item routed to the `reviewer` role with `implements`
    // relations to every spec referenced in the PR's commit messages. Best
    // effort: any failure here logs a warning and is otherwise silent — we
    // never fail `session_end` because of a queue side-effect.
    // trace:STORY-66 | ai:claude
    if let Some(summary) =
        try_auto_queue_pr_review(&project_root, &target.branch, &target.id)
    {
        println!("{} {}", "✓".green(), summary);
    }

    Ok(())
}

/// Open-PR metadata captured by `gh pr list` for a session's branch. Just
/// the fields the auto-queue side-effect needs to brief a reviewer.
/// trace:STORY-66 | ai:claude
struct OpenPrInfo {
    number: u64,
    title: String,
    url: String,
}

/// Look up a single open PR for `branch` via `gh`. Returns None when `gh`
/// isn't installed, the user isn't authenticated, the branch has no open
/// PR, or the output can't be parsed. All paths are best-effort —
/// session_end never fails because of this hook. trace:STORY-66 | ai:claude
fn detect_open_pr_for_branch(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<OpenPrInfo> {
    let out = std::process::Command::new("gh")
        .current_dir(project_root)
        .args([
            "pr", "list",
            "--head", branch,
            "--state", "open",
            "--limit", "1",
            "--json", "number,title,url",
            "-q", r#".[] | "\(.number)\t\(.title)\t\(.url)""#,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 3 {
        return None;
    }
    let number: u64 = parts[0].parse().ok()?;
    Some(OpenPrInfo {
        number,
        title: parts[1].to_string(),
        url: parts[2].to_string(),
    })
}

/// Detect whether a `Review PR-<n>:` story already exists in the local
/// store, so calling `aida session end` twice on the same branch doesn't
/// create duplicate queue entries. trace:STORY-66 | ai:claude
fn pr_review_story_already_exists(
    project_root: &std::path::Path,
    pr_number: u64,
) -> bool {
    let aida = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("aida"));
    let Ok(out) = std::process::Command::new(&aida)
        .current_dir(project_root)
        .args(["list", "--type", "story"])
        .output()
    else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let needle = format!("Review PR-{}:", pr_number);
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line.contains(&needle))
}

/// Strip ANSI SGR sequences (`ESC[...m`) so we can match output text
/// regardless of whether the child invocation colored its output. Keep it
/// minimal — we only need the SGR shape `aida add` emits.
/// trace:STORY-66 | ai:claude
fn strip_ansi_color(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for cc in chars.by_ref() {
                if cc.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Self-invoke `aida add` for a Story-typed review requirement and parse the
/// resulting spec id out of stdout. Returns None on any failure.
/// trace:STORY-66 | ai:claude
fn aida_subcmd_add_review_story(
    project_root: &std::path::Path,
    title: &str,
    description: &str,
) -> Option<String> {
    let aida = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("aida"));
    let out = std::process::Command::new(&aida)
        .current_dir(project_root)
        .args([
            "add",
            "--type", "story",
            "--status", "approved",
            "--priority", "medium",
            "--title", title,
            "--description", description,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "{} auto-queue review story: `aida add` failed: {}",
            "Warning:".yellow().bold(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_spec_id_from_add_output(&stdout)
}

/// Parse the spec id printed by `aida add`. The git-canonical path prints
/// `Added: STORY-N - <title>` (one line); the legacy YAML/SQLite path
/// prints a standalone `ID: STORY-N` line. Accept either so this hook keeps
/// working across backends. trace:STORY-66 | ai:claude
fn parse_spec_id_from_add_output(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let cleaned = strip_ansi_color(line);
        let candidate = cleaned
            .strip_prefix("Added: ")
            .map(|rest| rest.split(" - ").next().unwrap_or("").trim().to_string())
            .or_else(|| {
                cleaned
                    .strip_prefix("ID: ")
                    .map(|rest| rest.trim().to_string())
            });
        if let Some(c) = candidate {
            if !c.is_empty() {
                return Some(c);
            }
        }
    }
    None
}

/// Best-effort `aida rel add <from> <to> --type implements` (and the
/// matching reverse `implemented-by` so `aida show <spec>` surfaces the
/// review story). Custom relation types don't have an `inverse()` mapping
/// in core, so `--bidirectional` is a no-op for them — we add both
/// directions manually instead. Logs and continues on failure.
/// trace:STORY-66 | ai:claude
fn aida_subcmd_rel_add_implements(
    project_root: &std::path::Path,
    from: &str,
    to: &str,
) {
    aida_subcmd_rel_add(project_root, from, to, "implements");
    aida_subcmd_rel_add(project_root, to, from, "implemented-by");
}

fn aida_subcmd_rel_add(
    project_root: &std::path::Path,
    from: &str,
    to: &str,
    rel_type: &str,
) {
    let aida = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("aida"));
    match std::process::Command::new(&aida)
        .current_dir(project_root)
        .args(["rel", "add", from, to, "--type", rel_type])
        .output()
    {
        Ok(o) if o.status.success() => {}
        Ok(o) => eprintln!(
            "{} auto-queue: `rel add {} {} --type {}` failed: {}",
            "Warning:".yellow().bold(),
            from,
            to,
            rel_type,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => eprintln!(
            "{} auto-queue: could not invoke `aida rel add`: {}",
            "Warning:".yellow().bold(),
            e
        ),
    }
}

/// Best-effort `aida queue add <id> --for reviewer --no-scope --note <...>`.
/// trace:STORY-66 | ai:claude
fn aida_subcmd_queue_add_for_reviewer(
    project_root: &std::path::Path,
    spec_id: &str,
    note: &str,
) {
    let aida = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("aida"));
    let out = std::process::Command::new(&aida)
        .current_dir(project_root)
        .args([
            "queue", "add", spec_id,
            "--for", "reviewer",
            "--no-scope",
            "--note", note,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {}
        Ok(o) => eprintln!(
            "{} auto-queue: `queue add {}` failed: {}",
            "Warning:".yellow().bold(),
            spec_id,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => eprintln!(
            "{} auto-queue: could not invoke `aida queue add`: {}",
            "Warning:".yellow().bold(),
            e
        ),
    }
}

/// End-of-session auto-detect-and-queue. Runs as a side effect of
/// `aida session end` so a forgotten `gh pr create` doesn't leave the
/// reviewer unaware. Returns a one-line summary string when it filed (or
/// already-skipped) something; None when there's no PR to act on.
/// trace:STORY-66 | ai:claude
fn try_auto_queue_pr_review(
    project_root: &std::path::Path,
    branch: &str,
    session_id: &str,
) -> Option<String> {
    let pr = detect_open_pr_for_branch(project_root, branch)?;
    if pr_review_story_already_exists(project_root, pr.number) {
        return Some(format!(
            "PR #{} already has a `Review PR-{}` story queued — skipping",
            pr.number, pr.number
        ));
    }

    // Pull the commit-range spec ids using the existing helpers from STORY-67.
    let (base, head) = pr_base_head(project_root, ReviewForge::GitHub, pr.number)
        .unwrap_or_else(|_| ("main".to_string(), branch.to_string()));
    let messages = git_log_messages(project_root, &base, &head).unwrap_or_default();
    let mut spec_ids: Vec<String> = Vec::new();
    for msg in &messages {
        for id in extract_spec_ids_from_commit(msg) {
            if !spec_ids.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
                spec_ids.push(id);
            }
        }
    }

    let session_short: &str = &session_id[..session_id.len().min(8)];
    let mut desc = String::new();
    desc.push_str(&format!(
        "Auto-filed by `aida session end` (session `{}`) when its branch `{}` had an open PR.\n\n",
        session_short, branch
    ));
    desc.push_str(&format!("- PR: <{}>\n", pr.url));
    desc.push_str(&format!("- Branch: `{}` → `{}`\n\n", head, base));
    if spec_ids.is_empty() {
        desc.push_str(
            "No `(REQ-ID)` trailers were found in the PR's commit range — review against the PR title/body and link specs after the fact via `aida rel add <this> <spec> --type implements`.\n\n",
        );
    } else {
        desc.push_str("## Covers\n\n");
        for id in &spec_ids {
            desc.push_str(&format!("- {}\n", id));
        }
        desc.push('\n');
    }
    desc.push_str("## Acceptance\n\n");
    desc.push_str(&format!(
        "- Generate a structured review prompt with `aida review prompt --pr {}` and verify each spec's acceptance criteria.\n",
        pr.number
    ));
    desc.push_str("- Approve and merge, or request changes by spec id.\n");
    desc.push_str("- Mark this story `completed` once the PR is merged.\n");

    let title = format!("Review PR-{}: {}", pr.number, pr.title);
    let new_id = aida_subcmd_add_review_story(project_root, &title, &desc)?;

    for id in &spec_ids {
        aida_subcmd_rel_add_implements(project_root, &new_id, id);
    }

    let note = format!(
        "auto-queued by `aida session end` ({}); covers {} spec{}",
        session_short,
        spec_ids.len(),
        if spec_ids.len() == 1 { "" } else { "s" }
    );
    aida_subcmd_queue_add_for_reviewer(project_root, &new_id, &note);

    let covers = if spec_ids.is_empty() {
        "no specs".to_string()
    } else {
        spec_ids.join(", ")
    };
    Some(format!(
        "filed {} (covers {}) → reviewer queue (PR #{})",
        new_id, covers, pr.number
    ))
}

fn session_leases() -> Result<()> {
    let project_root = find_project_root()?;
    let leases = list_leases(&project_root);
    if leases.is_empty() {
        println!("(no active sessions)");
        println!();
        println!(
            "Start one with: {} {}",
            "aida session start --owns".cyan(),
            "<scope>".dimmed()
        );
        return Ok(());
    }
    println!("{}", "Active session leases".bold());
    println!();
    println!(
        "{:<14} {:<24} {:<22} {}",
        "id", "scope", "branch", "worktree"
    );
    println!("{}", "─".repeat(96));
    for l in &leases {
        println!(
            "{:<14} {:<24} {:<22} {}",
            (&l.id[..8]).yellow(),
            truncate(&l.scope, 24),
            truncate(&l.branch, 22),
            l.worktree_path.display()
        );
    }
    println!();
    println!(
        "End one with: {} {}",
        "aida session end".cyan(),
        "<id>".dimmed()
    );
    Ok(())
}

/// One `.jsonl` file old enough to prune. Captured up-front so the
/// confirmation list and the deletion loop see the same set.
/// trace:STORY-60 | ai:claude
struct PruneCandidate {
    path: std::path::PathBuf,
    size: u64,
    age_seconds: u64,
}

/// Format a byte count with a human-readable unit (KB/MB/GB). One
/// decimal place so a 1.5 MB file doesn't read as "1 MB" or "1500000 B".
/// trace:STORY-60 | ai:claude
fn humanize_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{} B", bytes)
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else if b < GB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.1} GB", b / GB)
    }
}

/// Compute the new queue position for `aida queue move X --after Y`.
/// `anchor` is Y's current position; `successor` is the position of the
/// next entry strictly after Y, or `None` when Y is at the bottom.
///
/// Returns a position that sorts strictly between `anchor` and the next
/// entry. Midpoint math when there's room; +1 fallback when adjacent
/// (collision risk acknowledged — safe in practice because positions are
/// initially gapped by 1000); +1000 when Y is at the bottom. All math is
/// saturating so a corrupt-state queue (every item at `i64::MAX` from a
/// pre-fix queue_add, see git_backend.rs) doesn't panic on overflow —
/// the callsite is expected to surface a friendlier hint instead.
/// trace:STORY-72 | ai:claude
fn position_after(anchor: i64, successor: Option<i64>) -> i64 {
    match successor {
        Some(np) if np > anchor.saturating_add(1) => {
            anchor.saturating_add((np.saturating_sub(anchor)) / 2)
        }
        Some(_) => anchor.saturating_add(1),
        None => anchor.saturating_add(1000),
    }
}

fn humanize_age_secs(secs: u64) -> String {
    if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 7 * 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs < 30 * 86_400 {
        format!("{}w", secs / (7 * 86_400))
    } else if secs < 365 * 86_400 {
        format!("{}mo", secs / (30 * 86_400))
    } else {
        format!("{}y", secs / (365 * 86_400))
    }
}

/// `aida session prune` — walk Claude Code's per-project session
/// directories under `~/.claude/projects/<encoded>/` and delete `.jsonl`
/// files whose mtime is older than `days`. Skips any project dir
/// corresponding to an active session lease so a long-running session
/// doesn't self-delete from a forgotten cron-like usage. Logs each
/// deletion to `<project>/.aida/session-prune.log` for auditing.
/// trace:STORY-60 | ai:claude
fn session_prune(days: u32, dry_run: bool, yes: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("could not determine cwd")?;
    let project_root = find_project_root().unwrap_or_else(|_| cwd.clone());

    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(u64::from(days) * 86_400))
        .ok_or_else(|| anyhow::anyhow!("--days {} is too large", days))?;
    let now = std::time::SystemTime::now();

    // Project dirs to walk: the current cwd's encoded dir, plus the parent
    // project's (when run from inside a session worktree). De-dup since
    // they collapse to the same dir for plain non-worktree usage.
    let mut search_dirs: Vec<std::path::PathBuf> = Vec::new();
    let push_dir = |dirs: &mut Vec<std::path::PathBuf>, p: std::path::PathBuf| {
        if p.is_dir() && !dirs.iter().any(|d| d == &p) {
            dirs.push(p);
        }
    };
    if let Ok(d) = session::claude_project_dir(&cwd) {
        push_dir(&mut search_dirs, d);
    }
    if let Some(parent) = parent_project_root_for_session(&cwd) {
        if let Ok(d) = session::claude_project_dir(&parent) {
            push_dir(&mut search_dirs, d);
        }
    }
    // Also include encoded dirs for every active lease's worktree path —
    // they often live OUTSIDE the parent project root (sibling worktrees),
    // so the parent-walk doesn't catch them. We'll skip these in the
    // candidate loop (see active_dirs below) but we still need to know
    // about them for the "skipped active" tally. trace:STORY-60 | ai:claude
    let active_leases = list_leases(&project_root);
    let mut active_dirs: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    for l in &active_leases {
        if let Ok(d) = session::claude_project_dir(&l.worktree_path) {
            active_dirs.insert(d.clone());
            push_dir(&mut search_dirs, d);
        }
    }

    if search_dirs.is_empty() {
        println!("(no Claude Code session directories found for this project)");
        return Ok(());
    }

    let mut candidates: Vec<PruneCandidate> = Vec::new();
    let mut skipped_active = 0usize;
    for dir in &search_dirs {
        if active_dirs.contains(dir) {
            // Defensive: anything in here belongs to an in-progress
            // session, regardless of mtime. Skip wholesale.
            // trace:STORY-60 | ai:claude
            if let Ok(read) = std::fs::read_dir(dir) {
                skipped_active += read
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
                    .count();
            }
            continue;
        }
        let Ok(read) = std::fs::read_dir(dir) else { continue };
        for entry in read.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else { continue };
            let Ok(mtime) = metadata.modified() else { continue };
            if mtime >= cutoff {
                continue;
            }
            let age_seconds = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);
            candidates.push(PruneCandidate {
                path,
                size: metadata.len(),
                age_seconds,
            });
        }
    }

    if candidates.is_empty() {
        println!(
            "(no .jsonl files older than {} day{} in {} session director{})",
            days,
            if days == 1 { "" } else { "s" },
            search_dirs.len(),
            if search_dirs.len() == 1 { "y" } else { "ies" },
        );
        if skipped_active > 0 {
            println!(
                "  ({} file{} skipped — belong to {} active session{})",
                skipped_active,
                if skipped_active == 1 { "" } else { "s" },
                active_dirs.len(),
                if active_dirs.len() == 1 { "" } else { "s" },
            );
        }
        return Ok(());
    }

    // Sort oldest-first so the user sees the most-stale entries up top.
    candidates.sort_by(|a, b| b.age_seconds.cmp(&a.age_seconds));

    let total_size: u64 = candidates.iter().map(|c| c.size).sum();
    let count = candidates.len();
    println!(
        "Found {} .jsonl file{} older than {} day{} ({} total):",
        count,
        if count == 1 { "" } else { "s" },
        days,
        if days == 1 { "" } else { "s" },
        humanize_size(total_size)
    );
    println!();
    for c in &candidates {
        let id = c
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?");
        let id_short = &id[..id.len().min(8)];
        println!(
            "  {:>6}  {:>10}  {}  {}",
            humanize_age_secs(c.age_seconds),
            humanize_size(c.size),
            id_short.dimmed(),
            c.path.display()
        );
    }
    println!();
    if skipped_active > 0 {
        println!(
            "{} {} file{} from {} active session{} excluded.",
            "Note:".yellow().bold(),
            skipped_active,
            if skipped_active == 1 { "" } else { "s" },
            active_dirs.len(),
            if active_dirs.len() == 1 { "" } else { "s" },
        );
    }

    if dry_run {
        println!("{}", "(--dry-run; nothing deleted)".dimmed());
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        print!("Delete {} file{}? [y/N] ", count, if count == 1 { "" } else { "s" });
        std::io::stdout().flush().ok();
        let mut ans = String::new();
        if std::io::stdin().read_line(&mut ans).is_err()
            || !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes")
        {
            println!("Aborted.");
            return Ok(());
        }
    }

    let log_path = project_root.join(".aida").join("session-prune.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut log_handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    let mut deleted = 0usize;
    let mut bytes_freed = 0u64;
    let mut errors = 0usize;
    let now_iso = chrono::Utc::now().to_rfc3339();
    for c in &candidates {
        match std::fs::remove_file(&c.path) {
            Ok(_) => {
                deleted += 1;
                bytes_freed += c.size;
                if let Some(f) = log_handle.as_mut() {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "{}\t{}\t{}\t{}",
                        now_iso,
                        c.size,
                        c.age_seconds,
                        c.path.display()
                    );
                }
            }
            Err(e) => {
                errors += 1;
                eprintln!(
                    "{} could not delete {}: {}",
                    "Warning:".yellow().bold(),
                    c.path.display(),
                    e
                );
            }
        }
    }

    println!(
        "{} Deleted {} file{} ({} freed){}",
        "✓".green(),
        deleted,
        if deleted == 1 { "" } else { "s" },
        humanize_size(bytes_freed),
        if errors > 0 {
            format!("; {} error{}", errors, if errors == 1 { "" } else { "s" })
        } else {
            String::new()
        }
    );
    if log_handle.is_some() {
        println!("  log: {}", log_path.display().to_string().dimmed());
    }
    Ok(())
}

/// True when one SHA is a (case-insensitive, hex) prefix of the other.
/// Used by statusline to compare a cache-stored SHA (potentially full,
/// 40 chars) against the current `git rev-parse --short HEAD` output (7
/// chars) without flagging a spurious stale.
/// trace:TASK-1-045 | ai:claude
fn sha_prefix_match(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return a == b;
    }
    let min = a.len().min(b.len());
    a[..min].eq_ignore_ascii_case(&b[..min])
}

#[cfg(test)]
mod statusline_tests {
    use super::*;

    /// Cache-stored full SHA matches `--short` output. trace:TASK-1-045
    #[test]
    fn sha_prefix_match_full_vs_short() {
        let full = "4e39de29ddd72417772aa14b552937018d270746";
        let short = "4e39de2";
        assert!(sha_prefix_match(full, short));
        assert!(sha_prefix_match(short, full));
    }

    #[test]
    fn sha_prefix_match_different_shas() {
        assert!(!sha_prefix_match("4e39de2", "deadbee"));
        assert!(!sha_prefix_match("4e39de29ddd724", "4e3a000000"));
    }

    #[test]
    fn sha_prefix_match_case_insensitive() {
        assert!(sha_prefix_match("4E39DE2", "4e39de29ddd724"));
    }

    #[test]
    fn sha_prefix_match_empty_strings() {
        assert!(sha_prefix_match("", ""));
        assert!(!sha_prefix_match("", "abc"));
        assert!(!sha_prefix_match("abc", ""));
    }

    /// STORY-66: the auto-queue hook parses `aida add`'s spec_id out of a
    /// line that may be wrapped in colored() output. Drop SGR sequences
    /// without depending on a regex / ansi crate.
    /// trace:STORY-66 | ai:claude
    #[test]
    fn strip_ansi_color_basic() {
        assert_eq!(strip_ansi_color("plain"), "plain");
        assert_eq!(strip_ansi_color("\x1b[32mSTORY-77\x1b[0m"), "STORY-77");
        assert_eq!(
            strip_ansi_color("\x1b[1;32mbold green\x1b[0m and tail"),
            "bold green and tail"
        );
        // Lone ESC without `[` survives — we only strip well-formed SGR.
        assert_eq!(strip_ansi_color("a\x1bb"), "a\x1bb");
    }

    /// STORY-60: byte-count formatter — boundary cases at KB/MB/GB
    /// crossover. Below 1KB renders as bytes; otherwise one decimal.
    /// trace:STORY-60 | ai:claude
    #[test]
    fn humanize_size_buckets() {
        assert_eq!(humanize_size(0), "0 B");
        assert_eq!(humanize_size(512), "512 B");
        assert_eq!(humanize_size(1024), "1.0 KB");
        assert_eq!(humanize_size(1536), "1.5 KB");
        assert_eq!(humanize_size(1024 * 1024), "1.0 MB");
        assert_eq!(humanize_size(1024 * 1024 * 3 / 2), "1.5 MB");
        assert_eq!(humanize_size(1024 * 1024 * 1024), "1.0 GB");
    }

    /// BUG-64: terminal-status predicate. Completed and Rejected are
    /// terminal; everything else is open and accepts new children.
    /// trace:BUG-64 | ai:claude
    #[test]
    fn is_terminal_status_buckets() {
        assert!(is_terminal_status(&RequirementStatus::Completed));
        assert!(is_terminal_status(&RequirementStatus::Rejected));
        assert!(!is_terminal_status(&RequirementStatus::Draft));
        assert!(!is_terminal_status(&RequirementStatus::Approved));
        assert!(!is_terminal_status(&RequirementStatus::Planned));
        assert!(!is_terminal_status(&RequirementStatus::InProgress));
    }

    /// STORY-72: position math for `queue move --after`. Three regimes —
    /// gapped (typical), adjacent (collision fallback), bottom (no
    /// successor). trace:STORY-72 | ai:claude
    #[test]
    fn position_after_picks_midpoint_when_gapped() {
        // Typical case: anchor + successor with the standard 1000 gap.
        assert_eq!(position_after(0, Some(1000)), 500);
        // Wider gap still midpoints.
        assert_eq!(position_after(2000, Some(4000)), 3000);
        // Anchor at bottom — no successor — uses the +1000 step that
        // matches the existing `--bottom` convention.
        assert_eq!(position_after(7000, None), 8000);
        // Adjacent positions: midpoint would land on the anchor (collision).
        // Fall through to +1 even though it risks colliding with the next
        // entry — the situation only arises in pathologically dense queues.
        assert_eq!(position_after(5, Some(6)), 6);
        assert_eq!(position_after(5, Some(5)), 6);
        // Negative anchor (queue items moved to top via `--top` use
        // negative positions) still produces a sortable midpoint.
        assert_eq!(position_after(-1000, Some(0)), -500);
        // Saturating arithmetic: a pre-fix corrupt queue where every
        // entry has `position: i64::MAX` (see git_backend's queue_add
        // sentinel resolution) must not overflow. The result clamps to
        // i64::MAX rather than wrapping. The user-visible result is a
        // no-op move, which is fine — better than a panic.
        assert_eq!(position_after(i64::MAX, None), i64::MAX);
        assert_eq!(position_after(i64::MAX, Some(i64::MAX)), i64::MAX);
        assert_eq!(position_after(i64::MAX - 1, None), i64::MAX);
    }

    /// STORY-60: age formatter for the prune candidate list. Resolution
    /// drops as we cross day/week/month/year boundaries; sub-day uses
    /// hours since `--days 30` makes anything finer than that
    /// uninteresting (and 0d would be confusing).
    /// trace:STORY-60 | ai:claude
    #[test]
    fn humanize_age_secs_buckets() {
        assert_eq!(humanize_age_secs(3600), "1h");
        assert_eq!(humanize_age_secs(86_399), "23h");
        assert_eq!(humanize_age_secs(86_400), "1d");
        assert_eq!(humanize_age_secs(7 * 86_400 - 1), "6d");
        assert_eq!(humanize_age_secs(7 * 86_400), "1w");
        assert_eq!(humanize_age_secs(30 * 86_400), "1mo");
        assert_eq!(humanize_age_secs(365 * 86_400), "1y");
    }

    /// STORY-66: parse spec_id out of `aida add` stdout. Cover both backend
    /// output shapes — git-canonical (`Added: ID - title`) and legacy
    /// (`ID: spec_id`) — plus colored output and trailing `Hint:` noise.
    /// trace:STORY-66 | ai:claude
    #[test]
    fn parse_spec_id_from_add_output_handles_known_shapes() {
        // Git-canonical default.
        assert_eq!(
            parse_spec_id_from_add_output(
                "Added: STORY-82 - Test STORY-66 auto-queue helper\nHint: link it via …\n"
            ),
            Some("STORY-82".to_string())
        );
        // Legacy YAML/SQLite path.
        assert_eq!(
            parse_spec_id_from_add_output(
                "Requirement added successfully!\nUUID: 019e1300-…\nID: \x1b[32mSTORY-77\x1b[0m\n"
            ),
            Some("STORY-77".to_string())
        );
        // Color-wrapped git-canonical.
        assert_eq!(
            parse_spec_id_from_add_output("Added: \x1b[1;32mSTORY-99\x1b[0m - hello\n"),
            Some("STORY-99".to_string())
        );
        // Output with no recognizable line.
        assert_eq!(
            parse_spec_id_from_add_output("something unrelated\n"),
            None
        );
    }

    /// STORY-55: scope-fallback decision table for the `@<…>` segment.
    /// Captures the four (latest-activity, active-lease) cases the
    /// statusline distinguishes.
    /// trace:STORY-55 | ai:claude
    #[test]
    fn scope_fallback_decision_table() {
        use chrono::TimeZone;
        let lease_started = chrono::Utc.with_ymd_and_hms(2026, 5, 9, 12, 0, 0).unwrap();
        let before_lease = chrono::Utc.with_ymd_and_hms(2026, 5, 9, 11, 0, 0).unwrap();
        let after_lease = chrono::Utc.with_ymd_and_hms(2026, 5, 9, 13, 0, 0).unwrap();

        let pick = |latest_at: Option<chrono::DateTime<chrono::Utc>>,
                    lease: Option<chrono::DateTime<chrono::Utc>>,
                    spec: &str,
                    scope: &str|
         -> Option<String> {
            match (latest_at, lease) {
                (Some(at), Some(started_at)) if at >= started_at => Some(spec.to_string()),
                (_, Some(_)) => Some(scope.to_string()),
                (Some(_), None) => Some(spec.to_string()),
                (None, None) => None,
            }
        };

        // In-session activity wins over scope.
        assert_eq!(
            pick(Some(after_lease), Some(lease_started), "STORY-48", "EPIC-20"),
            Some("STORY-48".into())
        );
        // Pre-session activity is shadowed by the scope.
        assert_eq!(
            pick(Some(before_lease), Some(lease_started), "STORY-54", "EPIC-20"),
            Some("EPIC-20".into())
        );
        // Lease but no activity at all → scope.
        assert_eq!(
            pick(None, Some(lease_started), "", "EPIC-20"),
            Some("EPIC-20".into())
        );
        // No lease, only activity → spec (existing behavior).
        assert_eq!(
            pick(Some(after_lease), None, "STORY-48", ""),
            Some("STORY-48".into())
        );
        // No lease, no activity → nothing.
        assert_eq!(pick(None, None, "", ""), None);
    }

    /// STORY-55: long scopes (e.g. file-path scopes) are truncated to fit
    /// the statusline budget, matching @SPEC's visual width.
    /// trace:STORY-55 | ai:claude
    #[test]
    fn scope_label_truncates_to_budget() {
        let long = "very-long-scope-name-that-overflows";
        let short = truncate(long, SCOPE_LABEL_MAX);
        assert!(short.chars().count() <= SCOPE_LABEL_MAX);
        assert!(short.ends_with('…'));

        let exact = "EPIC-20";
        assert_eq!(truncate(exact, SCOPE_LABEL_MAX), "EPIC-20");
    }

    /// STORY-48: enforcement-mode parsing is forgiving on capitalization
    /// and whitespace, and unknown values fall through to `Warn`.
    /// trace:STORY-48 | ai:claude
    #[test]
    fn session_enforcement_parsing() {
        assert_eq!(SessionEnforcement::from_config_str("off"), SessionEnforcement::Off);
        assert_eq!(SessionEnforcement::from_config_str("OFF"), SessionEnforcement::Off);
        assert_eq!(SessionEnforcement::from_config_str("none"), SessionEnforcement::Off);
        assert_eq!(SessionEnforcement::from_config_str(" warn "), SessionEnforcement::Warn);
        assert_eq!(SessionEnforcement::from_config_str("Warn"), SessionEnforcement::Warn);
        assert_eq!(SessionEnforcement::from_config_str("block"), SessionEnforcement::Block);
        assert_eq!(SessionEnforcement::from_config_str("strict"), SessionEnforcement::Block);
        // Unknown → Warn (the safe default).
        assert_eq!(SessionEnforcement::from_config_str("xyzzy"), SessionEnforcement::Warn);
    }

    /// STORY-53: the `sess:<scope>` segment reuses SCOPE_LABEL_MAX, the
    /// same budget that bounds @SPEC's width — long path-glob scopes get
    /// the trailing ellipsis so the statusline stays scannable, and short
    /// scopes pass through verbatim.
    /// trace:STORY-53 | ai:claude
    #[test]
    fn sess_segment_label_truncation() {
        let short = "EPIC-20";
        assert_eq!(truncate(short, SCOPE_LABEL_MAX), "EPIC-20");

        let long = "feature:auth-flow-rewrite";
        let out = truncate(long, SCOPE_LABEL_MAX);
        assert!(out.chars().count() <= SCOPE_LABEL_MAX);
        assert!(out.ends_with('…'));
    }

    /// STORY-56: appending session activity dedupes consecutive same-(role,
    /// spec_id, action) writes by ticking the timestamp instead of stacking
    /// duplicate entries — same shape as project-level role activity.
    /// trace:STORY-56 | ai:claude
    #[test]
    fn session_activity_dedupes_consecutive_repeats() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();

        let id = "test-session-01";
        append_session_activity(root, id, "implementer", "STORY-56", "edit").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        append_session_activity(root, id, "implementer", "STORY-56", "edit").unwrap();
        let log = load_session_activity(root, id);
        assert_eq!(log.entries.len(), 1, "consecutive same-action collapse");

        // A different action breaks the dedupe.
        append_session_activity(root, id, "implementer", "STORY-56", "show").unwrap();
        let log = load_session_activity(root, id);
        assert_eq!(log.entries.len(), 2);
        assert_eq!(log.entries[0].action, "show", "newest first");
    }

    /// BUG-65: dedupe is LRU-by-(role, spec_id, action), not just
    /// consecutive — interleaved actions across specs still collapse
    /// duplicates. Without this, a long agent run that revisits the same
    /// spec produces an ever-growing log.
    /// trace:BUG-65 | ai:claude
    #[test]
    fn session_activity_dedupes_lru_across_interleaving() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();
        let id = "lru-session-01";

        // Sequence: show A → edit B → show A. The second show A must
        // remove the first one and land at the front (not append a
        // duplicate behind edit B).
        append_session_activity(root, id, "implementer", "STORY-A", "show").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        append_session_activity(root, id, "implementer", "STORY-B", "edit").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        append_session_activity(root, id, "implementer", "STORY-A", "show").unwrap();

        let log = load_session_activity(root, id);
        assert_eq!(log.entries.len(), 2, "duplicate show STORY-A collapsed");
        assert_eq!(log.entries[0].spec_id, "STORY-A", "newest first");
        assert_eq!(log.entries[0].action, "show");
        assert_eq!(log.entries[1].spec_id, "STORY-B");
    }

    /// STORY-70: convention-check predicate flags STORY/BUG with no
    /// acceptance section, accepts STORY/BUG that has one (any of the
    /// recognized headings), and ignores other types entirely. Pins the
    /// scope of the lint so it doesn't grow over-eagerly.
    /// trace:STORY-70 | ai:claude
    #[test]
    fn requirement_missing_acceptance_scope() {
        use aida_core::{Requirement, RequirementType};

        // STORY without acceptance → flagged.
        let mut story = Requirement::new("S".into(), "Just a paragraph.".into());
        story.req_type = RequirementType::Story;
        assert!(requirement_missing_acceptance(&story));

        // STORY with `## Acceptance` → not flagged.
        let mut story_ok = Requirement::new(
            "S".into(),
            "Intro.\n\n## Acceptance\n\n- alpha\n".into(),
        );
        story_ok.req_type = RequirementType::Story;
        assert!(!requirement_missing_acceptance(&story_ok));

        // BUG with `## Verify` (alias) → not flagged.
        let mut bug_ok = Requirement::new(
            "B".into(),
            "Repro.\n\n## Verify\n\n- behaves\n".into(),
        );
        bug_ok.req_type = RequirementType::Bug;
        assert!(!requirement_missing_acceptance(&bug_ok));

        // BUG without acceptance → flagged.
        let mut bug = Requirement::new("B".into(), "Repro only.".into());
        bug.req_type = RequirementType::Bug;
        assert!(requirement_missing_acceptance(&bug));

        // EPIC / TASK / etc. are out of scope — even with no section,
        // they're never flagged.
        let mut epic = Requirement::new("E".into(), "No section.".into());
        epic.req_type = RequirementType::Epic;
        assert!(!requirement_missing_acceptance(&epic));
        let mut task = Requirement::new("T".into(), "No section.".into());
        task.req_type = RequirementType::Task;
        assert!(!requirement_missing_acceptance(&task));
    }

    /// BUG-65 acceptance: shipping 3 specs sequentially via a typical
    /// implementer lifecycle (edit → done) leaves the activity log
    /// pointing at the 3rd, not the 1st. This is the contract the
    /// statusline @SPEC reads off of, so this test pins the regression
    /// that motivated the bug.
    /// trace:BUG-65 | ai:claude
    #[test]
    fn session_activity_three_specs_lifecycle_points_at_last() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();
        let id = "lifecycle-01";

        for spec in ["STORY-1", "STORY-2", "STORY-3"] {
            append_session_activity(root, id, "implementer", spec, "edit").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1));
            append_session_activity(root, id, "implementer", spec, "done").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        let log = load_session_activity(root, id);
        // Newest action is `done` on STORY-3.
        assert_eq!(log.entries[0].spec_id, "STORY-3");
        assert_eq!(log.entries[0].action, "done");
        // Each spec contributes 2 entries (edit, done) — total 6, no dups.
        assert_eq!(log.entries.len(), 6);
    }

    /// STORY-67: extract_acceptance_section finds `## Acceptance`,
    /// `## Verify`, etc. (case-insensitive) and returns the body until
    /// the next `## ` heading. Missing or empty sections return None
    /// so the caller can render a placeholder rather than an empty
    /// section. trace:STORY-67 | ai:claude
    #[test]
    fn extract_acceptance_section_basic() {
        let desc = "Some intro paragraph.\n\n## Acceptance\n\n- alpha\n- bravo\n\n## Notes\n\nFollow-up.\n";
        let body = extract_acceptance_section(desc).unwrap();
        assert_eq!(body, "- alpha\n- bravo");
    }

    /// trace:STORY-67 | ai:claude
    #[test]
    fn extract_acceptance_section_aliases() {
        for heading in &["Acceptance", "Verify", "Test cases", "Tests", "Verification"] {
            let desc = format!("blah\n\n## {}\n\nbody text\n", heading);
            assert!(
                extract_acceptance_section(&desc).is_some(),
                "missing recognition for `## {}`",
                heading
            );
        }
        // Non-matching headings fall through.
        let desc = "Body.\n\n## Why\n\nBecause.\n";
        assert!(extract_acceptance_section(desc).is_none());
    }

    /// trace:STORY-67 | ai:claude
    #[test]
    fn extract_acceptance_section_empty_body() {
        let desc = "## Acceptance\n\n## Why\n\nReason.\n";
        // Empty body (just whitespace until the next heading) → None.
        assert!(extract_acceptance_section(desc).is_none());
    }

    /// STORY-67: spec ID detection inside a `(...)` group at end of
    /// commit subject. Matches AIDA-format SPEC-IDs and rejects
    /// anything else (e.g., issue refs, version strings).
    /// trace:STORY-67 | ai:claude
    #[test]
    fn extract_spec_ids_from_commit_subject() {
        let msg = "[AI:claude] feat(api): add endpoint (FR-1-042)\n\nBody text.\n";
        assert_eq!(
            extract_spec_ids_from_commit(msg),
            vec!["FR-1-042".to_string()]
        );
        let msg = "fix(scope): tweak (BUG-23)\n";
        assert_eq!(
            extract_spec_ids_from_commit(msg),
            vec!["BUG-23".to_string()]
        );
        // Commits with no trailer leave nothing.
        let msg = "chore: bump dep version\n";
        assert!(extract_spec_ids_from_commit(msg).is_empty());
        // Version-like parens shouldn't match.
        let msg = "release: v1.2.3 (1.2.3)\n";
        assert!(extract_spec_ids_from_commit(msg).is_empty());
    }

    /// STORY-67: looks_like_spec_id validates the alpha-DASH-digits
    /// shape used throughout AIDA.
    /// trace:STORY-67 | ai:claude
    #[test]
    fn spec_id_shape_recognition() {
        assert!(looks_like_spec_id("FR-42"));
        assert!(looks_like_spec_id("BUG-1-038"));
        assert!(looks_like_spec_id("EPIC-2"));
        assert!(looks_like_spec_id("STORY-100"));
        // Rejects.
        assert!(!looks_like_spec_id("v1.2.3"));
        assert!(!looks_like_spec_id("1.2"));
        assert!(!looks_like_spec_id("X-"));
        assert!(!looks_like_spec_id("X"));
        assert!(!looks_like_spec_id(""));
        // Lowercase prefix is permitted at this layer; commit subjects
        // typically uppercase, but `(fr-1)` shouldn't blow up if it
        // appears.
        assert!(looks_like_spec_id("fr-1"));
    }

    /// STORY-61: PR-N / MR-N scope parsing — case-insensitive, requires
    /// the trailing number, and rejects everything else (so the normal
    /// scope flow is preserved for non-review scopes like EPIC-20).
    /// trace:STORY-61 | ai:claude
    #[test]
    fn review_scope_parsing() {
        assert_eq!(parse_review_scope("PR-1"), Some((ReviewForge::GitHub, 1)));
        assert_eq!(parse_review_scope("pr-42"), Some((ReviewForge::GitHub, 42)));
        assert_eq!(parse_review_scope("MR-7"), Some((ReviewForge::GitLab, 7)));
        assert_eq!(parse_review_scope("mr-2024"), Some((ReviewForge::GitLab, 2024)));
        // Non-PR scopes pass through unchanged.
        assert_eq!(parse_review_scope("EPIC-20"), None);
        assert_eq!(parse_review_scope("FR-42"), None);
        assert_eq!(parse_review_scope("feature:auth"), None);
        // Missing number rejects.
        assert_eq!(parse_review_scope("PR-"), None);
        assert_eq!(parse_review_scope("MR-abc"), None);
    }

    /// STORY-61: refspec format — same-repo and fork PRs both work
    /// because `pull/N/head` (GitHub) and `merge-requests/N/head`
    /// (GitLab) are populated on origin in both cases.
    /// trace:STORY-61 | ai:claude
    #[test]
    fn review_forge_refspec() {
        assert_eq!(ReviewForge::GitHub.pr_head_ref(1), "pull/1/head");
        assert_eq!(ReviewForge::GitHub.pr_head_ref(123), "pull/123/head");
        assert_eq!(ReviewForge::GitLab.pr_head_ref(7), "merge-requests/7/head");
        assert_eq!(ReviewForge::GitHub.local_branch_for(1), "pr-1");
        assert_eq!(ReviewForge::GitLab.local_branch_for(7), "mr-7");
    }

    /// STORY-61: --forge string parsing accepts both the long form
    /// (`github`/`gitlab`) and the CLI-tool short form (`gh`/`glab`)
    /// since users usually have one or the other muscle-memory'd.
    /// trace:STORY-61 | ai:claude
    #[test]
    fn review_forge_override_parsing() {
        assert_eq!(ReviewForge::parse("github"), Some(ReviewForge::GitHub));
        assert_eq!(ReviewForge::parse("GitHub"), Some(ReviewForge::GitHub));
        assert_eq!(ReviewForge::parse("gh"), Some(ReviewForge::GitHub));
        assert_eq!(ReviewForge::parse("gitlab"), Some(ReviewForge::GitLab));
        assert_eq!(ReviewForge::parse("glab"), Some(ReviewForge::GitLab));
        assert_eq!(ReviewForge::parse("bitbucket"), None);
        assert_eq!(ReviewForge::parse(""), None);
    }

    /// STORY-57: routing-filter decision table for the consumer side.
    /// Entries with `for_scope` only route to sessions whose lease scope
    /// matches; entries with `for_session` only route to that exact lease
    /// (8+ char prefix). No lease + scope-tagged entry → filtered. The
    /// `--all` bypass (the boolean param) lets users see everything.
    /// trace:STORY-57 | ai:claude
    #[test]
    fn entry_scope_session_match_decision_table() {
        use aida_core::QueueEntry;
        let now = chrono::Utc::now();
        let mk = |scope: Option<&str>, sess: Option<&str>| QueueEntry {
            user_id: "u".into(),
            requirement_id: uuid::Uuid::nil(),
            position: 0,
            added_by: "u".into(),
            note: None,
            added_at: now,
            for_role: Some("implementer".into()),
            for_scope: scope.map(|s| s.to_string()),
            for_session: sess.map(|s| s.to_string()),
        };
        let lease = SessionLease {
            id: "abcdef123456".into(),
            scope: "EPIC-20".into(),
            slug: "epic-20".into(),
            owner: "u".into(),
            worktree_path: std::path::PathBuf::from("/tmp/wt"),
            branch: "br".into(),
            started_at: now,
            hostname: "h".into(),
            cargo_target_dir: None,
            parent_project_root: None,
            pr_head_sha: None,
            pr_base_sha: None,
            pr_base_ref: None,
        };

        // No routing tags = visible everywhere.
        assert!(entry_scope_session_match(&mk(None, None), Some(&lease), false));
        assert!(entry_scope_session_match(&mk(None, None), None, false));

        // Scope match passes; mismatch filters out.
        assert!(entry_scope_session_match(&mk(Some("EPIC-20"), None), Some(&lease), false));
        assert!(!entry_scope_session_match(&mk(Some("OTHER"), None), Some(&lease), false));
        // Scope-tagged entry without a lease → filtered (entry was for
        // a session, this shell isn't in one).
        assert!(!entry_scope_session_match(&mk(Some("EPIC-20"), None), None, false));

        // Session prefix matches case-insensitively.
        assert!(entry_scope_session_match(&mk(None, Some("abcdef12")), Some(&lease), false));
        assert!(entry_scope_session_match(&mk(None, Some("ABCDEF12")), Some(&lease), false));
        // Wrong session prefix is filtered.
        assert!(!entry_scope_session_match(&mk(None, Some("99999999")), Some(&lease), false));

        // Bypass shows everything regardless.
        assert!(entry_scope_session_match(&mk(Some("OTHER"), None), Some(&lease), true));
        assert!(entry_scope_session_match(&mk(Some("EPIC-20"), Some("99999999")), None, true));
    }

    /// STORY-56: aggregating a session log into the project role keeps
    /// only the newest entry per spec_id, merges in front of the role's
    /// existing activity, and respects ACTIVITY_MAX. The session's
    /// per-spec winners survive even when the project role already had
    /// older entries for the same specs (the session entry is fresher,
    /// so it wins).
    /// trace:STORY-56 | ai:claude
    #[test]
    fn session_aggregation_dedupes_and_promotes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".aida/sessions")).unwrap();
        std::fs::create_dir_all(root.join(".aida/roles")).unwrap();

        // Seed a project role with one stale entry for STORY-X.
        let role_path = root.join(".aida/roles/implementer.toml");
        let stale = chrono::Utc::now() - chrono::Duration::hours(1);
        let role = RoleState {
            name: "implementer".into(),
            purpose: None,
            created_at: stale,
            last_active_at: stale,
            working_directory: None,
            notes: None,
            global: false,
            activity: vec![RoleActivity {
                spec_id: "STORY-X".into(),
                action: "edit".into(),
                at: stale,
            }],
            scope_tags: vec![],
            scope_status: None,
            system_prompt: None,
        };
        std::fs::write(&role_path, toml::to_string_pretty(&role).unwrap()).unwrap();

        // Session log: STORY-X (newer than the seed) and STORY-Y.
        let id = "agg-session-01";
        append_session_activity(root, id, "implementer", "STORY-X", "show").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        append_session_activity(root, id, "implementer", "STORY-Y", "edit").unwrap();

        aggregate_session_activity_into_roles(root, id);

        let merged: RoleState =
            toml::from_str(&std::fs::read_to_string(&role_path).unwrap()).unwrap();
        let ids: Vec<&str> = merged.activity.iter().map(|a| a.spec_id.as_str()).collect();
        // STORY-Y (newest in session) first, then STORY-X (the session
        // entry wins over the seed; the seed's stale STORY-X is dropped).
        assert_eq!(ids, vec!["STORY-Y", "STORY-X"]);
        assert!(
            merged.activity[1].at > stale,
            "session-promoted STORY-X must carry the session timestamp, not the stale seed"
        );
    }

    /// STORY-52: detect_cargo_target_dir returns Some when target/ exists
    /// and None otherwise — the latter case is the "not a Rust project /
    /// never built" path that should silently skip env-shim generation.
    /// trace:STORY-52 | ai:claude
    #[test]
    fn detect_cargo_target_dir_only_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(detect_cargo_target_dir(root), None);

        std::fs::create_dir_all(root.join("target")).unwrap();
        let got = detect_cargo_target_dir(root).expect("target/ exists");
        // Result is canonicalized, so just assert it points at the dir we made.
        assert_eq!(
            got.canonicalize().unwrap(),
            root.join("target").canonicalize().unwrap()
        );

        // A regular file named `target` doesn't count.
        let tmp2 = tempfile::tempdir().unwrap();
        std::fs::write(tmp2.path().join("target"), b"not a dir").unwrap();
        assert_eq!(detect_cargo_target_dir(tmp2.path()), None);
    }

    /// STORY-52: render_session_env_file emits a sourceable export with
    /// the path POSIX-quoted so spaces or apostrophes in the parent path
    /// don't break the shell.
    /// trace:STORY-52 | ai:claude
    #[test]
    fn render_session_env_file_quotes_path() {
        let body = render_session_env_file(std::path::Path::new("/tmp/aida/target"));
        assert!(body.contains("export CARGO_TARGET_DIR='/tmp/aida/target'"));
        assert!(body.starts_with("# Generated by `aida session start`"));

        // Apostrophe in the path → close-reopen escape.
        let body = render_session_env_file(std::path::Path::new("/tmp/joe's repo/target"));
        assert!(body.contains("export CARGO_TARGET_DIR='/tmp/joe'\\''s repo/target'"));
    }

    /// STORY-52: write_session_env_file writes `.aida/session-env.sh` under
    /// the worktree, creating `.aida/` if it doesn't exist yet (the symlink
    /// pass in session_start sometimes runs before this, sometimes the dir
    /// is fresh — either way it should land in place).
    /// trace:STORY-52 | ai:claude
    #[test]
    fn write_session_env_file_creates_aida_dir_if_needed() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path();
        write_session_env_file(worktree, std::path::Path::new("/tmp/parent/target")).unwrap();
        let written =
            std::fs::read_to_string(worktree.join(".aida").join("session-env.sh")).unwrap();
        assert!(written.contains("CARGO_TARGET_DIR='/tmp/parent/target'"));
    }

    /// STORY-52: leases predating the cargo_target_dir field must still
    /// deserialize cleanly so an old session can be ended after upgrading
    /// aida. `#[serde(default)]` handles this; this test pins the contract.
    /// trace:STORY-52 | ai:claude
    #[test]
    fn lease_without_cargo_target_dir_deserializes() {
        let toml_text = r#"
id = "abcdef123456"
scope = "EPIC-20"
slug = "epic-20"
owner = "u"
worktree_path = "/tmp/wt"
branch = "br"
started_at = "2026-05-04T00:00:00Z"
hostname = "h"
"#;
        let lease: SessionLease = toml::from_str(toml_text).unwrap();
        assert_eq!(lease.id, "abcdef123456");
        assert!(lease.cargo_target_dir.is_none());
        // STORY-58 field carries forward the same back-compat contract.
        assert!(lease.parent_project_root.is_none());
    }

    /// STORY-58: from inside a worktree covered by a lease that records a
    /// parent, the helper returns that parent. Models the on-disk layout
    /// session_start produces (lease lives at <root>/.aida/sessions/) so
    /// we exercise the actual lookup path.
    /// trace:STORY-58 | ai:claude
    #[test]
    fn parent_project_root_for_session_returns_recorded_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let parent_dir = root.join("aida");
        let worktree = root.join("aida-epic-20");
        std::fs::create_dir_all(&parent_dir).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        let leases = worktree.join(".aida").join("sessions");
        std::fs::create_dir_all(&leases).unwrap();

        let lease = SessionLease {
            id: "abcdef123456".into(),
            scope: "EPIC-20".into(),
            slug: "epic-20".into(),
            owner: "u".into(),
            worktree_path: worktree.canonicalize().unwrap(),
            branch: "epic-20".into(),
            started_at: chrono::Utc::now(),
            hostname: "h".into(),
            cargo_target_dir: None,
            parent_project_root: Some(parent_dir.canonicalize().unwrap()),
            pr_head_sha: None,
            pr_base_sha: None,
            pr_base_ref: None,
        };
        std::fs::write(
            leases.join("abcdef123456.toml"),
            toml::to_string_pretty(&lease).unwrap(),
        )
        .unwrap();

        let got = parent_project_root_for_session(&worktree).expect("active lease w/ parent");
        assert_eq!(got, parent_dir.canonicalize().unwrap());
    }

    /// STORY-58: pre-STORY-58 leases (no parent recorded) return None
    /// even when the cwd is squarely inside the lease's worktree, so the
    /// list path falls back to the classic single-group output.
    /// trace:STORY-58 | ai:claude
    #[test]
    fn parent_project_root_for_session_none_for_legacy_lease() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        let leases = worktree.join(".aida").join("sessions");
        std::fs::create_dir_all(&leases).unwrap();

        // Old-format lease: no parent_project_root field.
        let toml_text = format!(
            r#"
id = "legacylease01"
scope = "EPIC-20"
slug = "epic-20"
owner = "u"
worktree_path = "{}"
branch = "br"
started_at = "2026-05-04T00:00:00Z"
hostname = "h"
"#,
            worktree.canonicalize().unwrap().display()
        );
        std::fs::write(leases.join("legacylease01.toml"), toml_text).unwrap();

        assert!(parent_project_root_for_session(&worktree).is_none());
    }

    /// STORY-58: outside a session worktree (no lease covers cwd), the
    /// helper returns None — list stays single-group as today.
    /// trace:STORY-58 | ai:claude
    #[test]
    fn parent_project_root_for_session_none_when_no_lease() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("not-a-session");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join(".aida").join("sessions")).unwrap();
        assert!(parent_project_root_for_session(&dir).is_none());
    }

    /// STORY-71: parsing the JSON shape `gh pr view --json
    /// headRefOid,baseRefOid,baseRefName` returns. Pinned so a future
    /// refactor of the field list can't silently break the lease enrichment.
    /// trace:STORY-71 | ai:claude
    #[test]
    fn parse_pr_metadata_json_github_shape() {
        let body = serde_json::json!({
            "headRefOid": "deadbeefcafe1234567890abcdef1234567890ab",
            "baseRefOid": "0011223344556677889900112233445566778899",
            "baseRefName": "main",
        });
        let m = parse_pr_metadata_json(ReviewForge::GitHub, &body);
        assert_eq!(
            m.head_sha.as_deref(),
            Some("deadbeefcafe1234567890abcdef1234567890ab")
        );
        assert_eq!(
            m.base_sha.as_deref(),
            Some("0011223344556677889900112233445566778899")
        );
        assert_eq!(m.base_ref.as_deref(), Some("main"));
    }

    /// STORY-71: glab's `mr view --output json` mirrors the GitLab REST
    /// API — head SHA in `sha`, base SHA in `diff_refs.base_sha`, base
    /// ref in `target_branch`. Test pins all three lookups.
    /// trace:STORY-71 | ai:claude
    #[test]
    fn parse_pr_metadata_json_gitlab_shape() {
        let body = serde_json::json!({
            "sha": "1111222233334444555566667777888899990000",
            "diff_refs": {
                "base_sha": "aaaabbbbccccddddeeeeffff0000111122223333",
                "head_sha": "1111222233334444555566667777888899990000",
            },
            "target_branch": "develop",
        });
        let m = parse_pr_metadata_json(ReviewForge::GitLab, &body);
        assert_eq!(
            m.head_sha.as_deref(),
            Some("1111222233334444555566667777888899990000")
        );
        assert_eq!(
            m.base_sha.as_deref(),
            Some("aaaabbbbccccddddeeeeffff0000111122223333")
        );
        assert_eq!(m.base_ref.as_deref(), Some("develop"));
    }

    /// STORY-71: missing or empty fields drop through as None rather than
    /// pinning empty strings into the lease. Forwards-compat for forge
    /// CLIs that omit some keys (auth scope, schema drift, etc.).
    /// trace:STORY-71 | ai:claude
    #[test]
    fn parse_pr_metadata_json_missing_fields_yield_none() {
        let body = serde_json::json!({ "headRefOid": "" });
        let m = parse_pr_metadata_json(ReviewForge::GitHub, &body);
        assert!(m.head_sha.is_none());
        assert!(m.base_sha.is_none());
        assert!(m.base_ref.is_none());
    }

    /// STORY-71: leases written before the new PR fields existed must
    /// still deserialize cleanly (so an in-flight session survives an
    /// aida upgrade). Test pins the back-compat contract.
    /// trace:STORY-71 | ai:claude
    #[test]
    fn lease_without_pr_fields_deserializes() {
        let toml_text = r#"
id = "abcdef123456"
scope = "PR-3"
slug = "pr-3"
owner = "u"
worktree_path = "/tmp/wt"
branch = "pr-3"
started_at = "2026-05-04T00:00:00Z"
hostname = "h"
"#;
        let lease: SessionLease = toml::from_str(toml_text).unwrap();
        assert!(lease.pr_head_sha.is_none());
        assert!(lease.pr_base_sha.is_none());
        assert!(lease.pr_base_ref.is_none());
    }
}

#[cfg(test)]
mod lease_enforcement_tests {
    use super::*;
    use aida_core::models::Relationship;

    /// trace:STORY-48 | ai:claude
    fn lease(scope: &str, id: &str) -> SessionLease {
        SessionLease {
            id: id.to_string(),
            scope: scope.to_string(),
            slug: scope.to_lowercase(),
            owner: "tester".into(),
            worktree_path: std::path::PathBuf::from(format!("/tmp/{}", id)),
            branch: format!("br-{}", id),
            started_at: chrono::Utc::now(),
            hostname: "test".into(),
            cargo_target_dir: None,
            parent_project_root: None,
            pr_head_sha: None,
            pr_base_sha: None,
            pr_base_ref: None,
        }
    }

    /// AIDA's parent-edge convention: a child stores `rel_type: Child`
    /// pointing at its parent (display reads "X is child of Y"). So to
    /// model "this requirement has these parents" in fixtures, we emit
    /// `Child` edges from `r` to each parent UUID.
    /// trace:STORY-48 | ai:claude
    fn req_with_parents(spec_id: &str, parents: &[Uuid]) -> Requirement {
        let mut r = Requirement::new(format!("Title for {}", spec_id), "".into());
        r.spec_id = Some(spec_id.into());
        r.relationships = parents
            .iter()
            .map(|pid| Relationship {
                rel_type: RelationshipType::Child,
                target_id: *pid,
                created_at: Some(chrono::Utc::now()),
                created_by: None,
            })
            .collect();
        r
    }

    /// Direct spec-id ownership: lease scope == target spec id.
    /// trace:STORY-48 | ai:claude
    #[test]
    fn lease_owns_direct_spec_match() {
        let target = req_with_parents("STORY-48", &[]);
        let mut store = RequirementsStore::new();
        store.requirements.push(target.clone());
        let leases = vec![lease("STORY-48", "abc123")];
        let owner = lease_owning_spec(&leases, None, target.id, target.spec_id.as_deref(), &store);
        assert!(owner.is_some());
        assert_eq!(owner.unwrap().scope, "STORY-48");
    }

    /// EPIC-scope ownership: lease.scope is the parent of target.
    /// trace:STORY-48 | ai:claude
    #[test]
    fn lease_owns_via_parent_chain() {
        let epic = req_with_parents("EPIC-20", &[]);
        let story = req_with_parents("STORY-48", &[epic.id]);
        let mut store = RequirementsStore::new();
        store.requirements.push(epic.clone());
        store.requirements.push(story.clone());
        let leases = vec![lease("EPIC-20", "epic")];
        let owner = lease_owning_spec(&leases, None, story.id, story.spec_id.as_deref(), &store);
        assert!(owner.is_some(), "EPIC-scope lease should own descendant story");
        assert_eq!(owner.unwrap().scope, "EPIC-20");
    }

    /// Self-lease must be skipped — a session can edit specs in its own
    /// scope without a warning.
    /// trace:STORY-48 | ai:claude
    #[test]
    fn lease_owning_skips_self() {
        let target = req_with_parents("STORY-48", &[]);
        let mut store = RequirementsStore::new();
        store.requirements.push(target.clone());
        let mine = lease("STORY-48", "self");
        let leases = vec![mine.clone()];
        let owner = lease_owning_spec(&leases, Some(&mine), target.id, target.spec_id.as_deref(), &store);
        assert!(owner.is_none(), "should not flag the caller's own lease");
    }

    /// BUG-54: a session whose scope is an EPIC must be allowed to edit
    /// children of that EPIC from inside the worktree. Direct-spec-match
    /// covers `aida edit EPIC-X` from the EPIC-X session; this exercises
    /// the parent-chain case (`aida edit <child-of-EPIC-X>`), which is
    /// the actual flow that triggered the in-session enforcement bug.
    /// trace:BUG-54 | ai:claude
    #[test]
    fn lease_owning_skips_self_via_parent_chain() {
        let epic = req_with_parents("EPIC-20", &[]);
        let story = req_with_parents("STORY-55", &[epic.id]);
        let mut store = RequirementsStore::new();
        store.requirements.push(epic.clone());
        store.requirements.push(story.clone());
        let mine = lease("EPIC-20", "ownsepic");
        let leases = vec![mine.clone()];
        let owner = lease_owning_spec(
            &leases,
            Some(&mine),
            story.id,
            story.spec_id.as_deref(),
            &store,
        );
        assert!(
            owner.is_none(),
            "owner-of-EPIC-X session must be allowed to edit children of EPIC-X"
        );
    }

    /// Path-glob / free-form scopes that don't resolve to a spec id are
    /// treated as non-enforced.
    /// trace:STORY-48 | ai:claude
    #[test]
    fn lease_owning_ignores_unresolved_scopes() {
        let target = req_with_parents("STORY-48", &[]);
        let mut store = RequirementsStore::new();
        store.requirements.push(target.clone());
        let leases = vec![lease("src/scaffolding/**", "glob")];
        let owner = lease_owning_spec(&leases, None, target.id, target.spec_id.as_deref(), &store);
        assert!(owner.is_none());
    }

    /// Cycle in parent edges must not infinite-loop the ancestor walk.
    /// trace:STORY-48 | ai:claude
    #[test]
    fn lease_owning_handles_parent_cycle() {
        let mut a = req_with_parents("FR-A", &[]);
        let mut b = req_with_parents("FR-B", &[]);
        // Cycle uses `Child` edges (the climb-toward-root direction in
        // AIDA's storage convention). Each side points at the other as
        // its "parent" — pathological, but lease_owning_spec must
        // terminate even so.
        a.relationships = vec![Relationship {
            rel_type: RelationshipType::Child,
            target_id: b.id,
            created_at: Some(chrono::Utc::now()),
            created_by: None,
        }];
        b.relationships = vec![Relationship {
            rel_type: RelationshipType::Child,
            target_id: a.id,
            created_at: Some(chrono::Utc::now()),
            created_by: None,
        }];
        let mut store = RequirementsStore::new();
        store.requirements.push(a.clone());
        store.requirements.push(b.clone());
        // No lease covers either; the call must terminate.
        let leases: Vec<SessionLease> = vec![];
        let owner = lease_owning_spec(&leases, None, a.id, a.spec_id.as_deref(), &store);
        assert!(owner.is_none());
    }
}

#[cfg(test)]
mod scope_fallback_tests {
    use super::*;
    use aida_core::models::Relationship;

    fn lease_for(scope: &str) -> SessionLease {
        SessionLease {
            id: "selflease0001".into(),
            scope: scope.to_string(),
            slug: scope.to_lowercase(),
            owner: "tester".into(),
            worktree_path: std::path::PathBuf::from("/tmp/x"),
            branch: "br".into(),
            started_at: chrono::Utc::now(),
            hostname: "h".into(),
            cargo_target_dir: None,
            parent_project_root: None,
            pr_head_sha: None,
            pr_base_sha: None,
            pr_base_ref: None,
        }
    }

    fn child_of(parent_uuid: Uuid, spec_id: &str) -> Requirement {
        let mut r = Requirement::new(format!("Title for {}", spec_id), "".into());
        r.spec_id = Some(spec_id.into());
        r.relationships = vec![Relationship {
            rel_type: RelationshipType::Child,
            target_id: parent_uuid,
            created_at: Some(chrono::Utc::now()),
            created_by: None,
        }];
        r.status = RequirementStatus::Approved;
        r.priority = RequirementPriority::Medium;
        r
    }

    fn scope_root(spec_id: &str, child_uuids: &[Uuid]) -> Requirement {
        let mut r = Requirement::new(format!("Title for {}", spec_id), "".into());
        r.spec_id = Some(spec_id.into());
        r.relationships = child_uuids
            .iter()
            .map(|cid| Relationship {
                rel_type: RelationshipType::Parent,
                target_id: *cid,
                created_at: Some(chrono::Utc::now()),
                created_by: None,
            })
            .collect();
        r
    }

    /// Highest-priority approved child wins.
    /// trace:STORY-63 | ai:claude
    #[test]
    fn picks_highest_priority_approved_child() {
        let mut a = child_of(Uuid::nil(), "STORY-1");
        let mut b = child_of(Uuid::nil(), "STORY-2");
        let mut c = child_of(Uuid::nil(), "STORY-3");
        a.priority = RequirementPriority::Low;
        b.priority = RequirementPriority::High;
        c.priority = RequirementPriority::Medium;
        let epic = scope_root("EPIC-20", &[a.id, b.id, c.id]);
        // Patch ancestors so child rels point at the EPIC's actual id.
        for child in [&mut a, &mut b, &mut c] {
            child.relationships[0].target_id = epic.id;
        }
        let mut store = RequirementsStore::new();
        store.requirements.extend([epic.clone(), a, b.clone(), c]);
        let lease = lease_for("EPIC-20");
        let res = scope_fallback_pick(&store, &lease, None).expect("expected a pick");
        assert_eq!(res.pick.spec_id.as_deref(), Some("STORY-2"));
        assert_eq!(res.approved_count, 3);
    }

    /// Created_at breaks ties at equal priority — older wins.
    /// trace:STORY-63 | ai:claude
    #[test]
    fn ties_break_on_created_at_oldest_first() {
        let mut older = child_of(Uuid::nil(), "STORY-A");
        let mut newer = child_of(Uuid::nil(), "STORY-B");
        older.priority = RequirementPriority::High;
        newer.priority = RequirementPriority::High;
        let now = chrono::Utc::now();
        older.created_at = now - chrono::Duration::hours(2);
        newer.created_at = now;
        let epic = scope_root("EPIC-20", &[older.id, newer.id]);
        for child in [&mut older, &mut newer] {
            child.relationships[0].target_id = epic.id;
        }
        let mut store = RequirementsStore::new();
        store.requirements.extend([epic.clone(), older, newer]);
        let lease = lease_for("EPIC-20");
        let res = scope_fallback_pick(&store, &lease, None).expect("expected a pick");
        assert_eq!(res.pick.spec_id.as_deref(), Some("STORY-A"));
    }

    /// Any sibling InProgress → no pick (don't run two children in
    /// parallel under the same EPIC).
    /// trace:STORY-63 | ai:claude
    #[test]
    fn skips_when_sibling_in_progress() {
        let mut active = child_of(Uuid::nil(), "STORY-ACTIVE");
        let mut waiting = child_of(Uuid::nil(), "STORY-WAITING");
        active.status = RequirementStatus::InProgress;
        waiting.priority = RequirementPriority::High;
        let epic = scope_root("EPIC-20", &[active.id, waiting.id]);
        for child in [&mut active, &mut waiting] {
            child.relationships[0].target_id = epic.id;
        }
        let mut store = RequirementsStore::new();
        store.requirements.extend([epic, active, waiting]);
        let lease = lease_for("EPIC-20");
        let res = scope_fallback_pick(&store, &lease, None);
        assert!(res.is_none(), "should not double-pick under the same EPIC");
    }

    /// Path-glob / free-form scope can't resolve to a Requirement → None.
    /// trace:STORY-63 | ai:claude
    #[test]
    fn unresolved_scope_returns_none() {
        let store = RequirementsStore::new();
        let lease = lease_for("src/**/*.rs");
        assert!(scope_fallback_pick(&store, &lease, None).is_none());
    }

    /// Scope resolves but has no children → None (caller falls through
    /// to the normal "queue empty" message + nudge).
    /// trace:STORY-63 | ai:claude
    #[test]
    fn scope_with_no_children_returns_none() {
        let epic = scope_root("EPIC-20", &[]);
        let mut store = RequirementsStore::new();
        store.requirements.push(epic);
        let lease = lease_for("EPIC-20");
        assert!(scope_fallback_pick(&store, &lease, None).is_none());
    }

    /// Children exist but none are Approved → None.
    /// trace:STORY-63 | ai:claude
    #[test]
    fn no_approved_children_returns_none() {
        let mut draft = child_of(Uuid::nil(), "STORY-DRAFT");
        draft.status = RequirementStatus::Draft;
        let epic = scope_root("EPIC-20", &[draft.id]);
        draft.relationships[0].target_id = epic.id;
        let mut store = RequirementsStore::new();
        store.requirements.extend([epic, draft]);
        let lease = lease_for("EPIC-20");
        assert!(scope_fallback_pick(&store, &lease, None).is_none());
    }

    /// Role scope filter on tags is honored — a candidate without all
    /// the role's required tags is dropped.
    /// trace:STORY-63 | ai:claude
    #[test]
    fn role_scope_filter_drops_untagged_candidates() {
        let mut tagged = child_of(Uuid::nil(), "STORY-TAGGED");
        let mut untagged = child_of(Uuid::nil(), "STORY-PLAIN");
        tagged.priority = RequirementPriority::Low;
        untagged.priority = RequirementPriority::High;
        tagged.tags.insert("session".into());
        let epic = scope_root("EPIC-20", &[tagged.id, untagged.id]);
        for child in [&mut tagged, &mut untagged] {
            child.relationships[0].target_id = epic.id;
        }
        let mut store = RequirementsStore::new();
        store.requirements.extend([epic, tagged, untagged]);
        let lease = lease_for("EPIC-20");
        // Role requires the "session" tag — the untagged High-prio item
        // is dropped, the tagged Low-prio item wins.
        let role_scope = (vec!["session".to_string()], None);
        let res = scope_fallback_pick(&store, &lease, Some(&role_scope))
            .expect("expected a pick");
        assert_eq!(res.pick.spec_id.as_deref(), Some("STORY-TAGGED"));
    }
}

#[cfg(test)]
mod store_walkup_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// `.aida/config.toml` only at the repo root → `aida edit` from a
    /// nested subdir must still resolve the store.
    /// trace:BUG-57 | ai:claude
    #[test]
    fn detect_distributed_store_walks_up_from_subdir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".aida")).unwrap();
        fs::write(
            root.join(".aida/config.toml"),
            "[deployment]\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join(".aida-store")).unwrap();

        let nested = root.join("aida-cli/src/foo");
        fs::create_dir_all(&nested).unwrap();

        let resolved = detect_distributed_store_from(&nested).expect("should walk up");
        assert_eq!(resolved, root.join(".aida-store"));
    }

    /// store_path is interpreted relative to the directory containing
    /// config.toml, not relative to the starting cwd.
    /// trace:BUG-57 | ai:claude
    #[test]
    fn detect_distributed_store_resolves_relative_to_config_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".aida")).unwrap();
        fs::write(
            root.join(".aida/config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join(".aida-store")).unwrap();

        let nested = root.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();

        // If we incorrectly resolved relative to nested, we'd look for
        // `<nested>/.aida-store/` which doesn't exist.
        let resolved = detect_distributed_store_from(&nested).unwrap();
        assert_eq!(resolved, root.join(".aida-store"));
    }

    /// No `.aida/config.toml` anywhere up the tree → returns None (caller
    /// falls through to legacy / registry resolution).
    /// trace:BUG-57 | ai:claude
    #[test]
    fn detect_distributed_store_returns_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(detect_distributed_store_from(&nested).is_none());
    }
}

/// Apply the user's `--color=auto|always|never` choice to the colored
/// crate's global override. `auto` is the colored crate's default
/// behavior — it uses `isatty(stdout)` and respects `NO_COLOR`.
/// trace:FR-1-041 | ai:claude
fn apply_color_mode(mode: &str) {
    match mode {
        "always" => colored::control::set_override(true),
        "never" => colored::control::set_override(false),
        _ => {} // "auto" — let colored crate decide via tty detection
    }
}

/// Count queue entries in the user's queue file that pass the role filter.
/// When `role` is `Some`, returns the count of entries with `for_role`
/// matching exactly. When `role` is `None`, returns the total entry
/// count (no role filter).
///
/// Reads `<project>/.aida-store/registry/queues/<user>.yaml` directly —
/// keeps statusline off the heavier Storage::load() path so the sub-50ms
/// budget holds even when the orphan store has hundreds of objects.
/// Returns `None` if the file is missing or unreadable.
/// trace:FR-1-041 | ai:claude
fn read_queue_depth(project_root: &std::path::Path, role: Option<&str>) -> Option<usize> {
    let user = std::env::var("AIDA_USER")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string());

    let queue_path = project_root
        .join(".aida-store/registry/queues")
        .join(format!("{}.yaml", user));

    let content = std::fs::read_to_string(&queue_path).ok()?;
    let entries: Vec<serde_yaml::Value> = serde_yaml::from_str(&content).ok()?;

    let count = match role {
        Some(want) => entries
            .iter()
            .filter(|e| {
                e.get("for_role")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|r| r == want)
                    .unwrap_or(false)
            })
            .count(),
        None => entries.len(),
    };
    Some(count)
}

fn handle_statusline_command(color: &str) -> Result<()> {
    // trace:FR-1-041 | ai:claude
    apply_color_mode(color);

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

    // Cache stats — fast SQLite lookups, no rebuild. Only used now for
    // the cache:fresh|stale label; the requirement count is no longer
    // surfaced (FR-1-041 swapped reqs:N for queue depth, which is more
    // actionable).
    let cache_path = project_root.join(".aida/cache.db");
    let cache_label = if cache_path.exists() {
        match rusqlite::Connection::open_with_flags(
            &cache_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(conn) => {
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
                // Prefix-tolerant equality: cache.set_source_head_sha is
                // called with whatever `git_ops::head_sha` returns (today
                // that's `git rev-parse --short HEAD` → 7 chars), but
                // older aida versions and `cache rebuild` paths have
                // historically stored full 40-char SHAs in some installs.
                // Treat either side as a prefix-match of the other so
                // statusline doesn't false-positive on the mixed form.
                // trace:TASK-1-045 | ai:claude
                let stale = recorded_sha
                    .as_deref()
                    .map(|recorded| !sha_prefix_match(recorded, &actual_sha))
                    .unwrap_or(true);
                Some(if !actual_sha.is_empty() && stale { "stale" } else { "fresh" })
            }
            Err(_) => None,
        }
    } else {
        None
    };

    // Queue depth — count entries in the user's queue routed to the
    // active role (or all entries when no role is set). Reads the orphan
    // store's queues/<user>.yaml file directly to keep this off the
    // backend's heavier load() path.
    let queue_depth = read_queue_depth(&project_root, role.as_deref()).unwrap_or(0);

    let separator = " · ".dimmed().to_string();
    // Brand anchor: Greek transliteration of "AIDA". Same 4-column
    // footprint as the prior literal "aida"; the δ is the only
    // unmistakably-Greek glyph, so the prefix carries a quiet identity
    // marker without looking foreign. trace:TASK-1-048 | ai:claude
    let mut parts: Vec<String> = vec![
        "αιδα".dimmed().to_string(),
        project_label.green().bold().to_string(),
    ];
    // Resolve the active session lease once: both the @SPEC fallback and
    // the dedicated sess: segment use it, and we want a single canonicalize
    // + read-dir per render. trace:STORY-53 | ai:claude
    let lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(&project_root, &cwd));

    if let Some(r) = &role {
        parts.push(format!("role:{}", r).yellow().bold().to_string());
        // @SPEC segment. Default: newest activity entry the active role
        // touched. Source preference: the session-local activity log when
        // cwd is inside an active session lease (STORY-56), else the
        // project-level role's activity stream. Override: with a session
        // active but no role activity yet inside it, fall back to
        // `@<scope>` so the prompt advertises the session anchor instead
        // of a pre-session spec. trace:STORY-55 | ai:claude
        let session_latest: Option<RoleActivity> = lease
            .as_ref()
            .and_then(|l| {
                load_session_activity(&project_root, &l.id)
                    .entries
                    .into_iter()
                    .find(|e| e.role == *r)
                    .map(|e| RoleActivity {
                        spec_id: e.spec_id,
                        action: e.action,
                        at: e.at,
                    })
            });
        let role_state = load_role(&project_root, r).ok().map(|(s, _)| s);
        let project_latest = role_state.as_ref().and_then(|s| s.activity.first()).cloned();
        let latest: Option<RoleActivity> = match (lease.as_ref(), session_latest, project_latest) {
            // In-session: prefer session log; only consider project-level
            // entries that are newer than the lease (i.e. a freshly
            // promoted entry from a prior `session end`).
            (Some(l), Some(s), _) => Some(s).filter(|s| s.at >= l.started_at),
            (Some(l), None, p) => p.filter(|p| p.at >= l.started_at),
            (None, _, p) => p,
        };
        let label: Option<String> = match (latest, lease.as_ref()) {
            (Some(act), Some(l)) if act.at >= l.started_at => Some(act.spec_id.clone()),
            (_, Some(l)) => Some(truncate(&l.scope, SCOPE_LABEL_MAX)),
            (Some(act), None) => Some(act.spec_id.clone()),
            (None, None) => None,
        };
        if let Some(label) = label {
            parts.push(format!("@{}", label).cyan().bold().to_string());
        }
    }
    if queue_depth > 0 {
        parts.push(format!("q:{}", queue_depth));
    }
    // Cache freshness: only surface when stale. Fresh is the boring
    // default and the cache is self-healing on the next read, so showing
    // `cache:fresh` on every prompt is noise. Stale stays — red — so the
    // user sees that the next read will trigger a rebuild.
    // trace:TASK-1-045 | ai:claude
    if let Some(l) = cache_label {
        if l != "fresh" {
            parts.push(format!("cache:{}", l).red().to_string());
        }
    }
    // sess:<scope> segment — emitted whenever cwd resolves into an active
    // session lease's worktree, regardless of role. Coexists with @SPEC:
    // @SPEC answers "which spec am I touching", sess: answers "which
    // session-scope owns this shell" (the latter sticks even when the
    // role's recent activity is on a child spec). Same color as role:
    // since both answer "what context am I in". Scope is truncated to the
    // same budget as @SPEC so the line stays scannable.
    // trace:STORY-53 | ai:claude
    if let Some(l) = lease.as_ref() {
        let label = truncate(&l.scope, SCOPE_LABEL_MAX);
        parts.push(format!("sess:{}", label).yellow().bold().to_string());
    }
    println!("{}", parts.join(&separator));
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
        DevCommand::Activate { repo, profile, debug, release, auto } => {
            handle_dev_activate(repo.as_deref(), profile.as_deref(), *debug, *release, *auto)
        }
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

/// Pick the build directory + profile name for activation, honoring an
/// explicit profile request (debug / release) when given, else falling
/// back to whichever of `target/release/aida` vs `target/debug/aida` has
/// the more recent mtime. Errors when the requested profile isn't built,
/// or when neither exists at all.
fn pick_dev_binary_dir(
    repo: &std::path::Path,
    requested: Option<&str>,
) -> Result<(std::path::PathBuf, &'static str)> {
    let release = repo.join("target/release/aida");
    let debug = repo.join("target/debug/aida");
    let release_mtime = std::fs::metadata(&release).and_then(|m| m.modified()).ok();
    let debug_mtime = std::fs::metadata(&debug).and_then(|m| m.modified()).ok();

    if let Some(req) = requested {
        return match req {
            "debug" => {
                if debug_mtime.is_none() {
                    anyhow::bail!(
                        "No debug build at {}.\nRun `cargo build` (debug) first.",
                        debug.display()
                    );
                }
                Ok((repo.join("target/debug"), "debug"))
            }
            "release" => {
                if release_mtime.is_none() {
                    anyhow::bail!(
                        "No release build at {}.\nRun `cargo build --release` first.",
                        release.display()
                    );
                }
                Ok((repo.join("target/release"), "release"))
            }
            other => anyhow::bail!("unknown profile '{}': expected debug or release", other),
        };
    }

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

/// True when the inactive-side build at `<repo>/target/<other>/aida` is
/// newer than the active-side build at `<repo>/target/<active>/aida`.
/// Used for the stale-build warning + PS1 marker.
/// trace:FR-1-068 | ai:claude
fn alternate_build_is_newer(repo: &std::path::Path, active: &str) -> bool {
    let other = if active == "debug" { "release" } else { "debug" };
    let active_mtime = std::fs::metadata(repo.join(format!("target/{}/aida", active)))
        .and_then(|m| m.modified())
        .ok();
    let other_mtime = std::fs::metadata(repo.join(format!("target/{}/aida", other)))
        .and_then(|m| m.modified())
        .ok();
    matches!((active_mtime, other_mtime), (Some(a), Some(o)) if o > a)
}

fn handle_dev_activate(
    repo_arg: Option<&str>,
    profile_pos: Option<&str>,
    debug_flag: bool,
    release_flag: bool,
    auto_flag: bool,
) -> Result<()> {
    let repo = resolve_aida_repo(repo_arg)?;

    // Resolve the explicit-profile request from any of: positional `profile`,
    // --debug / --release / --auto flags, or an existing AIDA_DEV_PROFILE_PIN.
    // Precedence: explicit CLI request beats the env-var pin; --auto clears.
    // trace:FR-1-068 | ai:claude
    let cli_request: Option<&str> = match (profile_pos, debug_flag, release_flag, auto_flag) {
        (Some("debug"), _, _, _) => Some("debug"),
        (Some("release"), _, _, _) => Some("release"),
        (Some("auto"), _, _, _) => None, // positional 'auto' also clears
        (_, true, _, _) => Some("debug"),
        (_, _, true, _) => Some("release"),
        _ => None,
    };
    let clear_pin = auto_flag || profile_pos == Some("auto");
    let env_pin = std::env::var("AIDA_DEV_PROFILE_PIN").ok();
    let effective_request: Option<&str> = if clear_pin {
        None
    } else if cli_request.is_some() {
        cli_request
    } else {
        env_pin.as_deref().filter(|s| !s.is_empty())
    };

    let (bin_dir, profile) = pick_dev_binary_dir(&repo, effective_request)?;
    let stale = alternate_build_is_newer(&repo, profile);
    let ps1_marker = if stale { "*" } else { "" };

    // Quote-safety: paths shouldn't contain double-quotes in practice;
    // single-quote everything we emit so shell evaluation is safe.
    println!(
        "# aida dev activate — using {} build at {}{}",
        profile,
        bin_dir.display(),
        if stale { "  (alternate build is newer)" } else { "" }
    );
    println!("export AIDA_DEV_REPO='{}'", repo.display());
    println!("export AIDA_DEV_BIN='{}'", bin_dir.display());
    println!("export AIDA_DEV_PROFILE='{}'", profile);
    println!("export AIDA_DEV_ACTIVE=1");

    // Persist the pin across re-activations. Three cases:
    //   - explicit CLI request → set the pin to that profile
    //   - --auto / 'auto' positional → clear the pin
    //   - neither → leave the existing pin alone (sticky)
    if let Some(pin) = cli_request {
        println!("export AIDA_DEV_PROFILE_PIN='{}'", pin);
    } else if clear_pin {
        println!("unset AIDA_DEV_PROFILE_PIN");
    }

    println!("if [ -z \"${{AIDA_DEV_PREV_PATH+x}}\" ]; then");
    println!("    export AIDA_DEV_PREV_PATH=\"$PATH\"");
    println!("fi");
    println!("export PATH='{}':\"$PATH\"", bin_dir.display());
    // TASK-19: splice-in semantics for PS1 instead of save/restore. We
    // record the literal prefix we're prepending in AIDA_DEV_PS1_PREFIX
    // so deactivate can strip exactly the same string regardless of what
    // else (e.g., `aida role enter`) has touched PS1 in between.
    // trace:TASK-19 | ai:claude
    let ps1_prefix = format!("(aida-{}{}) ", profile, ps1_marker);
    println!("if [ -n \"${{PS1+x}}\" ]; then");
    println!("    export AIDA_DEV_PS1_PREFIX='{}'", ps1_prefix);
    println!("    export PS1=\"$AIDA_DEV_PS1_PREFIX$PS1\"");
    println!("fi");

    let pin_note = match cli_request {
        Some(p) => format!(", pinned to {}", p),
        None if clear_pin => ", pin cleared".to_string(),
        None => String::new(),
    };
    let stale_note = if stale {
        " ⚠ alternate build is newer — run `aida dev status` for details"
    } else {
        ""
    };
    println!(
        "echo '✓ aida dev activated ({} build at {}{}){}'",
        profile,
        bin_dir.display(),
        pin_note,
        stale_note
    );
    Ok(())
}

fn handle_dev_deactivate() -> Result<()> {
    println!("# aida dev deactivate — restoring PATH and splicing dev prefix out of PS1");
    println!("if [ -n \"${{AIDA_DEV_PREV_PATH+x}}\" ]; then");
    println!("    export PATH=\"$AIDA_DEV_PREV_PATH\"");
    println!("    unset AIDA_DEV_PREV_PATH");
    println!("fi");
    // TASK-19: splice-out semantics. Strip exactly the prefix we recorded
    // at activate time so any other PS1 modifiers added in between (role
    // prefix, virtualenv name, etc.) are preserved on deactivate.
    // trace:TASK-19 | ai:claude
    println!("if [ -n \"${{AIDA_DEV_PS1_PREFIX+x}}\" ] && [ -n \"${{PS1+x}}\" ]; then");
    println!("    PS1=\"${{PS1/$AIDA_DEV_PS1_PREFIX/}}\"");
    println!("    unset AIDA_DEV_PS1_PREFIX");
    println!("fi");
    // Clean up the legacy save/restore env var if any prior session set it.
    println!("unset AIDA_DEV_PREV_PS1");
    println!("unset AIDA_DEV_REPO AIDA_DEV_BIN AIDA_DEV_PROFILE AIDA_DEV_ACTIVE AIDA_DEV_PROFILE_PIN");
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
        match std::env::var("AIDA_DEV_PROFILE_PIN") {
            Ok(pin) if !pin.is_empty() => {
                println!("Pin:          {} (sticky across re-activations)", pin);
            }
            _ => {
                println!(
                    "Pin:          {} (freshest of debug/release wins on `aida dev activate`)",
                    "auto".dimmed()
                );
            }
        }
    }

    // Stale-build warning: when we know the active repo + profile, compare
    // the inactive-side build's mtime. If newer, surface — re-running
    // `aida dev activate` would silently flip you to the alternate.
    // trace:FR-1-068 | ai:claude
    if active {
        let repo = std::env::var("AIDA_DEV_REPO").ok();
        let profile = std::env::var("AIDA_DEV_PROFILE").ok();
        if let (Some(repo), Some(profile)) = (repo, profile) {
            let repo_path = std::path::PathBuf::from(&repo);
            if alternate_build_is_newer(&repo_path, &profile) {
                let other = if profile == "debug" { "release" } else { "debug" };
                println!();
                println!(
                    "{}: the {} build is newer than the active {} build.",
                    "WARN".yellow().bold(),
                    other.bold(),
                    profile.bold()
                );
                let pinned = std::env::var("AIDA_DEV_PROFILE_PIN")
                    .map(|p| !p.is_empty())
                    .unwrap_or(false);
                if pinned {
                    println!(
                        "      Pin keeps you on {}. Run `aida dev activate --auto` to clear",
                        profile
                    );
                    println!(
                        "      and pick the freshest, or `aida dev activate {}` to switch.",
                        other
                    );
                } else {
                    println!(
                        "      Re-run `aida dev activate` to flip to {}, or pin with",
                        other
                    );
                    println!(
                        "      `aida dev activate {}` to keep working on {}.",
                        profile, profile
                    );
                }
            }
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
// trace:FR-2-004 | ai:claude
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

    // Detect if this repo has a git-canonical aida store to sync.
    let store_path = detect_store_path(&repo);

    // ── Step 1/5: sync store (pull) ──────────────────────────────────────
    if let Some(ref sp) = store_path {
        println!(
            "{}",
            "─── Step 1/5: syncing aida-store (pull) ───".bold()
        );
        let branch = aida_core::git_ops::current_branch(sp)
            .unwrap_or_else(|_| "aida-store".to_string());
        // Use rebase: bare `git pull` fails on divergent branches when
        // the user has no pull.rebase/pull.ff config; the orphan-store
        // model wants linear history. trace:BUG-1-051 | ai:claude
        match aida_core::git_ops::pull_rebase(sp, "origin", &branch) {
            Ok(()) => println!("  Store pull complete."),
            Err(e) => {
                anyhow::bail!(
                    "aida-store pull failed: {}\n\
                     Resolve store conflicts first: aida db sync --pull\n\
                     Then re-run `aida dev release {}`.",
                    e, bump
                );
            }
        }
        println!();
    }

    // ── Step 2/5: run release.sh ─────────────────────────────────────────
    println!(
        "{}",
        format!("─── Step 2/5: ./scripts/release.sh {} ───", bump).bold()
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

    // ── Step 3/5: wait for GitHub release artifacts ───────────────────────
    println!();
    println!(
        "{}",
        format!("─── Step 3/5: waiting for {} release artifacts ───", new_tag).bold()
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

    // ── Step 4/5: upgrade sibling installs ───────────────────────────────
    println!();
    println!(
        "{}",
        format!("─── Step 4/5: upgrading sibling installs to {} ───", new_tag).bold()
    );
    let bare_version = strip_v(&new_tag).to_string();
    upgrade_dev_mode_sibling_scan(false, Some(&bare_version), true, false)?;

    // ── Step 5/5: sync store (push) ───────────────────────────────────────
    if let Some(ref sp) = store_path {
        println!();
        println!(
            "{}",
            format!("─── Step 5/5: syncing aida-store (push) for {} ───", new_tag).bold()
        );
        let branch = aida_core::git_ops::current_branch(sp)
            .unwrap_or_else(|_| "aida-store".to_string());

        // Commit any pending store changes (e.g., block pointer updates)
        if aida_core::git_ops::has_changes(sp).unwrap_or(false) {
            let msg = format!("chore: sync store for release {}", new_tag);
            let _ = aida_core::git_ops::add_all(sp, "objects");
            let _ = aida_core::git_ops::add_all(sp, "registry");
            if sp.join("metadata.yaml").exists() {
                let _ = aida_core::git_ops::add(sp, &["metadata.yaml"]);
            }
            if let Err(e) = aida_core::git_ops::commit(sp, &msg) {
                eprintln!("  Warning: could not commit store changes: {}", e);
            } else {
                println!("  Committed store changes: {}", msg);
            }
        }

        match aida_core::git_ops::push(sp, "origin", &branch) {
            Ok(true) => println!("  Store push complete."),
            Ok(false) => {
                println!("  Push rejected. Pulling and retrying...");
                if let Err(e) = aida_core::git_ops::pull_rebase(sp, "origin", &branch) {
                    eprintln!("  Warning: store pull-rebase failed: {}", e);
                } else {
                    match aida_core::git_ops::push(sp, "origin", &branch) {
                        Ok(_) => println!("  Store push complete after rebase."),
                        Err(e) => eprintln!("  Warning: store push failed after rebase: {}", e),
                    }
                }
            }
            Err(e) => eprintln!("  Warning: store push failed: {}", e),
        }
    }

    println!();
    println!(
        "{}: shipped {} and refreshed sibling installs.",
        "DONE".green().bold(),
        new_tag
    );
    Ok(())
}

/// Find the aida-store path for a given repo, if configured.
fn detect_store_path(repo: &std::path::Path) -> Option<std::path::PathBuf> {
    let config_path = repo.join(".aida").join("config.toml");
    if !config_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&config_path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("store_path") {
            if let Some(val) = line.split('=').nth(1) {
                let val = val.trim().trim_matches('"').trim_matches('\'');
                let sp = repo.join(val);
                if sp.exists() && sp.is_dir() && aida_core::git_ops::is_git_repo(&sp) {
                    return Some(sp);
                }
            }
        }
    }
    None
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
    // If we're in (or under) the aida repo, use its built binary —
    // whichever of target/release vs target/debug is more recently
    // built. Mirrors `pick_dev_binary_dir`'s mtime-based choice for the
    // CLI binary, so an old `target/release/aida-server` from a stale
    // `cargo build --release` doesn't shadow a current debug build.
    // (Bug surfaced 2026-05-05: a Feb-22 release binary at v0.1.0 was
    // beating the May-4 debug binary at v0.4.3, and v0.1.0 lacked
    // git-backend support so `dev serve` failed with a YAML parse
    // error against the orphan store.) trace:BUG-1-049 | ai:claude
    let mut probe = cwd.to_path_buf();
    for _ in 0..4 {
        if is_aida_repo(&probe) {
            let release = probe.join("target/release/aida-server");
            let debug = probe.join("target/debug/aida-server");
            let release_mtime = std::fs::metadata(&release).and_then(|m| m.modified()).ok();
            let debug_mtime = std::fs::metadata(&debug).and_then(|m| m.modified()).ok();
            match (release_mtime, debug_mtime) {
                (Some(rm), Some(dm)) => return Ok(if rm >= dm { release } else { debug }),
                (Some(_), None) => return Ok(release),
                (None, Some(_)) => return Ok(debug),
                (None, None) => anyhow::bail!(
                    "Found aida repo at {} but no aida-server binary in target/. Run `cargo build` first.",
                    probe.display()
                ),
            }
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
// `aida push` — push code AND the orphan store in one shot.
// trace:FR-264 | ai:claude
// ----------------------------------------------------------------------------

fn handle_push_command(
    store_path: &std::path::Path,
    code_only: bool,
    store_only: bool,
    message: Option<&str>,
) -> Result<()> {
    use aida_core::git_ops;

    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // ---- Code push (current branch on the project repo) ----
    if !store_only {
        if !git_ops::has_remote(&project_root, "origin") {
            println!("  {} no `origin` remote — skipping code push", "Note:".dimmed());
        } else {
            let branch = git_ops::current_branch(&project_root)
                .unwrap_or_else(|_| "HEAD".to_string());
            println!(
                "{} {} → origin",
                "Pushing code".cyan().bold(),
                branch
            );
            let res = std::process::Command::new("git")
                .arg("-C")
                .arg(&project_root)
                .args(["push", "origin", &branch])
                .status();
            match res {
                Ok(s) if s.success() => {
                    println!("  {}", "code push complete".green());
                }
                Ok(s) => {
                    eprintln!(
                        "  {} git push exited with status {}",
                        "Warning:".yellow().bold(),
                        s
                    );
                }
                Err(e) => anyhow::bail!("git push failed: {}", e),
            }
        }
    }

    // ---- Store push (orphan branch via aida db sync) ----
    // BUG-44: only print the "Pushing store..." header when we'll actually
    // attempt a push. Mirroring the code-push leg above, which prints only
    // a Note line when there's no origin.
    if !code_only {
        if !git_ops::is_git_repo(store_path) {
            println!("  {} no orphan worktree — skipping store push", "Note:".dimmed());
            return Ok(());
        }
        let store_has_origin = git_ops::has_remote(store_path, "origin");
        if store_has_origin {
            println!("{} aida-store → origin", "Pushing store".cyan().bold());
        } else {
            println!("  {} orphan store has no `origin` — skipping store push", "Note:".dimmed());
        }
        // Commit any pending orphan-branch changes regardless of origin —
        // the user's local edits should land in a commit either way so
        // subsequent operations have a clean tree. trace:BUG-44 | ai:claude
        if git_ops::has_changes(store_path).unwrap_or(false) {
            let msg = message.unwrap_or("chore: sync pending changes");
            let _ = git_ops::add(store_path, &["."]);
            let _ = git_ops::commit(store_path, msg);
            println!("  Committed: {}", msg);
        }
        if store_has_origin {
            let branch = git_ops::current_branch(store_path)
                .unwrap_or_else(|_| "aida-store".to_string());
            match git_ops::push(store_path, "origin", &branch) {
                Ok(true) => println!("  {}", "store push complete".green()),
                Ok(false) => {
                    eprintln!(
                        "  {} push rejected — pull/rebase first (`aida db sync --pull`)",
                        "Warning:".yellow().bold()
                    );
                }
                Err(e) => anyhow::bail!("store push failed: {}", e),
            }
        }
    }

    Ok(())
}

/// `aida pull` — symmetric counterpart of `aida push`. Pulls both the
/// current code branch (via `git pull --ff-only`) and the orphan store
/// (via `git_ops::pull_rebase`, matching `aida db sync --pull`). Each
/// leg skips cleanly when its remote isn't configured, so the command
/// is safe to run in any project state. trace:TASK-43 | ai:claude
fn handle_pull_command(
    store_path: &std::path::Path,
    code_only: bool,
    store_only: bool,
) -> Result<()> {
    use aida_core::git_ops;

    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // ---- Code pull (current branch on the project repo) ----
    if !store_only {
        if !git_ops::has_remote(&project_root, "origin") {
            println!("  {} no `origin` remote — skipping code pull", "Note:".dimmed());
        } else {
            let branch = git_ops::current_branch(&project_root)
                .unwrap_or_else(|_| "HEAD".to_string());
            println!(
                "{} {} ← origin",
                "Pulling code".cyan().bold(),
                branch
            );
            // --ff-only refuses if the local branch has diverged from
            // origin (user has unpushed commits AND origin has new
            // commits). Safer default than --rebase for a working
            // branch — the user gets a clear error and can decide
            // whether to rebase, merge, or stash. Matches the task's
            // acceptance shape ("equivalent to `git pull --ff-only`").
            // trace:TASK-43 | ai:claude
            let res = std::process::Command::new("git")
                .arg("-C")
                .arg(&project_root)
                .args(["pull", "--ff-only", "origin", &branch])
                .status();
            match res {
                Ok(s) if s.success() => {
                    println!("  {}", "code pull complete".green());
                }
                Ok(s) => {
                    eprintln!(
                        "  {} git pull --ff-only exited with status {} — \
                         your branch may have diverged from origin/{}. Use \
                         `git pull --rebase` or `git pull --no-rebase` once \
                         you've decided how to reconcile.",
                        "Warning:".yellow().bold(),
                        s,
                        branch
                    );
                }
                Err(e) => anyhow::bail!("git pull failed: {}", e),
            }
        }
    }

    // ---- Store pull (orphan branch via pull_rebase) ----
    if !code_only {
        if !git_ops::is_git_repo(store_path) {
            println!("  {} no orphan worktree — skipping store pull", "Note:".dimmed());
            return Ok(());
        }
        if !git_ops::has_remote(store_path, "origin") {
            println!("  {} orphan store has no `origin` — skipping store pull", "Note:".dimmed());
            return Ok(());
        }
        // Mirror `aida db sync --pull`: commit any pending orphan
        // changes first, then pull --rebase. Without the pre-commit
        // step, rebase refuses on a dirty tree and leaves the user
        // half-pulled.
        if git_ops::has_changes(store_path).unwrap_or(false) {
            let _ = git_ops::add(store_path, &["."]);
            let _ = git_ops::commit(store_path, "chore: sync pending changes");
            println!("  Committed pending orphan changes before pull");
        }
        let branch = git_ops::current_branch(store_path)
            .unwrap_or_else(|_| "aida-store".to_string());
        println!("{} aida-store ← origin", "Pulling store".cyan().bold());
        match git_ops::pull_rebase(store_path, "origin", &branch) {
            Ok(()) => println!("  {}", "store pull complete".green()),
            Err(e) => {
                eprintln!(
                    "  {} {}\n  The orphan store may be mid-rebase. To recover:\n    \
                         cd {} && git rebase --abort\n  \
                     Then re-run `aida pull` or `aida db sync --pull`.",
                    "Warning:".yellow().bold(),
                    e,
                    store_path.display()
                );
            }
        }
    }

    Ok(())
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
                "  Branch aida-store: {} ahead of origin (run `aida push`)",
                a.to_string().yellow()
            ),
            (0, b) => println!(
                "  Branch aida-store: {} behind origin (run `aida db sync --pull`)",
                b.to_string().yellow()
            ),
            (a, b) => println!(
                "  Branch aida-store: {} ahead, {} behind (diverged — `aida db sync --pull` then `aida push`)",
                a.to_string().red(),
                b.to_string().red()
            ),
        }
        println!();
    }

    // Recent activity — top 5 most recently modified user-authored reqs.
    // META rows (AI prompt customization seeded by init) are excluded so a
    // brand-new project doesn't show "Recent activity: META-001..006" as
    // its entire feed. trace:BUG-30 | ai:claude
    let mut recent: Vec<_> = summaries
        .iter()
        .filter(|r| !r.req_type.eq_ignore_ascii_case("meta"))
        .cloned()
        .collect();
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
        println!("  (no user requirements yet — try `aida add --type vision --title \"...\"`)");
    }
    println!();

    // Scaffolding freshness — only useful for non-AIDA-self projects, since
    // AIDA's own .claude/ uses symlinks into aida-core/templates/ and can't
    // drift. The aida-self block below has its own template-symlink check.
    if !is_aida_repo(&project_root) {
        print_scaffolding_freshness(&project_root, &store, store_path);
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
fn print_scaffolding_freshness(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
    db_path: &std::path::Path,
) {
    use aida_core::scaffolding::{ScaffoldConfig, Scaffolder};

    // BUG-43: drive the scaffolder with the *actual* store and the
    // *actual* db_path, matching how init/scaffold-apply construct the
    // scaffolder. AIDA.md bakes both store-derived data (req count) and
    // db_path-derived data (`database_filename()`) into its content, so
    // any mismatch on either input falsely reports drift on a fresh
    // init. trace:BUG-43 | ai:claude
    let config = ScaffoldConfig::default();
    let mut scaffolder = Scaffolder::with_database(
        project_root.to_path_buf(),
        config,
        db_path.to_path_buf(),
    );
    let preview = scaffolder.preview(store);

    use aida_core::scaffolding::FileCategory;

    let mut total = 0usize;
    let mut present = 0usize;
    let mut matches = 0usize;
    // BUG-42: split drift by file category. Template-category drift is a
    // problem (AIDA-owned files shouldn't differ from embedded). Seed-
    // category drift is *expected* once the user customizes CLAUDE.md /
    // AGENTS.md — it's not really drift, it's their project. Reporting
    // them under the same STALE banner trains users to ignore the warning.
    // ManagedMerge sits with seed for now (user-owned post-init in v1).
    let mut template_drift: Vec<std::path::PathBuf> = Vec::new();
    let mut seed_drift: Vec<std::path::PathBuf> = Vec::new();

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
            continue;
        }
        match FileCategory::from_path(&artifact.path) {
            FileCategory::Template => template_drift.push(artifact.path.clone()),
            FileCategory::Seed | FileCategory::ManagedMerge => {
                seed_drift.push(artifact.path.clone());
            }
        }
    }

    // No scaffolding present at all — stay quiet (probably a non-aida project
    // that just happens to have a .aida/config.toml from somewhere unrelated).
    if present == 0 {
        return;
    }

    println!("{}", "─── Scaffolding ───".bold());
    println!("  Templates compared: {} total, {} present in project", total, present);
    if template_drift.is_empty() {
        println!(
            "  Status:             {} — all {} AIDA-owned file(s) match the embedded templates",
            "FRESH".green(),
            matches + seed_drift.len()
        );
    } else {
        println!(
            "  Status:             {} — {} AIDA-owned file(s) differ from the embedded templates",
            "STALE".yellow(),
            template_drift.len()
        );
        for path in template_drift.iter().take(5) {
            println!("    - {}", path.display());
        }
        if template_drift.len() > 5 {
            println!("    ... and {} more", template_drift.len() - 5);
        }
        println!(
            "  Refresh with:       {} (or `aida scaffold apply --dry-run` to preview)",
            "aida scaffold apply --force".cyan()
        );
    }
    if !seed_drift.is_empty() {
        // Seed customizations are expected — report them informationally
        // so the user knows their CLAUDE.md / AGENTS.md tweaks were
        // detected, but don't roll them into the STALE count.
        // trace:BUG-42 | ai:claude
        let label = format!(
            "  Customized:         {} user-owned file(s) (drift expected post-init): {}",
            seed_drift.len(),
            seed_drift
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("{}", label.dimmed());
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

/// Build time formatted in the user's local timezone — the banner is
/// human-facing so we render it where the user reads it, not in UTC.
/// On-disk fields (oplog, YAML created_at, etc.) stay UTC.
/// trace:feedback_local_time | ai:claude
fn build_time_iso() -> String {
    let secs: i64 = env!("AIDA_BUILD_UNIX_TIME").parse().unwrap_or(0);
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S %Z")
                .to_string()
        })
        .unwrap_or_else(|| "(unknown)".to_string())
}

/// One-line build banner for use in --version and status output:
///   "0.4.0 (built 2026-05-03 01:23:45 PDT, sha 866b050[+dirty])"
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
    diff: bool,
) -> Result<()> {
    // --target path: upgrade a specific binary, regardless of what's running.
    if let Some(target) = target {
        return upgrade_specific_binary(std::path::Path::new(target), check, version, yes);
    }

    let install = detect_install_method()?;

    // Developer build: don't try to upgrade ourselves; instead scan for
    // sibling installs and offer to upgrade them.
    if let InstallMethod::DeveloperBuild(_) = &install {
        return upgrade_dev_mode_sibling_scan(check, version, yes, diff);
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
fn upgrade_dev_mode_sibling_scan(
    check: bool,
    version: Option<&str>,
    yes: bool,
    diff: bool,
) -> Result<()> {
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
            print_unreleased_dev_hint(&exe, &target_tag, diff);
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
        print_unreleased_dev_hint(&exe, &target_tag, diff);
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
/// sitting in their repo. Pure hint; doesn't trigger any action. When
/// `with_diff` is true, also prints `git log --stat <tag>..HEAD` so the
/// user can vet what `aida dev patch` would ship.
fn print_unreleased_dev_hint(exe: &std::path::Path, target_tag: &str, with_diff: bool) {
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

    if with_diff {
        println!();
        println!("{}", format!("Unreleased commits ({}..HEAD):", latest).bold());
        println!("{}", "─".repeat(72));
        let log = std::process::Command::new("git")
            .args([
                "log",
                "--stat",
                "--no-decorate",
                "--pretty=format:%C(yellow)%h%Creset %s%n  %C(dim)%an, %ar%Creset",
                &format!("{}..HEAD", latest),
            ])
            .current_dir(&repo)
            .output();
        match log {
            Ok(out) if out.status.success() => {
                print!("{}", String::from_utf8_lossy(&out.stdout));
                if !out.stdout.ends_with(b"\n") {
                    println!();
                }
            }
            _ => {
                println!("  (could not read git log)");
            }
        }
        println!();
    } else {
        println!(
            "      Pass {} to see the unreleased commits before shipping.",
            "--diff".cyan()
        );
    }

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
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
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
        DbCommand::Path => {
            // trace:FR-1-076 | ai:claude
            println!("{}", requirements_path.display());
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
        DbCommand::Block { .. } => {
            println!(
                "{} Block commands are only available for git-backed stores. Use: aida --file <dir> db block ...",
                "!".yellow()
            );
        }
        DbCommand::RetireLegacyIds { .. } => {
            println!(
                "{} retire-legacy-ids only applies to git-backed stores.",
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

/// Bulk-import helper for git-canonical stores (FR-1-002): drains an iterator
/// of new requirements through a single `GitBackend::bulk_writer()` session
/// so one git commit covers the whole batch. Falls back to the legacy
/// `update_atomically` path for non-git backends (SQLite / YAML), since the
/// bulk-writer's optimization only applies to the git path.
/// trace:FR-1-002 | ai:claude
fn bulk_import_via_writer<I>(
    storage: &Storage,
    commit_subject: &str,
    reqs: I,
) -> Result<usize>
where
    I: IntoIterator<Item = Requirement>,
{
    let path = storage.path();
    if path.is_dir() {
        let backend = aida_core::GitBackend::new(path)?;
        let mut writer = backend.bulk_writer()?;
        for req in reqs {
            writer.add(req)?;
        }
        let n = writer.finish(commit_subject)?;
        return Ok(n);
    }

    // Non-git backend: legacy path (single update_atomically commit).
    let mut count = 0;
    let reqs: Vec<Requirement> = reqs.into_iter().collect();
    storage.update_atomically(|store| {
        for req in reqs {
            let type_prefix = store.get_type_prefix(&req.req_type);
            store.add_requirement_with_id(req, None, type_prefix.as_deref());
            count += 1;
        }
    })?;
    Ok(count)
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
            from_pos,
            to_pos,
            from_flag,
            to_flag,
            r#type,
            bidirectional,
            force_parent,
        } => {
            let from = from_pos.as_deref().or(from_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing FROM (positional or --from)"))?;
            let to = to_pos.as_deref().or(to_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing TO (positional or --to)"))?;
            add_relationship(storage, from, to, r#type, *bidirectional, *force_parent)?;
        }
        RelationshipCommand::Remove {
            from_pos,
            to_pos,
            from_flag,
            to_flag,
            r#type,
            bidirectional,
        } => {
            let from = from_pos.as_deref().or(from_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing FROM (positional or --from)"))?;
            let to = to_pos.as_deref().or(to_flag.as_deref())
                .ok_or_else(|| anyhow::anyhow!("missing TO (positional or --to)"))?;
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
    force_parent: bool,
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

    // BUG-64: terminal-status guard on the parent end of a parent/child
    // edge. Same logic as the git-canonical path. trace:BUG-64 | ai:claude
    if !force_parent {
        let parent_for_guard = match &rel_type {
            RelationshipType::Child => Some(&to_req),
            RelationshipType::Parent => Some(&from_req),
            _ => None,
        };
        if let Some(p) = parent_for_guard {
            if is_terminal_status(&p.status) {
                anyhow::bail!(
                    "parent {} is {} — adding new children to a closed parent is usually a mistake. \
                     Pass `--force-parent` to override.",
                    p.spec_id.as_deref().unwrap_or("?"),
                    p.status,
                );
            }
        }
    }

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
                RelationshipType::Parent => "is parent of".to_string(),
                RelationshipType::Child => "is child of".to_string(),
                RelationshipType::Duplicate => "is duplicate of".to_string(),
                RelationshipType::Verifies => "verifies".to_string(),
                RelationshipType::VerifiedBy => "is verified by".to_string(),
                RelationshipType::References => "references".to_string(),
                RelationshipType::Custom(name) => name.clone(),
            };

            // BUG-53: tag rejected targets so a dangling-looking edge is
            // recognizable as still-resolvable rather than removed.
            // trace:BUG-53 | ai:claude
            if matches!(target_req.status, RequirementStatus::Rejected) {
                println!(
                    "  {} {} {} ({}) - {}",
                    description.cyan(),
                    target_spec.yellow(),
                    "[REJECTED]".red().bold(),
                    target_req.id.to_string().dimmed(),
                    target_req.title
                );
            } else {
                println!(
                    "  {} {} ({}) - {}",
                    description.cyan(),
                    target_spec.yellow(),
                    target_req.id.to_string().dimmed(),
                    target_req.title
                );
            }
        } else {
            // BUG-53: shorten dangling target uuid + flag it as removed so
            // the line reads as a tombstone rather than a phantom.
            // trace:BUG-53 | ai:claude
            let uuid_str = relationship.target_id.to_string();
            let short = &uuid_str[..uuid_str.len().min(8)];
            println!(
                "  {} {} {}",
                relationship.rel_type.to_string().cyan(),
                short.dimmed(),
                "(removed — run `aida doctor verify-relationships --repair` to clean up)".red()
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

/// Result of the STORY-63 scope-fallback resolver.
/// trace:STORY-63 | ai:claude
struct ScopeFallback<'a> {
    /// Total number of Approved children of the scope (informational —
    /// we display this count in the rendered message).
    approved_count: usize,
    /// The chosen pick after priority + created_at sort.
    pick: &'a Requirement,
}

/// Find the highest-priority approved child of `lease.scope` that no
/// session is already mid-work on. Returns `None` when:
///   - the scope is a path-glob / free-form string we can't resolve to
///     a spec (no ancestry to walk)
///   - any child of the scope is already InProgress (don't pick a
///     parallel within the same EPIC)
///   - no child satisfies status=Approved + the role scope filter
///
/// Priority order: High > Medium > Low, then created_at ascending so
/// the oldest approved item wins ties (closest to the project's
/// implicit work order).
/// trace:STORY-63 | ai:claude
fn scope_fallback_pick<'a>(
    store: &'a RequirementsStore,
    lease: &SessionLease,
    role_scope: Option<&(Vec<String>, Option<String>)>,
) -> Option<ScopeFallback<'a>> {
    let scope_lc = lease.scope.to_ascii_lowercase();
    let scope_req = store.requirements.iter().find(|r| {
        let spec_match = r
            .spec_id
            .as_deref()
            .map(|s| s.to_ascii_lowercase() == scope_lc)
            .unwrap_or(false);
        let agreed_match = r
            .agreed_id
            .as_deref()
            .map(|s| s.to_ascii_lowercase() == scope_lc)
            .unwrap_or(false);
        spec_match || agreed_match
    })?;

    let child_ids: HashSet<Uuid> = scope_req
        .relationships
        .iter()
        .filter(|r| r.rel_type == RelationshipType::Parent)
        .map(|r| r.target_id)
        .collect();
    if child_ids.is_empty() {
        return None;
    }
    let children: Vec<&Requirement> = store
        .requirements
        .iter()
        .filter(|r| child_ids.contains(&r.id))
        .collect();

    // If anything under this scope is already in flight, the session's
    // attention belongs to that — don't suggest a second item.
    if children
        .iter()
        .any(|r| r.status == RequirementStatus::InProgress)
    {
        return None;
    }

    let approved_count = children
        .iter()
        .filter(|r| r.status == RequirementStatus::Approved)
        .count();

    let mut candidates: Vec<&Requirement> = children
        .iter()
        .copied()
        .filter(|r| r.status == RequirementStatus::Approved)
        .filter(|r| {
            // Apply the active role's tag/status scope filter if present
            // (mirrors the existing queue-next post-filter so the two
            // entry points behave consistently).
            let Some((scope_tags, scope_status)) = role_scope else {
                return true;
            };
            if let Some(want) = scope_status {
                if !format!("{}", r.status).eq_ignore_ascii_case(want)
                    && !format!("{:?}", r.status).eq_ignore_ascii_case(want)
                {
                    return false;
                }
            }
            for tag in scope_tags {
                if !r.tags.iter().any(|t| t == tag) {
                    return false;
                }
            }
            true
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| {
        priority_rank(&a.priority)
            .cmp(&priority_rank(&b.priority))
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    Some(ScopeFallback {
        approved_count,
        pick: candidates[0],
    })
}

/// Sort key for priority — lower rank wins (High = 0, Medium = 1,
/// Low = 2) so an ascending sort puts High first.
/// trace:STORY-63 | ai:claude
fn priority_rank(p: &RequirementPriority) -> u8 {
    match p {
        RequirementPriority::High => 0,
        RequirementPriority::Medium => 1,
        RequirementPriority::Low => 2,
    }
}

/// Render `root` and its descendants as an indented tree, two spaces per
/// level. Each node prints as `<status-glyph> <ID>  <Status>  <title>`.
/// Children are walked via rel_type:Parent edges (AIDA's
/// parent-points-at-child storage convention). Recursion stops at
/// `max_depth` (the root is depth 0). Cycles are guarded by a visited
/// set — defensive only; the data model shouldn't allow them.
/// trace:STORY-62 | ai:claude
fn render_tree(
    backend: &aida_core::CachedGitBackend,
    root: &Requirement,
    max_depth: usize,
) -> Result<()> {
    let mut visited: HashSet<Uuid> = HashSet::new();
    render_tree_node(backend, root, 0, max_depth, &mut visited)
}

fn render_tree_node(
    backend: &aida_core::CachedGitBackend,
    req: &Requirement,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<Uuid>,
) -> Result<()> {
    if !visited.insert(req.id) {
        return Ok(()); // already rendered — treat as cycle, skip silently
    }
    let indent = "  ".repeat(depth);
    let status = format!("{}", req.effective_status());
    // Glyph hint without emoji (CLAUDE.md house rule): two-state mark on
    // the most useful axis — completed vs everything else.
    let glyph = if status.eq_ignore_ascii_case("completed") {
        "✓".green().to_string()
    } else {
        "○".dimmed().to_string()
    };
    let id_label = req.display_id();
    println!(
        "{}{} {:<14} {:<12} {}",
        indent,
        glyph,
        id_label,
        status,
        req.title,
    );
    if depth >= max_depth {
        return Ok(());
    }
    // Collect direct children (Parent edges on this req point to children).
    let mut children: Vec<Requirement> = Vec::new();
    for rel in &req.relationships {
        if rel.rel_type == RelationshipType::Parent {
            if let Some(child) = backend.get_requirement(&rel.target_id)? {
                children.push(child);
            }
        }
    }
    // Stable order: by display_id ascending so the same tree prints the
    // same way across runs.
    children.sort_by(|a, b| a.display_id().cmp(&b.display_id()));
    for child in &children {
        render_tree_node(backend, child, depth + 1, max_depth, visited)?;
    }
    Ok(())
}

fn print_comment(comment: &Comment, indent: usize) {
    let indent_str = "  ".repeat(indent);
    println!();
    println!("{}{}:", indent_str, comment.id.to_string().yellow());
    let edited_marker = if comment.modified_at > comment.created_at {
        format!(
            " (edited {})",
            comment
                .modified_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
        )
        .dimmed()
        .to_string()
    } else {
        String::new()
    };
    println!(
        "{}  {} {} at {}{}",
        indent_str,
        "By:".dimmed(),
        comment.author.cyan(),
        comment
            .created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
            .dimmed(),
        edited_marker,
    );
    println!("{}  {}", indent_str, comment.content);

    if !comment.replies.is_empty() {
        for reply in &comment.replies {
            print_comment(reply, indent + 1);
        }
    }
}

/// Resolve a user-supplied comment identifier into a concrete Uuid by
/// matching against the requirement's comment tree. Accepts:
/// - a full UUID string (e.g., `019df478-7a34-7f92-8d46-b00e0d1eeda7`)
/// - a UUID prefix (e.g., `019df478`) — must uniquely match one comment
/// Returns an error on no-match or ambiguous-prefix.
/// trace:SPIKE-2 | ai:claude
fn resolve_comment_uuid(req: &aida_core::Requirement, query: &str) -> Result<Uuid> {
    if let Ok(parsed) = Uuid::parse_str(query) {
        // Verify the exact UUID exists in the tree, even if parse succeeded.
        if collect_comment_ids(&req.comments)
            .into_iter()
            .any(|id| id == parsed)
        {
            return Ok(parsed);
        }
        anyhow::bail!("No comment with id {} on this requirement", parsed);
    }

    let q = query.to_lowercase();
    let matches: Vec<Uuid> = collect_comment_ids(&req.comments)
        .into_iter()
        .filter(|id| id.to_string().to_lowercase().starts_with(&q))
        .collect();
    match matches.len() {
        0 => anyhow::bail!(
            "No comment matches '{}' on this requirement (use `aida comment list <REQ>` to see ids)",
            query
        ),
        1 => Ok(matches[0]),
        n => anyhow::bail!(
            "Ambiguous comment prefix '{}' — matches {} comments. Use a longer prefix.",
            query,
            n
        ),
    }
}

/// Walk the (potentially nested) comment tree and collect every comment id.
fn collect_comment_ids(comments: &[Comment]) -> Vec<Uuid> {
    let mut out = Vec::new();
    for c in comments {
        out.push(c.id);
        out.extend(collect_comment_ids(&c.replies));
    }
    out
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
            trace.created_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M"),
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
            // Find requirement by spec_id (case-insensitive — trace comments may be lowercase)
            if let Some(req) = store
                .requirements
                .iter_mut()
                .find(|r| r.spec_id.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(&req_id)))
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
            // Find requirement by spec_id (case-insensitive — commit refs may be lowercase)
            if let Some(req) = store
                .requirements
                .iter_mut()
                .find(|r| r.spec_id.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(&req_id)))
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

// ---------------------------------------------------------------------------
// `aida review` — review-workflow helpers (STORY-67)
// ---------------------------------------------------------------------------

/// Section headings the prompt-generator looks for in a requirement's
/// description. The first match (case-insensitive) wins; everything from
/// that heading until the next `## ` heading or end-of-string is the
/// extracted body.
/// trace:STORY-67 | ai:claude
const ACCEPTANCE_SECTION_HEADINGS: &[&str] =
    &["Acceptance", "Verify", "Test cases", "Tests", "Verification"];

/// Extract the acceptance-criteria body from a requirement description.
/// Returns the body text (without the heading line) when one of the
/// recognized headings is found; None otherwise so the caller can render
/// a "no acceptance criteria documented" placeholder instead of silently
/// emitting an empty section. trace:STORY-67 | ai:claude
fn extract_acceptance_section(description: &str) -> Option<String> {
    let lines: Vec<&str> = description.lines().collect();
    let mut start: Option<usize> = None;
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i].trim();
        if let Some(rest) = line.strip_prefix("## ") {
            let lower = rest.trim().to_ascii_lowercase();
            if ACCEPTANCE_SECTION_HEADINGS
                .iter()
                .any(|h| lower.starts_with(&h.to_ascii_lowercase()))
            {
                start = Some(i + 1);
                break;
            }
        }
        i += 1;
    }
    let start = start?;
    let mut end = lines.len();
    for (j, line) in lines.iter().enumerate().skip(start) {
        if line.trim_start().starts_with("## ") {
            end = j;
            break;
        }
    }
    let body = lines[start..end].join("\n").trim().to_string();
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

/// Pull `(REQ-ID)` trailers from a single commit message body. AIDA's
/// commit format wraps the requirement id in parens at end-of-subject:
/// `[AI:tool] feat(scope): description (REQ-ID)`. Also tolerates the
/// shorter form without `[AI:tool]` (chores/docs).
/// trace:STORY-67 | ai:claude
fn extract_spec_ids_from_commit(message: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in message.lines() {
        let trimmed = line.trim();
        // Walk until the LAST `(...)` group on the subject line, which
        // is where the REQ-ID lives. Body lines occasionally mention
        // other reqs in prose; we ignore those to avoid false matches.
        let Some(open_at) = trimmed.rfind('(') else { continue };
        let Some(close_at) = trimmed[open_at..].find(')').map(|n| open_at + n) else {
            continue;
        };
        let inner = &trimmed[open_at + 1..close_at];
        if looks_like_spec_id(inner) {
            out.push(inner.to_string());
        }
    }
    out
}

fn looks_like_spec_id(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 3 || s.len() > 40 {
        return false;
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i < 2 || i >= bytes.len() || bytes[i] != b'-' {
        return false;
    }
    i += 1;
    if i >= bytes.len() || !bytes[i].is_ascii_digit() {
        return false;
    }
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_digit() || b == b'-' {
            i += 1;
        } else {
            return false;
        }
    }
    true
}

/// trace:STORY-67 | ai:claude
fn handle_review_command(cmd: &ReviewCommand, storage: &Storage) -> Result<()> {
    match cmd {
        ReviewCommand::Prompt {
            specs,
            pr,
            forge,
            write,
        } => generate_review_prompt(storage, specs.as_deref(), *pr, forge.as_deref(), write.as_deref()),
    }
}

/// trace:STORY-67 | ai:claude
fn generate_review_prompt(
    storage: &Storage,
    specs_csv: Option<&str>,
    pr: Option<u64>,
    forge_override: Option<&str>,
    write_path: Option<&str>,
) -> Result<()> {
    let store = storage.load()?;

    // Resolve the spec list. Preference order: --specs explicit,
    // --pr range parse, error if neither.
    let (spec_ids, header_subtitle): (Vec<String>, String) = if let Some(csv) = specs_csv {
        let ids: Vec<String> = csv
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if ids.is_empty() {
            anyhow::bail!("--specs was empty after splitting on commas");
        }
        (ids.clone(), format!("Specs: {}", ids.join(", ")))
    } else if let Some(pr_n) = pr {
        let project_root = find_project_root()?;
        let forge = forge_override
            .and_then(ReviewForge::parse)
            .or_else(|| detect_forge_from_origin(&project_root))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "couldn't detect forge from origin URL — pass --forge github|gitlab"
                )
            })?;
        let (base, head) = pr_base_head(&project_root, forge, pr_n)?;
        let messages = git_log_messages(&project_root, &base, &head)?;
        let mut ids: Vec<String> = Vec::new();
        for msg in &messages {
            for id in extract_spec_ids_from_commit(msg) {
                if !ids.iter().any(|existing| existing.eq_ignore_ascii_case(&id)) {
                    ids.push(id);
                }
            }
        }
        if ids.is_empty() {
            anyhow::bail!(
                "no `(REQ-ID)` trailers found in {}..{} ({} commits inspected)",
                base,
                head,
                messages.len()
            );
        }
        let label = match forge {
            ReviewForge::GitHub => format!("PR #{} — branch `{}`", pr_n, head),
            ReviewForge::GitLab => format!("MR !{} — branch `{}`", pr_n, head),
        };
        (ids, label)
    } else {
        anyhow::bail!("pass --specs <CSV> or --pr <N>");
    };

    // Compose the markdown.
    let mut out = String::new();
    out.push_str("# Review Prompt\n\n");
    out.push_str(&header_subtitle);
    out.push_str("\n\n## What to verify\n\n");

    let mut missing: Vec<String> = Vec::new();
    for id in &spec_ids {
        let req = store
            .requirements
            .iter()
            .find(|r| r.spec_id.as_deref() == Some(id.as_str()))
            .or_else(|| {
                uuid::Uuid::parse_str(id)
                    .ok()
                    .and_then(|u| store.requirements.iter().find(|r| r.id == u))
            });
        let Some(req) = req else {
            out.push_str(&format!(
                "### {}\n\n_(not found in store)_\n\n",
                id
            ));
            missing.push(id.clone());
            continue;
        };
        out.push_str(&format!(
            "### {} — {}\n\n",
            req.spec_id.as_deref().unwrap_or(id.as_str()),
            req.title
        ));
        match extract_acceptance_section(&req.description) {
            Some(body) => {
                out.push_str(&body);
                out.push_str("\n\n");
            }
            None => {
                out.push_str("_(no `## Acceptance` / `## Verify` section in description — review against the requirement title and description.)_\n\n");
            }
        }
    }

    out.push_str("## Decide\n\n");
    out.push_str(
        "If every item above passes: approve and merge (`gh pr merge --squash` / \
         `glab mr merge --squash`), then mark each linked req `completed`.\n\n",
    );
    out.push_str(
        "If any item fails: request changes with specifics tied to the spec_id, \
         so the contributor can address them by id (not by paraphrase).\n",
    );

    if let Some(path) = write_path {
        std::fs::write(path, &out)
            .with_context(|| format!("failed to write review prompt to {}", path))?;
        eprintln!(
            "{} review prompt written to {} ({} spec{}{})",
            "✓".green().bold(),
            path,
            spec_ids.len(),
            if spec_ids.len() == 1 { "" } else { "s" },
            if missing.is_empty() {
                String::new()
            } else {
                format!(", {} missing in store", missing.len())
            },
        );
    } else {
        print!("{}", out);
    }
    Ok(())
}

/// trace:STORY-67 | ai:claude
fn pr_base_head(
    project_root: &std::path::Path,
    forge: ReviewForge,
    n: u64,
) -> Result<(String, String)> {
    // Use the forge CLI when available — it knows about fork PRs and
    // returns the resolved branch names. Fall back to a pure-git
    // approximation for environments without `gh`/`glab` (we fetch the
    // standard server-side ref and pretend base = current branch's
    // merge base with it; not perfect but better than failing).
    let (cli, args): (&str, &[&str]) = match forge {
        ReviewForge::GitHub => (
            "gh",
            &["pr", "view", "", "--json", "baseRefName,headRefName", "-q",
              ".baseRefName + \"\\t\" + .headRefName"],
        ),
        ReviewForge::GitLab => (
            "glab",
            &["mr", "view", "", "-F", "json"],
        ),
    };
    // The CLI invocation needs the PR number injected — clone args and
    // overwrite the empty placeholder.
    let mut args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    args_owned[2] = n.to_string();

    // gh / glab don't take `-C <path>` like git does — set the cwd via
    // `current_dir` so each tool runs against the right repo.
    // trace:STORY-67 | ai:claude
    let out = std::process::Command::new(cli)
        .current_dir(project_root)
        .args(&args_owned[..])
        .output();

    if let Ok(out) = out {
        if out.status.success() {
            match forge {
                ReviewForge::GitHub => {
                    let s = String::from_utf8_lossy(&out.stdout);
                    let parts: Vec<&str> = s.trim().split('\t').collect();
                    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
                        return Ok((parts[0].to_string(), parts[1].to_string()));
                    }
                }
                ReviewForge::GitLab => {
                    // Cheap parse: grep for the two fields rather than
                    // depending on serde_json — avoids a fresh dep just
                    // for this one path.
                    let s = String::from_utf8_lossy(&out.stdout).into_owned();
                    let base = json_string_field(&s, "target_branch");
                    let head = json_string_field(&s, "source_branch");
                    if let (Some(b), Some(h)) = (base, head) {
                        return Ok((b, h));
                    }
                }
            }
        }
    }

    // Fallback: pure-git. We don't know the contributor's base branch,
    // so we point at `main` (the most common case in this codebase) and
    // set `head` to the local review branch (`pr-N` / `mr-N`) the user
    // is presumed to have fetched via `aida session start --owns PR-N`
    // (STORY-61). Tell the user.
    eprintln!(
        "{} couldn't resolve PR base/head via {} — falling back to base=main, head={}-{}; \
         pass an explicit base via `git log <base>..<head>` if this is wrong.",
        "Note:".yellow().bold(),
        cli,
        if matches!(forge, ReviewForge::GitHub) { "pr" } else { "mr" },
        n
    );
    let head = match forge {
        ReviewForge::GitHub => format!("pr-{}", n),
        ReviewForge::GitLab => format!("mr-{}", n),
    };
    Ok(("main".to_string(), head))
}

/// Cheap "extract \"key\": \"value\"" JSON field grep. Only handles
/// string values without escapes — fine for branch names but not a
/// general parser. trace:STORY-67 | ai:claude
fn json_string_field(s: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", key);
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Run `git log <base>..<head> --pretty=format:%B%n--END--`. Returns
/// each commit message as a separate string. trace:STORY-67 | ai:claude
fn git_log_messages(
    project_root: &std::path::Path,
    base: &str,
    head: &str,
) -> Result<Vec<String>> {
    let range = format!("{}..{}", base, head);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", "--pretty=format:%B%n--END--", &range])
        .output()
        .with_context(|| format!("running git log {}", range))?;
    if !out.status.success() {
        anyhow::bail!(
            "git log {} failed: {}",
            range,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let messages: Vec<String> = stdout
        .split("--END--")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Ok(messages)
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

                // Load from embedded and write to disk WITH the AIDA-Generated
                // header so the file round-trips cleanly via `scaffold status`
                // (i.e. user can extract → cp into project → status reports
                // clean, instead of "modified" because the bare embedded bytes
                // have no header). trace:BUG-1-034 | ai:claude
                let mut temp_loader = TemplateLoader::new();
                if let Some(content) = temp_loader.load(key) {
                    let wrapped = aida_core::scaffolding::wrap_with_aida_header(
                        std::path::Path::new(key),
                        &content,
                    );
                    std::fs::write(&full_path, &wrapped)?;
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
        ScaffoldCommand::Upgrade {
            project_root,
            dry_run,
            force,
        } => {
            // trace:FR-1-028 | ai:claude
            let store = storage.load()?;
            let root = project_root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            if !root.exists() {
                anyhow::bail!("Project root does not exist: {}", root.display());
            }

            let mut scaffolder = aida_core::scaffolding::Scaffolder::with_database(
                root.clone(),
                ScaffoldConfig::default(),
                db_path.to_path_buf(),
            );
            let preview = scaffolder.preview(&store);
            run_scaffold_upgrade(&root, &preview, *dry_run, *force)?;
        }
        ScaffoldCommand::Diff {
            path,
            project_root,
            no_color,
            context,
            list,
        } => {
            // trace:FR-1-027 | ai:claude
            let store = storage.load()?;
            let root = project_root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            if !root.exists() {
                anyhow::bail!("Project root does not exist: {}", root.display());
            }

            let config = ScaffoldConfig::default();
            // Use with_database so our preview matches what `scaffold status`
            // produces — without the db_path the scaffolder renders CLAUDE.md
            // / AGENTS.md against legacy defaults, which then "drifts" against
            // a fresh `aida init` for purely cosmetic reasons.
            let mut scaffolder = aida_core::scaffolding::Scaffolder::with_database(
                root.clone(),
                config.clone(),
                db_path.to_path_buf(),
            );
            let preview = scaffolder.preview(&store);

            // Resolve which artifacts to diff. When `path` is given, restrict
            // to that one entry (error if not in the manifest); else walk all
            // artifacts and diff any whose on-disk content differs.
            // Exit codes per FR-1-027: 0=clean, 1=drift, 2=usage error.
            let targets: Vec<&aida_core::scaffolding::ScaffoldArtifact> = match path {
                Some(p) => {
                    let needle = p.clone();
                    match preview.artifacts.iter().find(|a| a.path == needle) {
                        Some(matched) => vec![matched],
                        None => {
                            eprintln!(
                                "Error: {} is not a scaffolded file (run `aida scaffold status` to see what is)",
                                needle.display()
                            );
                            std::process::exit(2);
                        }
                    }
                }
                None => preview.artifacts.iter().collect(),
            };

            let any_drift = print_scaffold_diffs(&root, &targets, *context, *no_color, *list)?;
            if any_drift {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

/// What `scaffold upgrade` should do for a single artifact, computed from
/// its category + drift state.
/// trace:FR-1-028 | ai:claude
enum UpgradeAction {
    /// File missing on disk — create it.
    Create,
    /// File exists, drifted — full overwrite. Templates by default;
    /// anything when `--force`.
    Overwrite,
    /// File exists with AIDA-AUTOGEN markers and the block content is
    /// drifted — rewrite just the marked block, preserve user content
    /// outside the markers.
    RewriteAidaBlock,
    /// CLAUDE.md exists but is missing the `@.claude/AIDA.md` import line.
    /// Insert it, preserving everything else.
    /// trace:BUG-1-065 | ai:claude
    InsertClaudeImport,
    /// ManagedMerge file with AIDA-owned slot drift — replace just the
    /// declared slots, preserve everything else verbatim. The `Vec`
    /// records what changed for the per-row UI.
    /// trace:FR-1-047 | ai:claude
    SlotMerge {
        changes: Vec<aida_core::SlotChange>,
        merged: serde_json::Value,
    },
    /// File exists, drifted, but the file is user-owned (Seed without
    /// markers) — log and skip.
    LeaveAlone,
    /// File exists and matches — silent count.
    None,
}

/// Pick an upgrade action for a managed-merge file by parsing both the
/// on-disk JSON and the AIDA-rendered template, then running them
/// through `slot_merge`. Falls back to `LeaveAlone` if either side fails
/// to parse — bad JSON should be a user-facing error from elsewhere
/// (e.g. `scaffold status` / `scaffold diff`), not a silent overwrite.
/// trace:FR-1-047 | ai:claude
fn decide_managed_merge(
    relative_path: &std::path::Path,
    on_disk_path: &std::path::Path,
    expected_content: &str,
) -> UpgradeAction {
    let actual_text = match std::fs::read_to_string(on_disk_path) {
        Ok(s) => s,
        Err(_) => return UpgradeAction::LeaveAlone,
    };
    let actual_json: serde_json::Value = match serde_json::from_str(&actual_text) {
        Ok(v) => v,
        Err(_) => return UpgradeAction::LeaveAlone,
    };
    let expected_json: serde_json::Value = match serde_json::from_str(expected_content) {
        Ok(v) => v,
        Err(_) => return UpgradeAction::LeaveAlone,
    };
    let slots = aida_core::scaffolding::slots_for_file(relative_path);
    let (merged, changes) = aida_core::scaffolding::slot_merge(&actual_json, &expected_json, slots);
    if changes.is_empty() {
        UpgradeAction::None
    } else {
        UpgradeAction::SlotMerge { changes, merged }
    }
}

/// Replace the content between `<!-- AIDA-AUTOGEN-BEGIN -->` and
/// `<!-- AIDA-AUTOGEN-END -->` in `actual` with the corresponding block
/// from `expected`. Preserves everything outside the markers verbatim.
/// Falls back to `actual` if either side is missing markers (defensive
/// — caller should only invoke this when both have markers).
/// trace:FR-1-028 | ai:claude
fn rewrite_aida_block(actual: &str, expected: &str) -> String {
    use aida_core::scaffolding::extract_aida_block;
    let Some(actual_block) = extract_aida_block(actual) else {
        return actual.to_string();
    };
    let Some(expected_block) = extract_aida_block(expected) else {
        return actual.to_string();
    };
    // Replace the first occurrence of the actual block with the expected
    // block. Both extract_aida_block returns are slices of the original
    // strings (between markers), so we can match-replace inside `actual`.
    actual.replacen(actual_block, expected_block, 1)
}

/// Mirror of `report.rs::file_matches_for_status`. Seeds (CLAUDE.md,
/// AGENTS.md) use marker-presence semantics — CLAUDE.md is matching if
/// the file exists at all; AGENTS.md only needs block-content comparison
/// when AIDA-AUTOGEN markers are present (user can opt out by removing
/// them). Templates and managed-merge use whole-content equality.
/// trace:FR-1-028 | ai:claude
fn file_matches_artifact(path: &std::path::Path, actual: &str, expected: &str) -> bool {
    use aida_core::FileCategory;
    match FileCategory::from_path(path) {
        FileCategory::Seed => {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            match name {
                // CLAUDE.md drift = missing the @.claude/AIDA.md import line.
                // Mirrors `seed_matches` in aida-core/src/report.rs.
                // trace:BUG-1-065 | ai:claude
                "CLAUDE.md" => aida_core::scaffolding::claude_md_has_import(actual),
                "AGENTS.md" => {
                    match aida_core::scaffolding::extract_aida_block(actual) {
                        // markers present → AIDA owns the block content
                        Some(a) => match aida_core::scaffolding::extract_aida_block(expected) {
                            Some(e) => a.trim() == e.trim(),
                            None => true,
                        },
                        // markers absent → user opted out, fully their file
                        None => true,
                    }
                }
                _ => actual.trim() == expected.trim(),
            }
        }
        FileCategory::Template => actual.trim() == expected.trim(),
        FileCategory::ManagedMerge => {
            // Slot-equality: parse both sides as JSON and compare just the
            // AIDA-owned slots. User keys outside the slots don't trigger
            // drift. Mirrors `report.rs::managed_merge_matches` and what
            // `scaffold upgrade` actually applies. trace:FR-1-047
            use serde_json::Value;
            let Ok(av): Result<Value, _> = serde_json::from_str(actual) else {
                return actual.trim() == expected.trim();
            };
            let Ok(ev): Result<Value, _> = serde_json::from_str(expected) else {
                return actual.trim() == expected.trim();
            };
            let slots = aida_core::scaffolding::slots_for_file(path);
            if slots.is_empty() {
                actual.trim() == expected.trim()
            } else {
                slots.iter().all(|s| av.pointer(s) == ev.pointer(s))
            }
        }
    }
}

/// Category-aware scaffold upgrade. For each artifact, decide what to do
/// based on its `FileCategory` and current drift state, then either
/// write or leave alone. Output is grouped by category with per-file
/// detail only for files that actually need attention or changed.
///
/// Strategies:
///   - Template + drifted/missing → overwrite/create
///   - Template + matching        → no-op (no message)
///   - Seed + missing             → create
///   - Seed + drifted             → leave alone (user owns; drift expected)
///   - Seed + matching            → no-op
///   - ManagedMerge + missing     → create
///   - ManagedMerge + drifted     → v1: leave alone with a "deferred" note
///   - ManagedMerge + matching    → no-op
///
/// `--force` overrides the per-category strategy and overwrites every
/// drifted file regardless of category (parity with `apply --force`,
/// just with cleaner output).
///
/// trace:FR-1-028 | ai:claude
fn run_scaffold_upgrade(
    project_root: &std::path::Path,
    preview: &aida_core::ScaffoldPreview,
    dry_run: bool,
    force: bool,
) -> Result<()> {
    use aida_core::FileCategory;
    use std::path::PathBuf;

    #[derive(Default)]
    struct CategoryStats {
        upgraded: Vec<PathBuf>,
        created: Vec<PathBuf>,
        left_alone: Vec<PathBuf>,
        unchanged: usize,
    }

    let mut by_cat: std::collections::BTreeMap<&str, CategoryStats> = std::collections::BTreeMap::new();

    for artifact in &preview.artifacts {
        let cat = artifact.category();
        let cat_label = cat.label();
        let stats = by_cat.entry(cat_label).or_default();

        let on_disk_path = project_root.join(&artifact.path);
        let exists = on_disk_path.exists();
        // Use content-equality directly rather than `artifact.file_status`
        // — there's a pre-existing bug in `check_file_status` where files
        // with YAML frontmatter (skills, commands) report Modified even
        // when they're byte-identical, because the header's stored
        // checksum is computed against the post-frontmatter body but the
        // expected checksum is computed against the full raw_content.
        // `aida scaffold status` already sidesteps this with content
        // equality; matching that behavior here. The underlying bug is
        // tracked separately so file_status can be made trustworthy.
        // trace:FR-1-028 | ai:claude
        let drifted = if exists {
            match std::fs::read_to_string(&on_disk_path) {
                Ok(actual) => !file_matches_artifact(&artifact.path, &actual, &artifact.content),
                Err(_) => true,
            }
        } else {
            // Missing isn't drift — handled separately as "create".
            false
        };

        // Decide action per category. Two special cases for v1.1+ work:
        // - AGENTS.md (Seed) with AIDA-AUTOGEN markers gets a block-only
        //   rewrite (FR-1-035) — preserves user content outside the
        //   block.
        // - Managed-merge files (settings.json, .mcp.json) with drift
        //   in any AIDA-owned slot get a slot-merge (FR-1-047) — replace
        //   only the declared slots, preserve every other key verbatim.
        let action = if !exists {
            UpgradeAction::Create
        } else if !drifted {
            UpgradeAction::None
        } else if force {
            UpgradeAction::Overwrite
        } else {
            match cat {
                FileCategory::Template => UpgradeAction::Overwrite,
                FileCategory::Seed => {
                    let name = artifact.path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    let actual_text = std::fs::read_to_string(&on_disk_path).ok();
                    match name {
                        "AGENTS.md" if actual_text
                            .as_deref()
                            .and_then(aida_core::scaffolding::extract_aida_block)
                            .is_some() =>
                        {
                            UpgradeAction::RewriteAidaBlock
                        }
                        "CLAUDE.md" if !actual_text
                            .as_deref()
                            .map(aida_core::scaffolding::claude_md_has_import)
                            .unwrap_or(true) =>
                        {
                            // The only AIDA-managed bit of CLAUDE.md is the
                            // import line; if it's missing we can fix that
                            // surgically without touching anything else.
                            // trace:BUG-1-065 | ai:claude
                            UpgradeAction::InsertClaudeImport
                        }
                        _ => UpgradeAction::LeaveAlone,
                    }
                }
                FileCategory::ManagedMerge => decide_managed_merge(
                    &artifact.path,
                    &on_disk_path,
                    &artifact.content,
                ),
            }
        };

        match action {
            UpgradeAction::Create => {
                if !dry_run {
                    if let Some(parent) = on_disk_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&on_disk_path, &artifact.content)?;
                }
                stats.created.push(artifact.path.clone());
            }
            UpgradeAction::Overwrite => {
                if !dry_run {
                    if let Some(parent) = on_disk_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&on_disk_path, &artifact.content)?;
                }
                stats.upgraded.push(artifact.path.clone());
            }
            UpgradeAction::RewriteAidaBlock => {
                if !dry_run {
                    let actual = std::fs::read_to_string(&on_disk_path)?;
                    let merged = rewrite_aida_block(&actual, &artifact.content);
                    std::fs::write(&on_disk_path, merged)?;
                }
                stats.upgraded.push(artifact.path.clone());
            }
            UpgradeAction::InsertClaudeImport => {
                // trace:BUG-1-065 | ai:claude
                if !dry_run {
                    let actual = std::fs::read_to_string(&on_disk_path)?;
                    let updated = aida_core::scaffolding::insert_claude_md_import(&actual);
                    std::fs::write(&on_disk_path, updated)?;
                }
                stats.upgraded.push(artifact.path.clone());
            }
            UpgradeAction::SlotMerge { changes, merged } => {
                // trace:FR-1-047 | ai:claude
                if !dry_run {
                    let pretty = serde_json::to_string_pretty(&merged)?;
                    std::fs::write(&on_disk_path, pretty + "\n")?;
                }
                stats.upgraded.push(artifact.path.clone());
                // Surface the per-slot diff inline since it's the most
                // useful signal for a managed-merge upgrade.
                for ch in &changes {
                    let kind = match ch.kind {
                        aida_core::SlotChangeKind::Replaced => "↑".cyan().to_string(),
                        aida_core::SlotChangeKind::Added => "+".green().to_string(),
                    };
                    eprintln!(
                        "      {}   {}: {} {}",
                        " ".repeat(0),
                        artifact.path.display().to_string().dimmed(),
                        kind,
                        ch.slot
                    );
                }
            }
            UpgradeAction::LeaveAlone => {
                stats.left_alone.push(artifact.path.clone());
            }
            UpgradeAction::None => {
                stats.unchanged += 1;
            }
        }
    }

    // Render. One block per category, in the same order as the SPIKE
    // doc + the FileCategory enum (template → seed → managed-merge).
    let order = ["template", "seed", "managed-merge"];
    let mut total_changes = 0usize;
    for cat in order {
        let Some(stats) = by_cat.get(cat) else { continue };
        let header = match cat {
            "template" => "Templates (AIDA-owned)".cyan().bold(),
            "seed" => "Seed (user-owned post-init)".yellow().bold(),
            "managed-merge" => "Managed-merge (slot-shared)".magenta().bold(),
            _ => cat.normal().bold(),
        };
        println!("\n{}", header);
        if !stats.created.is_empty() {
            println!("  {} {} created:", "+".green().bold(), stats.created.len());
            for p in &stats.created {
                println!("      + {}", p.display());
            }
            total_changes += stats.created.len();
        }
        if !stats.upgraded.is_empty() {
            let verb = if force { "overwritten" } else { "upgraded" };
            println!("  {} {} {}:", "↑".cyan().bold(), stats.upgraded.len(), verb);
            for p in &stats.upgraded {
                println!("      ↑ {}", p.display());
            }
            total_changes += stats.upgraded.len();
        }
        if !stats.left_alone.is_empty() {
            let why = match cat {
                "seed" => "user-owned; drift expected. Edit by hand or `apply --force`",
                "managed-merge" => "slot-merge deferred (FR-1-028 v2). Edit by hand or `apply --force`",
                _ => "left alone",
            };
            println!(
                "  {} {} drifted, left alone ({}):",
                "·".yellow(),
                stats.left_alone.len(),
                why
            );
            for p in &stats.left_alone {
                println!("      · {}", p.display());
            }
        }
        if stats.unchanged > 0 {
            println!(
                "  {} {} matching (no action needed)",
                "✓".green(),
                stats.unchanged
            );
        }
    }

    println!();
    if dry_run {
        println!(
            "{} Dry run — {} file(s) would change. Re-run without --dry-run to apply.",
            "→".cyan().bold(),
            total_changes
        );
    } else if total_changes == 0 {
        println!("{} Scaffold up to date — nothing to do.", "✓".green().bold());
    } else {
        println!(
            "{} {} file(s) changed.",
            "✓".green().bold(),
            total_changes
        );
    }
    Ok(())
}

/// Walk the resolved artifact set, diffing each against its on-disk copy.
/// Returns true if any drift was emitted (so the caller can set exit code).
/// Files that are missing on disk are reported as a single header + note,
/// not as a full diff (the unified-diff format isn't useful when actual is
/// empty / nonexistent — `aida scaffold status` already covers that case).
/// trace:FR-1-027 | ai:claude
fn print_scaffold_diffs(
    project_root: &std::path::Path,
    artifacts: &[&aida_core::scaffolding::ScaffoldArtifact],
    context_lines: usize,
    no_color: bool,
    list_only: bool,
) -> Result<bool> {
    use aida_core::DiffSlice;

    if no_color {
        colored::control::set_override(false);
    }

    let mut any_drift = false;
    let mut printed_count = 0;
    for artifact in artifacts {
        let full_path = project_root.join(&artifact.path);
        let actual_result = std::fs::read_to_string(&full_path);

        // Resolve drift state via the slice helper so CLAUDE.md (presence-
        // only) and AGENTS.md (AUTOGEN-block-only) get scoped properly.
        // Missing-file handling stays in this layer because the slice helper
        // doesn't see the filesystem. trace:FR-1-027 | ai:claude
        let slice = match &actual_result {
            Ok(actual) => aida_core::aida_managed_diff_slice(&artifact.path, &artifact.content, actual),
            Err(_) => {
                // Single-file mode: explicit user asked for this path → surface.
                // Bulk mode: only surface for known-required files (Template
                // category — AIDA owns those).
                let category = artifact.category();
                let surface = artifacts.len() == 1
                    || matches!(category, aida_core::FileCategory::Template);
                if !surface {
                    continue;
                }
                if list_only {
                    println!("{}", artifact.path.display());
                } else {
                    println!(
                        "{}",
                        format!("# {} is missing on disk", artifact.path.display())
                            .yellow()
                    );
                }
                any_drift = true;
                continue;
            }
        };

        match slice {
            DiffSlice::Match => continue,
            DiffSlice::MarkerMissing { message } => {
                any_drift = true;
                if list_only {
                    println!("{}", artifact.path.display());
                } else {
                    if printed_count > 0 {
                        println!();
                    }
                    printed_count += 1;
                    println!(
                        "{}",
                        format!("# {}: {}", artifact.path.display(), message).yellow()
                    );
                }
            }
            DiffSlice::FullDiff { expected, actual } => {
                any_drift = true;
                if list_only {
                    println!("{}", artifact.path.display());
                    continue;
                }
                if printed_count > 0 {
                    println!();
                }
                printed_count += 1;

                println!("{}", format!("--- a/{}  (template)", artifact.path.display()).red());
                println!("{}", format!("+++ b/{}  (on disk)", artifact.path.display()).green());
                render_unified_diff(&expected, &actual, context_lines);
            }
            DiffSlice::SliceDiff { expected, actual, note } => {
                any_drift = true;
                if list_only {
                    println!("{}", artifact.path.display());
                    continue;
                }
                if printed_count > 0 {
                    println!();
                }
                printed_count += 1;

                println!("{}", format!("--- a/{}  (template)", artifact.path.display()).red());
                println!("{}", format!("+++ b/{}  (on disk)", artifact.path.display()).green());
                println!("{}", format!("# {}", note).dimmed());
                render_unified_diff(&expected, &actual, context_lines);
            }
        }
    }

    if !any_drift && !list_only {
        eprintln!("{}", "No drift — scaffold matches on-disk files.".dimmed());
    }
    Ok(any_drift)
}

/// Render a unified diff of two strings to stdout with git-style coloring.
fn render_unified_diff(expected: &str, actual: &str, context_lines: usize) {
    use similar::{ChangeTag, TextDiff};
    let diff = TextDiff::configure()
        .algorithm(similar::Algorithm::Myers)
        .diff_lines(expected, actual);
    for hunk in diff.unified_diff().context_radius(context_lines).iter_hunks() {
        println!("{}", format!("{}", hunk.header()).cyan());
        for change in hunk.iter_changes() {
            let line = change.value();
            let line = line.strip_suffix('\n').unwrap_or(line);
            match change.tag() {
                ChangeTag::Delete => println!("{}", format!("-{}", line).red()),
                ChangeTag::Insert => println!("{}", format!("+{}", line).green()),
                ChangeTag::Equal => println!(" {}", line),
            }
        }
    }
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
            no_scope,
            global,
            local,
        } => {
            let user_id = get_user(user);
            let raw_entries = if *global {
                Vec::new()
            } else {
                storage.queue_list(&user_id, *include_completed)?
            };
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

            // Phase 3 scope: AND the active role's scope_tags / scope_status
            // on top of the role-routing filter. --all and --no-scope both
            // bypass; --all also bypasses role routing.
            // trace:TASK-1-021 | ai:claude
            let scope = if *all || *no_scope { None } else { active_role_scope() };

            // STORY-57: scope/session routing filter — when in a session,
            // an entry tagged for_scope=X is only visible if X matches the
            // active lease (or `--all` bypasses).
            let self_lease_for_routing: Option<SessionLease> = std::env::current_dir()
                .ok()
                .and_then(|cwd| {
                    let project_root = storage
                        .path()
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    active_lease_for_cwd(&project_root, &cwd)
                });

            let entries: Vec<&aida_core::QueueEntry> = raw_entries
                .iter()
                .filter(|e| match &role_filter {
                    Some(r) => e.for_role.as_deref() == Some(r.as_str()),
                    None => true,
                })
                .filter(|e| {
                    entry_scope_session_match(e, self_lease_for_routing.as_ref(), *all)
                })
                .filter(|e| {
                    let Some((scope_tags, scope_status)) = &scope else {
                        return true;
                    };
                    let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
                        return true;
                    };
                    if let Some(want) = scope_status {
                        if !format!("{}", req.status).eq_ignore_ascii_case(want)
                            && !format!("{:?}", req.status).eq_ignore_ascii_case(want)
                        {
                            return false;
                        }
                    }
                    for tag in scope_tags {
                        if !req.tags.iter().any(|t| t == tag) {
                            return false;
                        }
                    }
                    true
                })
                .collect();

            // Load global entries for the active role unless --local was passed.
            // The global queue is role-scoped (one file per role) — it only
            // makes sense when there's a role filter in effect. trace:FR-1-012
            let global_entries: Vec<global_queue::GlobalQueueEntry> = if *local {
                Vec::new()
            } else if let Some(role_name) = &role_filter {
                global_queue::load(role_name).unwrap_or_default()
            } else {
                Vec::new()
            };

            if entries.is_empty() && global_entries.is_empty() {
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

            let total = entries.len() + global_entries.len();
            let title = match &role_filter {
                Some(r) => format!(
                    "My Queue · role:{} ({} item{})",
                    r,
                    total,
                    if total == 1 { "" } else { "s" }
                ),
                None => format!(
                    "My Queue ({} item{})",
                    total,
                    if total == 1 { "" } else { "s" }
                ),
            };
            println!("{}", title.bold());
            println!("{}", "─".repeat(80));

            // Local-project name for tagging local entries when global is also
            // shown (so the user can tell at a glance which is which).
            let local_project_name = global_queue::project_name_for(storage.path().parent().unwrap_or(storage.path()));

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
                // STORY-57: inline routing tags. Show for: only when the
                // user isn't already filtering on a specific role (avoids
                // repeating "for:implementer" on every line in the
                // role-filtered view). Always show scope/session — those
                // are session-axis filters that don't get hoisted into
                // the title bar.
                if role_filter.is_none() {
                    if let Some(ref r) = entry.for_role {
                        print!("  {}", format!("[for:{}]", r).cyan());
                    }
                }
                if let Some(ref s) = entry.for_scope {
                    print!("  {}", format!("[@{}]", s).cyan());
                }
                if let Some(ref s) = entry.for_session {
                    let short = &s[..s.len().min(8)];
                    print!("  {}", format!("[session:{}]", short).cyan());
                }
                // When the global queue is also being shown, tag local entries
                // with their origin so the merge view is unambiguous.
                if !global_entries.is_empty() {
                    print!("  {}", format!("[origin:{}]", local_project_name).dimmed());
                }
                if let Some(ref note) = entry.note {
                    print!("  {}", format!("\"{}\"", note).dimmed().italic());
                }
                println!();
            }

            // Global entries follow the locals, numbered continuously.
            // We can't apply scope_tags / scope_status filters since we don't
            // have the foreign requirement loaded — surface them all and rely
            // on the cached spec_id/title in the entry. trace:FR-1-012
            for (idx, entry) in global_entries.iter().enumerate() {
                let i = entries.len() + idx;
                let spec_id = entry.spec_id.as_deref().unwrap_or("???");
                let title = entry.title.as_deref().unwrap_or("(no cached title)");

                print!(
                    "  {}. {} {}",
                    (i + 1).to_string().dimmed(),
                    spec_id.bold(),
                    title
                );
                if entry.added_by != user_id {
                    print!("  {}", format!("(from @{})", entry.added_by).dimmed());
                }
                if role_filter.is_none() {
                    print!("  {}", format!("[for:{}]", entry.for_role).cyan());
                }
                print!("  {}", format!("[origin:{}]", entry.project_name).dimmed());
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
            scope,
            for_session,
            no_scope,
            global,
        } => {
            let user_id = get_user(user);
            let store = storage.load()?;

            // Default routing: when no --for is given but the active session
            // has a role (AIDA_SESSION_ROLE), route to that role automatically.
            // Without this, `queue add X` produced an unrouted entry that
            // `queue next` (filtered to active role by default) wouldn't show
            // — surprising "queue is empty" right after queueing something.
            // Pass `--for any` to keep the unrouted behavior explicitly.
            // trace:BUG-18 | ai:claude
            let r#for: Option<String> = match r#for.as_deref() {
                Some("any") => None,
                Some(role) => Some(role.to_string()),
                None => std::env::var("AIDA_SESSION_ROLE")
                    .ok()
                    .filter(|s| !s.is_empty()),
            };

            // STORY-57: default scope routing. When adding inside a session
            // worktree without --scope or --no-scope, fill `for_scope` with
            // the active lease's scope so concurrent sessions sharing a
            // role don't see each other's work. --for-session is more
            // specific than --scope and overrides it for filtering, but we
            // keep both fields in the entry — the consumer side ANDs them.
            // trace:STORY-57 | ai:claude
            let active_lease_for_routing: Option<SessionLease> = std::env::current_dir()
                .ok()
                .and_then(|cwd| {
                    let project_root = storage
                        .path()
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    active_lease_for_cwd(&project_root, &cwd)
                });
            let for_scope_routing: Option<String> = if *no_scope {
                None
            } else if let Some(s) = scope.as_deref() {
                Some(s.to_string())
            } else if for_session.is_some() {
                // --for-session is the more specific axis; don't also
                // auto-add a scope filter unless the user asked for it.
                None
            } else {
                active_lease_for_routing.as_ref().map(|l| l.scope.clone())
            };
            let for_session_routing: Option<String> = for_session.clone();

            // Resolve requirement ID
            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store.get_requirement_by_spec_id(id)
            }
            .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

            let position = if *top {
                let entries = storage.queue_list(&user_id, true)?;
                entries.first().map(|e| e.position - 1000).unwrap_or(1000)
            } else {
                i64::MAX // sentinel: queue_add auto-assigns max+1000
            };

            let spec_id = req.spec_id.as_deref().unwrap_or("???");

            // --global routes to ~/.aida/queue/<role>.yaml. The role comes
            // from --for, falling back to the active role. Refuse silently
            // if neither is set — global queues only make sense role-scoped.
            // trace:FR-1-012 | ai:claude
            if *global {
                let role = r#for
                    .clone()
                    .or_else(|| std::env::var("AIDA_SESSION_ROLE").ok().filter(|s| !s.is_empty()))
                    .ok_or_else(|| anyhow::anyhow!(
                        "--global requires --for <role> or an active role (AIDA_SESSION_ROLE). \
                         The global queue is keyed by role."
                    ))?;
                let project_root = storage
                    .path()
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let project_root = project_root.canonicalize().unwrap_or(project_root);
                let project_name = global_queue::project_name_for(&project_root);
                let position = if *top {
                    let existing = global_queue::load(&role).unwrap_or_default();
                    existing.first().map(|e| e.position - 1000).unwrap_or(1000)
                } else {
                    i64::MAX
                };
                // Resolve i64::MAX to actual max+1000 inline (the local queue
                // path delegates this to the backend; we do it here ourselves).
                let position = if position == i64::MAX {
                    let existing = global_queue::load(&role).unwrap_or_default();
                    existing.iter().map(|e| e.position).max().unwrap_or(0) + 1000
                } else {
                    position
                };
                let gentry = global_queue::GlobalQueueEntry {
                    requirement_id: req.id,
                    project_root,
                    project_name: project_name.clone(),
                    spec_id: req.spec_id.clone(),
                    title: Some(req.title.clone()),
                    position,
                    added_by: user_id.clone(),
                    added_at: chrono::Utc::now(),
                    note: note.clone(),
                    for_role: role.clone(),
                };
                global_queue::add(&role, gentry)?;
                // BUG-65: bump role activity on queue-add so statusline
                // tracks "started caring about this spec" alongside the
                // existing edit/show/comment events.
                // trace:BUG-65 | ai:claude
                record_role_activity(spec_id, "queue-add");
                println!(
                    "{} Added {} ({}) to {} {}",
                    "✓".green(),
                    spec_id.bold(),
                    req.title,
                    "global queue".cyan(),
                    format!("[role:{}, origin:{}]", role, project_name).dimmed()
                );
                return Ok(());
            }

            let entry = aida_core::QueueEntry {
                user_id: user_id.clone(),
                requirement_id: req.id,
                position,
                added_by: user_id.clone(),
                note: note.clone(),
                added_at: chrono::Utc::now(),
                for_role: r#for.clone(),
                for_scope: for_scope_routing.clone(),
                for_session: for_session_routing.clone(),
            };
            storage.queue_add(entry)?;
            // BUG-65: bump role activity on queue-add so statusline tracks
            // "started caring about this spec" alongside edit/show/comment.
            // trace:BUG-65 | ai:claude
            record_role_activity(spec_id, "queue-add");

            // trace:STORY-57 | ai:claude
            let mut routing_parts: Vec<String> = Vec::new();
            if let Some(r) = &r#for {
                routing_parts.push(format!("for:{}", r));
            }
            if let Some(s) = &for_scope_routing {
                routing_parts.push(format!("@{}", s));
            }
            if let Some(s) = &for_session_routing {
                routing_parts.push(format!("session:{}", &s[..s.len().min(8)]));
            }
            let routing = if routing_parts.is_empty() {
                String::new()
            } else {
                format!(" [{}]", routing_parts.join(" ").cyan())
            };
            println!(
                "{} Added {} ({}) to queue{}",
                "✓".green(),
                spec_id.bold(),
                req.title,
                routing
            );
        }
        QueueCommand::Remove { id, user, global, r#for } => {
            let user_id = get_user(user);

            // --global removes from ~/.aida/queue/<role>.yaml. Role from
            // --for or AIDA_SESSION_ROLE. trace:FR-1-012 | ai:claude
            if *global {
                let role = r#for
                    .clone()
                    .or_else(|| std::env::var("AIDA_SESSION_ROLE").ok().filter(|s| !s.is_empty()))
                    .ok_or_else(|| anyhow::anyhow!(
                        "--global requires --for <role> or an active role (AIDA_SESSION_ROLE)."
                    ))?;
                // Match by spec_id from the global entries (no local store needed).
                let entries = global_queue::load(&role).unwrap_or_default();
                let target = entries.iter().find(|e| {
                    e.spec_id.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(id))
                        || uuid::Uuid::parse_str(id).map(|u| u == e.requirement_id).unwrap_or(false)
                });
                let Some(target) = target else {
                    anyhow::bail!("{} not found in global queue for role:{}", id, role);
                };
                let removed = global_queue::remove(
                    &role,
                    &target.requirement_id,
                    Some(&target.project_root),
                )?;
                if removed {
                    println!(
                        "{} Removed {} from global queue [role:{}, origin:{}]",
                        "✓".green(),
                        target.spec_id.as_deref().unwrap_or("???").bold(),
                        role,
                        target.project_name
                    );
                }
                return Ok(());
            }

            let store = storage.load()?;
            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store.get_requirement_by_spec_id(id)
            }
            .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

            storage.queue_remove(&user_id, &req.id)?;
            let spec_id = req.spec_id.as_deref().unwrap_or("???");
            println!("{} Removed {} from queue", "✓".green(), spec_id.bold());
        }
        QueueCommand::Move {
            id,
            top,
            bottom,
            before,
            after,
        } => {
            let user_id = std::env::var("AIDA_USER")
                .or_else(|_| std::env::var("USER"))
                .unwrap_or_else(|_| "default".to_string());
            let store = storage.load()?;

            let req = if let Ok(uuid) = uuid::Uuid::parse_str(id) {
                store.requirements.iter().find(|r| r.id == uuid)
            } else {
                store.get_requirement_by_spec_id(id)
            }
            .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

            let mut entries = storage.queue_list(&user_id, true)?;
            // STORY-72: queues created before the queue_add sentinel-fix
            // can have every entry at `position: i64::MAX`, which makes
            // any --before/--after/--top math unable to produce a
            // distinct-sorting result. Detect that state and lay down
            // properly-gapped positions (preserving display order) before
            // we compute the new slot. trace:STORY-72 | ai:claude
            if entries.iter().any(|e| e.position == i64::MAX) {
                let renumber: Vec<(uuid::Uuid, i64)> = entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| (e.requirement_id, (i as i64 + 1) * 1000))
                    .collect();
                storage.queue_reorder(&user_id, &renumber)?;
                entries = storage.queue_list(&user_id, true)?;
            }
            let new_position = if *top {
                entries.first().map(|e| e.position - 1000).unwrap_or(0)
            } else if *bottom {
                entries.last().map(|e| e.position + 1000).unwrap_or(1000)
            } else if let Some(ref before_id) = before {
                let before_req = if let Ok(uuid) = uuid::Uuid::parse_str(before_id) {
                    store.requirements.iter().find(|r| r.id == uuid)
                } else {
                    store.get_requirement_by_spec_id(before_id)
                }
                .ok_or_else(|| not_found::requirement_not_found(before_id, Some(storage.path())))?;
                if before_req.id == req.id {
                    anyhow::bail!("--before target is the same as the moved item");
                }
                entries
                    .iter()
                    .find(|e| e.requirement_id == before_req.id)
                    .ok_or_else(|| anyhow::anyhow!(
                        "{} is not in the queue", before_id
                    ))
                    .map(|e| e.position - 1)?
            } else if let Some(ref after_id) = after {
                // STORY-72: --after Y places X immediately after Y in the
                // queue. Symmetric to --before. Midpoint math against the
                // successor avoids the naive `Y.pos + 1` collision when the
                // queue happens to be densely-packed; falls back to
                // `Y.pos + 1000` when Y is at the bottom.
                // trace:STORY-72 | ai:claude
                let after_req = if let Ok(uuid) = uuid::Uuid::parse_str(after_id) {
                    store.requirements.iter().find(|r| r.id == uuid)
                } else {
                    store.get_requirement_by_spec_id(after_id)
                }
                .ok_or_else(|| not_found::requirement_not_found(after_id, Some(storage.path())))?;
                if after_req.id == req.id {
                    anyhow::bail!("--after target is the same as the moved item");
                }
                let anchor_pos = entries
                    .iter()
                    .find(|e| e.requirement_id == after_req.id)
                    .ok_or_else(|| anyhow::anyhow!(
                        "{} is not in the queue", after_id
                    ))?
                    .position;
                // Successor is the next entry strictly after the anchor in
                // sorted-by-position order. queue_list already returns sorted.
                let successor_pos = entries
                    .iter()
                    .find(|e| e.position > anchor_pos)
                    .map(|e| e.position);
                position_after(anchor_pos, successor_pos)
            } else {
                anyhow::bail!("Specify --top, --bottom, --before <ID>, or --after <ID>");
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
        QueueCommand::Next { role, all, user, no_scope, global, local } => {
            let user_id = get_user(user);
            let raw_entries = if *global {
                Vec::new()
            } else {
                storage.queue_list(&user_id, /* include_completed */ false)?
            };
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

            // Phase 3 scope filter (see queue list).
            // trace:TASK-1-021 | ai:claude
            let scope = if *all || *no_scope { None } else { active_role_scope() };

            // STORY-48: skip queue items whose target spec is owned by
            // another active session. Honors `[session].enforcement`:
            //   off   → no filtering
            //   warn  → entry is filtered out, but a stderr note explains
            //           why so the user isn't confused by an empty queue
            //   block → entry is filtered out silently (consistent with
            //           a hard "those specs aren't yours" stance)
            // trace:STORY-48 | ai:claude
            let project_root_for_leases = find_project_root().ok();
            let leases = project_root_for_leases
                .as_ref()
                .map(|p| list_leases(p))
                .unwrap_or_default();
            let self_lease = if leases.is_empty() {
                None
            } else {
                std::env::current_dir().ok().and_then(|cwd| {
                    project_root_for_leases
                        .as_ref()
                        .and_then(|root| active_lease_for_cwd(root, &cwd))
                })
            };
            let enforcement_mode = project_root_for_leases
                .as_ref()
                .map(|p| session_enforcement(p))
                .unwrap_or(SessionEnforcement::Warn);
            let lease_filter_active = !leases.is_empty()
                && enforcement_mode != SessionEnforcement::Off;
            let mut skipped_for_lease: Vec<(String, String)> = Vec::new();

            let next_entry = raw_entries
                .iter()
                .filter(|e| match &role_filter {
                    Some(r) => e.for_role.as_deref() == Some(r.as_str()),
                    None => true,
                })
                .filter(|e| {
                    // STORY-57: scope/session routing — only show items
                    // targeted at this session (or unrouted on that axis).
                    // --all bypasses; consistent with queue list.
                    entry_scope_session_match(e, self_lease.as_ref(), *all)
                })
                .filter(|e| {
                    if !lease_filter_active {
                        return true;
                    }
                    let Some(req) =
                        store.requirements.iter().find(|r| r.id == e.requirement_id)
                    else {
                        return true;
                    };
                    let owner = lease_owning_spec(
                        &leases,
                        self_lease.as_ref(),
                        req.id,
                        req.spec_id.as_deref(),
                        &store,
                    );
                    match owner {
                        None => true,
                        Some(o) => {
                            skipped_for_lease.push((
                                req.spec_id.clone().unwrap_or_else(|| "?".into()),
                                o.scope.clone(),
                            ));
                            false
                        }
                    }
                })
                .filter(|e| {
                    let Some((scope_tags, scope_status)) = &scope else {
                        return true;
                    };
                    let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
                        return true;
                    };
                    if let Some(want) = scope_status {
                        if !format!("{}", req.status).eq_ignore_ascii_case(want)
                            && !format!("{:?}", req.status).eq_ignore_ascii_case(want)
                        {
                            return false;
                        }
                    }
                    for tag in scope_tags {
                        if !req.tags.iter().any(|t| t == tag) {
                            return false;
                        }
                    }
                    true
                })
                .min_by_key(|e| e.position);

            // Local wins on tiebreak — the FR specifies that local-context
            // work takes precedence. Only fall through to global when local
            // is empty (or --global was passed). trace:FR-1-012
            let global_next: Option<global_queue::GlobalQueueEntry> = if *local || next_entry.is_some() {
                None
            } else if let Some(role_name) = &role_filter {
                let entries = global_queue::load(role_name).unwrap_or_default();
                entries.into_iter().min_by_key(|e| e.position)
            } else {
                None
            };

            if next_entry.is_none() && global_next.is_none() {
                // STORY-63: scope fallback. If the personal+global queues
                // are both empty AND we're inside a session lease, surface
                // the EPIC's approved children — `aida session start
                // --owns EPIC-X` should make picking work feel automatic,
                // not require pre-queueing every story.
                //
                // Rules:
                //   - resolve self_lease.scope to a Requirement (path-glob
                //     scopes don't qualify and fall through to the empty
                //     message)
                //   - if any child of that scope is already InProgress,
                //     don't auto-pick a parallel one — the session is
                //     already busy, even if the user got here looking for
                //     "what's next." Better to surface the in-flight item
                //     than start a second one
                //   - candidates: status=Approved, plus the active role's
                //     scope filter (tags/status) when set, same as the
                //     existing queue path
                //   - sort: priority High → Medium → Low, then created_at
                //     oldest-first as tiebreak
                // trace:STORY-63 | ai:claude
                if let Some(self_l) = self_lease.as_ref() {
                    if let Some(picked) = scope_fallback_pick(&store, self_l, scope.as_ref()) {
                        let approved_count = picked.approved_count;
                        let pick = picked.pick;
                        println!(
                            "{} {} has {} approved child(ren); picking {} {}",
                            "Queue empty —".dimmed(),
                            self_l.scope.cyan().bold(),
                            approved_count,
                            pick.spec_id.as_deref().unwrap_or("?").green().bold(),
                            "(scope fallback)".dimmed(),
                        );
                        println!();
                        println!("  {}: {}", "Title".bold(), pick.title);
                        println!("  {}: {}", "Status".bold(), pick.status);
                        println!("  {}: {}", "Priority".bold(), pick.priority);
                        if !pick.tags.is_empty() {
                            let mut tags: Vec<&String> = pick.tags.iter().collect();
                            tags.sort();
                            let tags_str = tags
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            println!("  {}: {}", "Tags".bold(), tags_str);
                        }
                        println!();
                        println!("{}", "Suggested:".dimmed());
                        let id_for_cmd = pick.spec_id.as_deref().unwrap_or("?");
                        println!(
                            "  aida show {}  &&  aida edit {} --status in-progress",
                            id_for_cmd, id_for_cmd
                        );
                        return Ok(());
                    }
                }

                let scope = match &role_filter {
                    Some(r) => format!(" for role {}", r.cyan()),
                    None => String::new(),
                };
                println!("{} Queue is empty{}.", "Nothing to do —".dimmed(), scope);
                // STORY-48: if the queue would have had items but they were
                // all owned by other sessions, name them — otherwise the
                // user sees an empty queue with no idea why.
                if !skipped_for_lease.is_empty() {
                    eprintln!();
                    eprintln!(
                        "{} {} item(s) skipped (owned by other sessions):",
                        "Note:".yellow().bold(),
                        skipped_for_lease.len()
                    );
                    for (spec, scope) in &skipped_for_lease {
                        eprintln!("  · {}  →  scope {}", spec.cyan(), scope.cyan());
                    }
                }
                // STORY-63: nudge toward `aida list --status approved` when
                // even the scope fallback came up empty, so the user has a
                // concrete next step rather than a dead-end.
                if self_lease.is_some() {
                    println!(
                        "  ({})",
                        "scope has no approved+ready children either — try `aida list --status approved`".dimmed()
                    );
                } else {
                    println!("  ({})", "pick up new work via `aida role enter dialog` or wait for items".dimmed());
                }
                return Ok(());
            }
            // STORY-48: surface non-fatal skips before rendering the next
            // item — useful when the active queue contains a mix of
            // in-scope and out-of-scope items.
            if !skipped_for_lease.is_empty() {
                eprintln!(
                    "{} {} other-session item(s) skipped (run `aida session leases` to see who)",
                    "Note:".dimmed(),
                    skipped_for_lease.len()
                );
            }

            // If only the global has an item, render it and return.
            if let Some(entry) = global_next {
                let spec_id = entry.spec_id.as_deref().unwrap_or("???");
                let title = entry.title.as_deref().unwrap_or("(no cached title)");
                println!("{}", "Next up:".bold());
                println!("  {}: {}", spec_id.green().bold(), title.bold());
                println!(
                    "  {} {}",
                    format!("[role:{}]", entry.for_role).cyan(),
                    format!("[origin:{}]", entry.project_name).dimmed()
                );
                if let Some(ref note) = entry.note {
                    println!("  Note: {}", note.italic());
                }
                println!();
                println!("{}", "Suggested:".dimmed());
                println!(
                    "  cd {}  &&  aida show {}",
                    entry.project_root.display().to_string().cyan(),
                    spec_id
                );
                return Ok(());
            }

            match next_entry {
                None => {
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
                store.get_requirement_by_spec_id(id)
            }
            .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

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
            // across SQLite and git-canonical modes. set_status_from_str
            // also clears any stale custom_status so the canonical enum
            // value actually takes effect (BUG-1-025).
            // trace:BUG-1-025 | ai:claude
            let req_id = req.id;
            storage.update_atomically(|s| {
                if let Some(r) = s.requirements.iter_mut().find(|r| r.id == req_id) {
                    r.set_status_from_str("Completed");
                    r.modified_at = chrono::Utc::now();
                }
            })?;
            storage.queue_remove(&user_id, &req_id)?;
            // BUG-65: queue done bypasses Command::Edit (sets status via
            // update_atomically directly), so the role activity log used
            // to miss it entirely — leaving statusline @SPEC stuck on the
            // last show/comment after every shipped spec. Bump explicitly
            // here so the most recently shipped spec wins.
            // trace:BUG-65 | ai:claude
            record_role_activity(spec_id, "done");

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
                .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

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

            // Bulk-writer path (FR-1-002): one commit per pull, no full-store
            // round-trip. trace:FR-1-002 | ai:claude
            let imported = bulk_import_via_writer(
                storage,
                "feat(jira)",
                to_import.iter().map(|issue| {
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
                    req
                }),
            )?;

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
                .ok_or_else(|| not_found::requirement_not_found(id, Some(storage.path())))?;

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

            // Import using the bulk-writer path (FR-1-002): one git commit
            // for the whole batch, no full-store load/iterate.
            // trace:FR-1-002 | ai:claude
            let imported = bulk_import_via_writer(
                storage,
                "feat(github)",
                to_import.iter().map(|issue| {
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
                    if issue.state == "closed" {
                        req.status = RequirementStatus::Completed;
                    }
                    for label in &issue.labels {
                        req.tags.insert(format!("gh:{}", label.name));
                    }
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
                    req
                }),
            )?;

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
                    r.spec_id.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(req_id))
                        || r.id.to_string() == *req_id
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
                        .with_timezone(&chrono::Local)
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
                    r.spec_id.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(req_id))
                        || r.id.to_string() == *req_id
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
