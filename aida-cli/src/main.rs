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
    determine_requirements_path, export, get_registry_path, Cardinality, Comment, FieldChange,
    IdFormat, NumberingStrategy, Registry, RelationshipDefinition, RelationshipType, Requirement,
    RequirementPriority, RequirementStatus, RequirementType, RequirementsStore, Storage,
};

use crate::cli::{
    Cli, Command, CommentCommand, ConfigCommand, DbCommand, FeatureCommand, RelDefCommand,
    RelationshipCommand, ServerCommand, TypeCommand,
};

fn main() -> Result<()> {
    let mut cli = Cli::parse();

    // Check for AIDA_SERVER environment variable if --server not specified
    if cli.server.is_none() {
        cli.server = std::env::var("AIDA_SERVER").ok();
    }

    // Determine which requirements file to use
    let requirements_path = determine_requirements_path(cli.project.as_deref())?;
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
        Command::Edit { id } => {
            edit_requirement(&storage, id)?;
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

fn edit_requirement(storage: &Storage, id_str: &str) -> Result<()> {
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
