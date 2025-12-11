mod cli;
#[cfg(feature = "remote")]
mod client;
mod prompts;

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use std::collections::HashSet;
use uuid::Uuid;

use aida_core::{
    check_migration_status, check_scaffold_status, determine_requirements_path, export,
    get_registry_path, ArtifactType, Cardinality, Comment, FieldChange, FileStatus, IdFormat,
    MigrationCheck, NumberingStrategy, Registry, RelationshipDefinition, RelationshipType,
    ReportFormat, ReportGenerator, Requirement, RequirementPriority, RequirementStatus,
    RequirementType, RequirementsStore, ScaffoldConfig, Scaffolder, Storage, TraceLink,
};

use crate::cli::{
    Cli, Command, CommentCommand, ConfigCommand, DbCommand, FeatureCommand, RelDefCommand,
    RelationshipCommand, ReportCommand, ScaffoldCommand, ServerCommand, TraceCommand, TypeCommand,
};

fn main() -> Result<()> {
    let mut cli = Cli::parse();

    // Check for AIDA_SERVER environment variable if --server not specified
    if cli.server.is_none() {
        cli.server = std::env::var("AIDA_SERVER").ok();
    }

    // Determine which requirements file to use
    // trace:REQ-0231 | ai:claude:high
    let requirements_path = if let Some(ref explicit_file) = cli.file {
        // User explicitly specified a file path - use it directly
        std::path::PathBuf::from(explicit_file)
    } else {
        // Auto-detect: first find the base path, then check migration status
        let initial_path = determine_requirements_path(cli.project.as_deref())?;

        // Check for migration status (REQ-0231)
        // Storage class now auto-detects SQLite vs YAML by file extension
        match check_migration_status(&initial_path) {
            MigrationCheck::NoMigration(path) => path,
            MigrationCheck::MigratedToSqlite { yaml_path: _, sqlite_path } => {
                // YAML was officially migrated - use SQLite
                eprintln!(
                    "{}: Using SQLite database: {}",
                    "INFO".blue(),
                    sqlite_path.display()
                );
                sqlite_path
            }
            MigrationCheck::PossibleStaleYaml { yaml_path: _, sqlite_path } => {
                // Both exist but no marker - default to SQLite as it's likely more current
                eprintln!(
                    "{}: Both YAML and SQLite exist. Using SQLite.",
                    "INFO".blue()
                );
                eprintln!("Use --file requirements.yaml to use YAML instead.");
                sqlite_path
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
        Command::Export { format, output } => {
            handle_export_command(&storage, format, output.as_deref())?;
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
            handle_scaffold_command(scaffold_cmd, &storage)?;
        }
    }

    Ok(())
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
            ServerCommand::List { status, feature, limit } => {
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

    if let Some(owner_val) = owner {
        requirement.owner = owner_val.clone();
    }

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
            changes.push(Requirement::field_change("title", req.title.clone(), new_title.clone()));
            req.title = new_title.clone();
        }
    }

    // Update description
    if let Some(new_desc) = description {
        if new_desc != &req.description {
            changes.push(Requirement::field_change("description", req.description.clone(), new_desc.clone()));
            req.description = new_desc.clone();
        }
    }

    // Update status
    if let Some(status_str) = status {
        let new_status = match status_str.to_lowercase().as_str() {
            "draft" => RequirementStatus::Draft,
            "approved" => RequirementStatus::Approved,
            "completed" => RequirementStatus::Completed,
            "rejected" => RequirementStatus::Rejected,
            _ => anyhow::bail!("Invalid status '{}'. Use: draft, approved, completed, rejected", status_str),
        };
        if new_status != req.status {
            changes.push(Requirement::field_change("status", format!("{:?}", req.status), format!("{:?}", new_status)));
            req.status = new_status;
        }
    }

    // Update priority
    if let Some(priority_str) = priority {
        let new_priority = match priority_str.to_lowercase().as_str() {
            "high" => RequirementPriority::High,
            "medium" | "med" => RequirementPriority::Medium,
            "low" => RequirementPriority::Low,
            _ => anyhow::bail!("Invalid priority '{}'. Use: high, medium, low", priority_str),
        };
        if new_priority != req.priority {
            changes.push(Requirement::field_change("priority", format!("{:?}", req.priority), format!("{:?}", new_priority)));
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
            _ => anyhow::bail!("Invalid type '{}'. Use: functional, non-functional, system, user, bug, epic, story, task, spike, sprint, folder", type_str),
        };
        if new_type != req.req_type {
            changes.push(Requirement::field_change("type", format!("{:?}", req.req_type), format!("{:?}", new_type)));
            req.req_type = new_type;
        }
    }

    // Update owner
    if let Some(new_owner) = owner {
        if new_owner != &req.owner {
            changes.push(Requirement::field_change("owner", req.owner.clone(), new_owner.clone()));
            req.owner = new_owner.clone();
        }
    }

    // Update feature
    if let Some(new_feature) = feature {
        if new_feature != &req.feature {
            changes.push(Requirement::field_change("feature", req.feature.clone(), new_feature.clone()));
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
    println!("{} Updated {} ({} field(s) changed)", "✓".green(), spec_id, changes.len());

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
        DbCommand::Migrate { from, to, output, force } => {
            // trace:REQ-0231 | ai:claude:high
            let source_ext = match from.to_lowercase().as_str() {
                "yaml" | "yml" => "yaml",
                "sqlite" | "db" => "db",
                _ => {
                    println!("{} Invalid source format '{}'. Use 'yaml' or 'sqlite'.", "!".red(), from);
                    return Ok(());
                }
            };

            let target_ext = match to.to_lowercase().as_str() {
                "yaml" | "yml" => "yaml",
                "sqlite" | "db" => "db",
                _ => {
                    println!("{} Invalid target format '{}'. Use 'yaml' or 'sqlite'.", "!".red(), to);
                    return Ok(());
                }
            };

            if source_ext == target_ext {
                println!("{} Source and target formats are the same.", "!".yellow());
                return Ok(());
            }

            // Determine output path
            let target_path = output.clone().unwrap_or_else(|| {
                requirements_path.with_extension(target_ext)
            });

            // Check if target exists
            if target_path.exists() && !*force {
                println!(
                    "{} Target file '{}' already exists. Use --force to overwrite.",
                    "!".yellow(),
                    target_path.display()
                );
                return Ok(());
            }

            // Perform migration
            println!("Migrating from {} to {}...", requirements_path.display(), target_path.display());

            let count = if source_ext == "yaml" {
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
            // trace:REQ-0231 | ai:claude:high
            use aida_core::{BackendType, create_backend};

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

            if backend.backend_type() == BackendType::Sqlite {
                println!();
                println!("{}", "Concurrency Support".bold());
                println!("{}", "─".repeat(40));
                println!("Store Version:  {}", store.store_version);
                println!("WAL Mode:       Enabled (recommended for concurrent access)");
                println!("Optimistic Locking: Supported");
            }
        }
    }

    Ok(())
}

fn handle_export_command(
    storage: &Storage,
    format: &str,
    output: Option<&std::path::Path>,
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
        _ => {
            anyhow::bail!(
                "Unknown export format: {}. Supported formats: mapping, json, spec, impl",
                format
            );
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
            author,
            parent,
            interactive,
        } => {
            if *interactive || content.is_none() {
                add_comment_interactive(storage, id, author.as_deref(), parent.as_deref())?;
            } else {
                add_comment_cli(
                    storage,
                    id,
                    content.as_ref().unwrap(),
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
        inquire::Text::new("Author:").prompt()?
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

    let author = author.unwrap_or("Unknown").to_string();

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

    // Regex pattern for trace comments: // trace:REQ-ID | ai:tool:confidence
    let trace_pattern =
        regex::Regex::new(r"//\s*trace:([A-Z]+-\d+)(?:\s*\|\s*ai:(\w+):(\w+))?").unwrap();

    let mut found_traces: Vec<(String, String, String, u32, Option<String>, Option<String>)> =
        Vec::new();

    // Walk through files
    fn scan_dir(
        dir: &std::path::Path,
        ext_list: &[&str],
        pattern: &regex::Regex,
        found: &mut Vec<(String, String, String, u32, Option<String>, Option<String>)>,
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
        found: &mut Vec<(String, String, String, u32, Option<String>, Option<String>)>,
        verbose: bool,
    ) -> Result<()> {
        let file = fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);

        for (line_num, line) in reader.lines().enumerate() {
            let line = line?;
            if let Some(caps) = pattern.captures(&line) {
                let req_id = caps.get(1).map(|m| m.as_str().to_string()).unwrap();
                let tool = caps.get(2).map(|m| m.as_str().to_string());
                let confidence = caps.get(3).map(|m| m.as_str().to_string());

                if verbose {
                    println!(
                        "  Found: {} in {}:{}",
                        req_id.yellow(),
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
        for (_, file, _, line, tool, conf) in traces {
            let ai_info = match (tool, conf) {
                (Some(t), Some(c)) => format!(" [ai:{t}:{c}]").dimmed().to_string(),
                _ => String::new(),
            };
            println!("    {}:{}{}", file.cyan(), line, ai_info);
        }
    }

    if update {
        println!("\n{} Updating requirements database...", "→".blue());
        let mut store = storage.load()?;
        let mut added = 0;

        for (req_id, file_path, _, line_num, tool, confidence) in found_traces {
            // Find requirement by spec_id
            if let Some(req) = store
                .requirements
                .iter_mut()
                .find(|r| r.spec_id.as_deref() == Some(&req_id))
            {
                // Check if trace link already exists for this file and line
                let exists = req.trace_links.iter().any(|t| {
                    t.file_path == file_path && t.line_start == Some(line_num)
                });

                if !exists {
                    let mut trace = TraceLink::new(ArtifactType::SourceCode, file_path.clone());
                    trace.line_start = Some(line_num);
                    trace.created_by = Some("scan".to_string());
                    if let Some(t) = tool {
                        trace.notes = Some(format!("AI tool: {}", t));
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
        anyhow::bail!("git log failed: {}", String::from_utf8_lossy(&output.stderr));
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

            found_refs.push((
                commit_hash.to_string(),
                req_id,
                subject.to_string(),
            ));
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
fn handle_scaffold_command(cmd: &ScaffoldCommand, storage: &Storage) -> Result<()> {
    match cmd {
        ScaffoldCommand::Status {
            project_root,
            verbose,
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
            let status = check_scaffold_status(&store, &root, &config);

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
            let scaffolder = Scaffolder::new(root.clone(), config);
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
            let scaffolder = Scaffolder::new(root.clone(), config);

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
    }

    Ok(())
}
