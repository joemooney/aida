//! Prompt Templates for AI Operations
//!
//! This module builds structured prompts for various AI operations,
//! providing rich context from the requirements database.
//!
//! Prompts can be customized in three ways (checked in order):
//! 1. META requirements in database (MetaSubtype::Prompt with matching title)
//! 2. Custom templates in store.ai_prompts configuration
//! 3. Embedded default templates

use crate::meta::get_prompt_template;
use crate::models::{Requirement, RequirementsStore};

/// Build context about the project
pub fn build_project_context(store: &RequirementsStore) -> String {
    let total_reqs = store.requirements.len();
    let active_reqs = store.requirements.iter().filter(|r| !r.archived).count();

    let features: Vec<String> = store
        .features
        .iter()
        .map(|f| format!("{}-{}", f.number, f.name))
        .collect();

    let types: Vec<String> = store
        .type_definitions
        .iter()
        .map(|t| t.name.clone())
        .collect();

    format!(
        r#"## Project Context
- Project Name: {}
- Total Requirements: {} ({} active, {} archived)
- Features: {}
- Requirement Types: {}

## Lifecycle Vocabulary

AIDA's status lifecycle is: Draft → Approved → Planned → In Progress → Done → Completed (with Rejected as a separate terminal state for work that won't ship). Two states are commonly confused:

- **Done**: work finished on a branch but NOT yet merged to the default branch. Set by `aida queue done <id>` when the implementer finishes coding. PRs that are open or awaiting review sit at Done.
- **Completed**: merged to the default branch. Reached via the auto-bump scan in `aida pull` when a commit referencing the spec lands on main, OR manually via `aida edit <id> --status completed`.

There is also an off-mainline pause state, **Needs Attention**: an autonomous agent was working the spec (In Progress) but hit a design-fork it could not safely resolve and punted (`aida punt`) instead of guessing. It carries a structured reason and awaits human or advisor triage back to Approved / In Progress / Rejected.

When suggesting status transitions or generating child specs, prefer `Done` over `Completed` for "finished but not yet shipped" work, and only call something `Completed` when it has actually merged. trace:STORY-86, TASK-215, STORY-332 | ai:claude"#,
        if store.title.is_empty() {
            &store.name
        } else {
            &store.title
        },
        total_reqs,
        active_reqs,
        total_reqs - active_reqs,
        if features.is_empty() {
            "None defined".to_string()
        } else {
            features.join(", ")
        },
        if types.is_empty() {
            "Functional, NonFunctional, System, User, Epic, Story, Task".to_string()
        } else {
            types.join(", ")
        }
    )
}

/// Build context about related requirements
fn build_related_context(req: &Requirement, store: &RequirementsStore) -> String {
    let mut related = Vec::new();

    // Find parent requirements
    for rel in &req.relationships {
        if rel.rel_type.to_string() == "child" || rel.rel_type.to_string() == "parent" {
            if let Some(target) = store.requirements.iter().find(|r| r.id == rel.target_id) {
                related.push(format!(
                    "- {} ({}): {} [{}]",
                    target.spec_id.as_deref().unwrap_or("?"),
                    rel.rel_type,
                    target.title,
                    target.status
                ));
            }
        }
    }

    // Find requirements with same feature
    let same_feature: Vec<String> = store
        .requirements
        .iter()
        .filter(|r| r.id != req.id && r.feature == req.feature && !r.archived)
        .take(5)
        .map(|r| {
            format!(
                "- {} [{}]: {}",
                r.spec_id.as_deref().unwrap_or("?"),
                r.req_type,
                r.title
            )
        })
        .collect();

    let mut context = String::new();

    if !related.is_empty() {
        context.push_str("## Related Requirements\n");
        context.push_str(&related.join("\n"));
        context.push('\n');
    }

    if !same_feature.is_empty() {
        context.push_str("\n## Other Requirements in Same Feature\n");
        context.push_str(&same_feature.join("\n"));
        context.push('\n');
    }

    context
}

/// Build the current requirement as JSON context
fn requirement_to_context(req: &Requirement) -> String {
    format!(
        r#"## Current Requirement
- SPEC-ID: {}
- Title: {}
- Type: {}
- Status: {}
- Priority: {}
- Feature: {}
- Owner: {}
- Tags: {}

### Description
{}

### Existing Relationships
{}"#,
        req.spec_id.as_deref().unwrap_or("(not assigned)"),
        req.title,
        req.req_type,
        req.status,
        req.priority,
        req.feature,
        if req.owner.is_empty() {
            "(none)"
        } else {
            &req.owner
        },
        if req.tags.is_empty() {
            "(none)".to_string()
        } else {
            req.tags.iter().cloned().collect::<Vec<_>>().join(", ")
        },
        if req.description.is_empty() {
            "(no description provided)"
        } else {
            &req.description
        },
        if req.relationships.is_empty() {
            "(none)".to_string()
        } else {
            req.relationships
                .iter()
                .map(|r| format!("- {}: {}", r.rel_type, r.target_id))
                .collect::<Vec<_>>()
                .join("\n")
        }
    )
}

/// Build all requirements summary for duplicate detection
pub fn build_requirements_summary(store: &RequirementsStore, exclude_id: uuid::Uuid) -> String {
    let summaries: Vec<String> = store
        .requirements
        .iter()
        .filter(|r| r.id != exclude_id && !r.archived)
        .map(|r| {
            format!(
                "- {}: {} [{}] - {}",
                r.spec_id.as_deref().unwrap_or("?"),
                r.title,
                r.req_type,
                if r.description.len() > 100 {
                    format!("{}...", &r.description[..100])
                } else if r.description.is_empty() {
                    "(no description)".to_string()
                } else {
                    r.description.clone()
                }
            )
        })
        .collect();

    format!(
        "## All Requirements (for comparison)\n{}",
        summaries.join("\n")
    )
}

/// Build a full requirements summary for chat context (no exclusions, includes status/priority/dates)
pub fn build_all_requirements_summary(store: &RequirementsStore) -> String {
    let summaries: Vec<String> = store
        .requirements
        .iter()
        .filter(|r| !r.archived)
        .map(|r| {
            let desc = if r.description.len() > 120 {
                format!("{}...", &r.description[..120])
            } else if r.description.is_empty() {
                "(no description)".to_string()
            } else {
                r.description.clone()
            };
            let owner = if r.owner.is_empty() { "" } else { &r.owner };
            format!(
                "- {} [{}|{}|{}] created:{} modified:{}{}: {} — {}",
                r.spec_id.as_deref().unwrap_or("?"),
                r.req_type,
                r.status,
                r.priority,
                r.created_at.format("%Y-%m-%d"),
                r.modified_at.format("%Y-%m-%d"),
                if owner.is_empty() {
                    String::new()
                } else {
                    format!(" owner:{}", owner)
                },
                r.title,
                desc,
            )
        })
        .collect();

    format!(
        "## All Requirements ({} active)\nToday's date is {}.\n{}",
        summaries.len(),
        chrono::Utc::now().format("%Y-%m-%d"),
        summaries.join("\n")
    )
}

/// Build prompt for evaluating a requirement
pub fn build_evaluation_prompt(req: &Requirement, store: &RequirementsStore) -> String {
    let project_context = build_project_context(store);
    let req_context = requirement_to_context(req);
    let related_context = build_related_context(req, store);
    let config = &store.ai_prompts;
    let req_type = req.req_type.to_string();

    // Build substitution values
    let global_context_section = if !config.global_context.is_empty() {
        format!("\n## Project-Specific Context\n{}\n", config.global_context)
    } else {
        String::new()
    };

    let additional_instructions = if !config.evaluation.additional_instructions.is_empty() {
        format!(
            "\n## Additional Instructions\n{}\n",
            config.evaluation.additional_instructions
        )
    } else {
        String::new()
    };

    let type_extra = config
        .get_type_evaluation_extra(&req_type)
        .map(|s| format!("\n## Type-Specific Instructions ({})\n{}\n", req_type, s))
        .unwrap_or_default();

    // Check for custom template in store.ai_prompts config first
    if let Some(custom_template) = &config.evaluation.custom_template {
        return custom_template
            .replace("{project_context}", &project_context)
            .replace("{req_context}", &req_context)
            .replace("{related_context}", &related_context)
            .replace("{global_context}", &global_context_section)
            .replace("{additional_instructions}", &additional_instructions)
            .replace("{type_extra}", &type_extra)
            .replace("{req_type}", &req_type);
    }

    // Check for META prompt in database, fall back to embedded default
    let template = get_prompt_template(store, "Evaluate Requirement");

    // Apply substitutions to the template
    template
        .replace("{project_context}", &project_context)
        .replace("{req_context}", &req_context)
        .replace("{related_context}", &related_context)
        .replace("{global_context}", &global_context_section)
        .replace("{additional_instructions}", &additional_instructions)
        .replace("{type_extra}", &type_extra)
        .replace("{req_type}", &req_type)
}

/// Build prompt for finding duplicates
pub fn build_duplicates_prompt(req: &Requirement, store: &RequirementsStore) -> String {
    let project_context = build_project_context(store);
    let req_context = requirement_to_context(req);
    let all_reqs = build_requirements_summary(store, req.id);
    let config = &store.ai_prompts;
    let req_type = req.req_type.to_string();

    // Build substitution values
    let global_context_section = if !config.global_context.is_empty() {
        format!("\n## Project-Specific Context\n{}\n", config.global_context)
    } else {
        String::new()
    };

    let additional_instructions = if !config.duplicates.additional_instructions.is_empty() {
        format!(
            "\n## Additional Instructions\n{}\n",
            config.duplicates.additional_instructions
        )
    } else {
        String::new()
    };

    // Check for custom template in store.ai_prompts config first
    if let Some(custom_template) = &config.duplicates.custom_template {
        return custom_template
            .replace("{project_context}", &project_context)
            .replace("{req_context}", &req_context)
            .replace("{all_reqs}", &all_reqs)
            .replace("{global_context}", &global_context_section)
            .replace("{additional_instructions}", &additional_instructions)
            .replace("{req_type}", &req_type);
    }

    // Check for META prompt in database, fall back to embedded default
    let template = get_prompt_template(store, "Find Duplicates");

    // Apply substitutions to the template
    template
        .replace("{project_context}", &project_context)
        .replace("{req_context}", &req_context)
        .replace("{all_reqs}", &all_reqs)
        .replace("{global_context}", &global_context_section)
        .replace("{additional_instructions}", &additional_instructions)
        .replace("{req_type}", &req_type)
}

/// Build prompt for suggesting relationships
pub fn build_relationships_prompt(req: &Requirement, store: &RequirementsStore) -> String {
    let project_context = build_project_context(store);
    let req_context = requirement_to_context(req);
    let all_reqs = build_requirements_summary(store, req.id);
    let config = &store.ai_prompts;
    let req_type = req.req_type.to_string();

    // Get relationship type definitions
    let rel_types: Vec<String> = store
        .relationship_definitions
        .iter()
        .map(|rd| format!("{}: {}", rd.name, rd.description))
        .collect();

    let rel_types_str = if rel_types.is_empty() {
        "parent, child, duplicate, verifies, verified_by, references".to_string()
    } else {
        rel_types.join("\n- ")
    };

    // Build substitution values
    let global_context_section = if !config.global_context.is_empty() {
        format!("\n## Project-Specific Context\n{}\n", config.global_context)
    } else {
        String::new()
    };

    let additional_instructions = if !config.relationships.additional_instructions.is_empty() {
        format!(
            "\n## Additional Instructions\n{}\n",
            config.relationships.additional_instructions
        )
    } else {
        String::new()
    };

    // Check for custom template in store.ai_prompts config first
    if let Some(custom_template) = &config.relationships.custom_template {
        return custom_template
            .replace("{project_context}", &project_context)
            .replace("{req_context}", &req_context)
            .replace("{all_reqs}", &all_reqs)
            .replace("{rel_types}", &rel_types_str)
            .replace("{global_context}", &global_context_section)
            .replace("{additional_instructions}", &additional_instructions)
            .replace("{req_type}", &req_type);
    }

    // Check for META prompt in database, fall back to embedded default
    let template = get_prompt_template(store, "Suggest Relationships");

    // Apply substitutions to the template
    template
        .replace("{project_context}", &project_context)
        .replace("{req_context}", &req_context)
        .replace("{all_reqs}", &all_reqs)
        .replace("{rel_types}", &rel_types_str)
        .replace("{global_context}", &global_context_section)
        .replace("{additional_instructions}", &additional_instructions)
        .replace("{req_type}", &req_type)
}

/// Build prompt for improving description
pub fn build_improve_prompt(req: &Requirement, store: &RequirementsStore) -> String {
    let project_context = build_project_context(store);
    let req_context = requirement_to_context(req);
    let related_context = build_related_context(req, store);
    let config = &store.ai_prompts;
    let req_type = req.req_type.to_string();

    // Find examples of well-written requirements (mature ones with
    // descriptions). STORY-86: `Done` is the new "work finished on a
    // branch" state and qualifies as mature for example-extraction
    // purposes — no reason to wait for the merge before using a
    // spec as a few-shot prompt.
    let examples: Vec<String> = store
        .requirements
        .iter()
        .filter(|r| {
            r.id != req.id
                && !r.archived
                && r.description.len() > 100
                && matches!(r.status.to_string().as_str(), "Done" | "Completed")
        })
        .take(2)
        .map(|r| {
            format!(
                "### Example: {} ({})\n{}",
                r.title,
                r.req_type,
                &r.description[..r.description.len().min(300)]
            )
        })
        .collect();

    let examples_str = if examples.is_empty() {
        String::new()
    } else {
        format!(
            "## Examples of Well-Written Requirements\n{}",
            examples.join("\n\n")
        )
    };

    // Build substitution values
    let global_context_section = if !config.global_context.is_empty() {
        format!("\n## Project-Specific Context\n{}\n", config.global_context)
    } else {
        String::new()
    };

    let additional_instructions = if !config.improve.additional_instructions.is_empty() {
        format!(
            "\n## Additional Instructions\n{}\n",
            config.improve.additional_instructions
        )
    } else {
        String::new()
    };

    let type_extra = config
        .get_type_improve_extra(&req_type)
        .map(|s| format!("\n## Type-Specific Instructions ({})\n{}\n", req_type, s))
        .unwrap_or_default();

    // Check for custom template in store.ai_prompts config first
    if let Some(custom_template) = &config.improve.custom_template {
        return custom_template
            .replace("{project_context}", &project_context)
            .replace("{req_context}", &req_context)
            .replace("{related_context}", &related_context)
            .replace("{examples}", &examples_str)
            .replace("{global_context}", &global_context_section)
            .replace("{additional_instructions}", &additional_instructions)
            .replace("{type_extra}", &type_extra)
            .replace("{req_type}", &req_type);
    }

    // Check for META prompt in database, fall back to embedded default
    let template = get_prompt_template(store, "Improve Description");

    // Apply substitutions to the template
    template
        .replace("{project_context}", &project_context)
        .replace("{req_context}", &req_context)
        .replace("{related_context}", &related_context)
        .replace("{examples}", &examples_str)
        .replace("{global_context}", &global_context_section)
        .replace("{additional_instructions}", &additional_instructions)
        .replace("{type_extra}", &type_extra)
        .replace("{req_type}", &req_type)
}

/// Build prompt for generating child requirements
pub fn build_generate_children_prompt(req: &Requirement, store: &RequirementsStore) -> String {
    let project_context = build_project_context(store);
    let req_context = requirement_to_context(req);
    let config = &store.ai_prompts;
    let req_type = req.req_type.to_string();

    // Find existing children
    let existing_children: Vec<String> = req
        .relationships
        .iter()
        .filter(|r| r.rel_type.to_string() == "parent")
        .filter_map(|r| store.requirements.iter().find(|req| req.id == r.target_id))
        .map(|r| format!("- {}: {}", r.spec_id.as_deref().unwrap_or("?"), r.title))
        .collect();

    let existing_str = if existing_children.is_empty() {
        "(none yet)".to_string()
    } else {
        existing_children.join("\n")
    };

    // Get available types
    let types: Vec<String> = store
        .type_definitions
        .iter()
        .map(|t| t.name.clone())
        .collect();

    let types_str = if types.is_empty() {
        "Functional, NonFunctional, System, User, Task, Story".to_string()
    } else {
        types.join(", ")
    };

    // Build substitution values
    let global_context_section = if !config.global_context.is_empty() {
        format!("\n## Project-Specific Context\n{}\n", config.global_context)
    } else {
        String::new()
    };

    let additional_instructions = if !config.generate_children.additional_instructions.is_empty() {
        format!(
            "\n## Additional Instructions\n{}\n",
            config.generate_children.additional_instructions
        )
    } else {
        String::new()
    };

    let type_extra = config
        .get_type_generate_children_extra(&req_type)
        .map(|s| format!("\n## Type-Specific Instructions ({})\n{}\n", req_type, s))
        .unwrap_or_default();

    // Check for custom template in store.ai_prompts config first
    if let Some(custom_template) = &config.generate_children.custom_template {
        return custom_template
            .replace("{project_context}", &project_context)
            .replace("{req_context}", &req_context)
            .replace("{existing_children}", &existing_str)
            .replace("{available_types}", &types_str)
            .replace("{global_context}", &global_context_section)
            .replace("{additional_instructions}", &additional_instructions)
            .replace("{type_extra}", &type_extra)
            .replace("{req_type}", &req_type);
    }

    // Check for META prompt in database, fall back to embedded default
    let template = get_prompt_template(store, "Generate Children");

    // Apply substitutions to the template
    template
        .replace("{project_context}", &project_context)
        .replace("{req_context}", &req_context)
        .replace("{existing_children}", &existing_str)
        .replace("{available_types}", &types_str)
        .replace("{global_context}", &global_context_section)
        .replace("{additional_instructions}", &additional_instructions)
        .replace("{type_extra}", &type_extra)
        .replace("{req_type}", &req_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RequirementPriority, RequirementStatus, RequirementType};

    fn create_test_req() -> Requirement {
        let mut req = Requirement::new(
            "User Login".to_string(),
            "Users should be able to log in".to_string(),
        );
        req.spec_id = Some("FR-001".to_string());
        req.status = RequirementStatus::Draft;
        req.priority = RequirementPriority::High;
        req.req_type = RequirementType::Functional;
        req.feature = "Authentication".to_string();
        req
    }

    fn create_test_store() -> RequirementsStore {
        RequirementsStore {
            name: "test-project".to_string(),
            title: "Test Project".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_build_evaluation_prompt() {
        let req = create_test_req();
        let store = create_test_store();
        let prompt = build_evaluation_prompt(&req, &store);

        assert!(prompt.contains("User Login"));
        assert!(prompt.contains("FR-001"));
        assert!(prompt.contains("quality_score"));
    }

    #[test]
    fn test_build_duplicates_prompt() {
        let req = create_test_req();
        let store = create_test_store();
        let prompt = build_duplicates_prompt(&req, &store);

        assert!(prompt.contains("duplicate"));
        assert!(prompt.contains("similarity"));
    }

    /// TASK-215: every prompt that includes `{project_context}` should
    /// surface the AIDA Done lifecycle vocabulary so AI agents know the
    /// difference between Done (work finished on a branch) and Completed
    /// (merged to main). Spot-check via the evaluate prompt — the
    /// project-context helper is shared across all five META prompts so
    /// covering one is sufficient. trace:TASK-215 | ai:claude
    #[test]
    fn project_context_mentions_done_lifecycle() {
        let store = create_test_store();
        let ctx = build_project_context(&store);
        assert!(
            ctx.contains("Lifecycle Vocabulary"),
            "project context missing lifecycle section:\n{}",
            ctx
        );
        // Both states named explicitly so AI can disambiguate.
        assert!(ctx.contains("Done"), "lifecycle missing Done: {}", ctx);
        assert!(
            ctx.contains("Completed"),
            "lifecycle missing Completed: {}",
            ctx
        );
        // The key distinguishing phrase should be present so the AI
        // doesn't confuse the two terminals.
        assert!(
            ctx.contains("merged to the default branch"),
            "lifecycle missing the Completed = 'merged to main' phrasing"
        );
    }

    /// TASK-215: the generate-children template explicitly mentions Done
    /// as the pre-merge state for child specs so generated children land
    /// with sensible status guidance. trace:TASK-215 | ai:claude
    #[test]
    fn generate_children_prompt_mentions_done() {
        let template = crate::meta::DEFAULT_GENERATE_CHILDREN_PROMPT;
        assert!(
            template.contains("Done"),
            "generate-children prompt missing Done state mention"
        );
        assert!(
            template.contains("aida queue done")
                || template.contains("`aida queue done`")
                || template.contains("when the work is finished on a branch"),
            "generate-children prompt should name the Done set-point"
        );
    }
}
