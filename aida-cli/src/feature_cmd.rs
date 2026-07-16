//! `aida feature` command cluster (feature-category management).
//!
//! Add / list / show / edit of feature definitions, plus the legacy
//! feature-name fallbacks read off the requirement set. Extracted verbatim
//! from `main.rs` (SPIKE-78); no behavior change.

use anyhow::Result;
use colored::Colorize;

use aida_core::Requirement;
use aida_core::Storage;

use crate::cli::FeatureCommand;

pub(crate) fn handle_feature_command(cmd: &FeatureCommand, storage: &Storage) -> Result<()> {
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
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
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
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
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
                println!(
                    "{} Feature updated successfully.",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
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
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
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
