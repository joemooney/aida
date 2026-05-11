use anyhow::Result;
use inquire::{Confirm, Select, Text};
use std::env;

use aida_core::{Requirement, RequirementPriority, RequirementStatus, RequirementType};

/// Prompts the user for a new requirement
pub fn prompt_new_requirement(store: &mut aida_core::RequirementsStore) -> Result<Requirement> {
    // Get basic information
    let title = Text::new("Title:").prompt()?;

    // Use the Editor type for multiline input
    let description = inquire::Editor::new("Description:").prompt()?;

    // Create a basic requirement
    let mut req = Requirement::new(title, description);

    // Get additional information with default values
    let status_options = vec![
        RequirementStatus::Draft,
        RequirementStatus::Approved,
        RequirementStatus::Planned,
        RequirementStatus::InProgress,
        RequirementStatus::Completed,
        RequirementStatus::Rejected,
    ];
    let status = Select::new("Status:", status_options).prompt()?;
    req.status = status;

    let priority_options = vec![
        RequirementPriority::High,
        RequirementPriority::Medium,
        RequirementPriority::Low,
    ];
    let priority = Select::new("Priority:", priority_options).prompt()?;
    req.priority = priority;

    let type_options = vec![
        RequirementType::Functional,
        RequirementType::NonFunctional,
        RequirementType::System,
        RequirementType::User,
        RequirementType::ChangeRequest,
        RequirementType::Bug,
        RequirementType::Epic,
        RequirementType::Story,
        RequirementType::Task,
        RequirementType::Spike,
    ];
    let req_type = Select::new("Type:", type_options).prompt()?;
    req.req_type = req_type;

    let owner = Text::new("Owner:").prompt()?;
    req.owner = owner;

    // Get existing features or create a new one
    let existing_features = store.get_feature_names();
    let default_feature = env::var("REQ_FEATURE").unwrap_or_else(|_| String::from("Uncategorized"));

    // Add option to create new feature
    let mut feature_options = vec!["Create new feature".to_string()];
    feature_options.extend(existing_features.clone());

    let feature_selection = Select::new("Feature:", feature_options).prompt()?;

    if feature_selection == "Create new feature" {
        // Prompt for new feature name
        let feature_prompt = format!("New feature name [{}]:", default_feature);
        let feature_input = Text::new(&feature_prompt).prompt()?;

        // Use input if provided, otherwise use the default
        let feature_name = if !feature_input.is_empty() {
            feature_input
        } else {
            default_feature
        };

        // Format the feature with a sequential number
        let next_number = store.get_next_feature_number();
        req.feature = format!("{}-{}", next_number, feature_name);
    } else {
        // Use the selected existing feature
        req.feature = feature_selection;
    }

    // Prompt for tags
    let add_tags = Confirm::new("Add tags?").prompt()?;
    if add_tags {
        let tags_input = Text::new("Tags (comma separated):").prompt()?;
        let tags = tags_input
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        req.tags = tags;
    }

    Ok(req)
}

/// Prompts the user for a new feature
pub fn prompt_new_feature(store: &mut aida_core::RequirementsStore) -> Result<String> {
    // Get feature name
    let name = Text::new("Feature name:").prompt()?;

    // Format the feature with a sequential number
    let next_number = store.get_next_feature_number();
    let formatted_name = format!("{}-{}", next_number, name);

    Ok(formatted_name)
}

/// Prompts the user for editing a feature
pub fn prompt_edit_feature(current_name: &str) -> Result<String> {
    // Get new feature name
    let prompt = format!("New feature name [{}]:", current_name);
    let new_name = Text::new(&prompt).prompt()?;

    if new_name.is_empty() {
        return Ok(current_name.to_string());
    }

    // Preserve the number prefix if it exists
    if let Some((prefix, _)) = current_name.split_once('-') {
        if prefix.parse::<u32>().is_ok() {
            return Ok(format!("{}-{}", prefix, new_name));
        }
    }

    Ok(new_name)
}
