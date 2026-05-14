//! META Requirements Module
//!
//! This module handles:
//! - Default prompt templates for AI operations
//! - Seeding META requirements in new databases
//! - Loading prompts from database with embedded fallback

use crate::models::{
    MetaSubtype, RelationshipType, Requirement, RequirementType, RequirementsStore,
};
use anyhow::Result;

// ============================================================================
// Default Prompt Templates
// ============================================================================

/// Default template for requirement evaluation
pub const DEFAULT_EVALUATION_PROMPT: &str = r#"You are an expert requirements analyst evaluating a software requirement for quality and completeness.

{global_context}
{project_context}

{req_context}

{related_context}
{additional_instructions}{type_extra}
## Task
Evaluate this requirement and provide a structured assessment. Consider:
1. Clarity: Is the requirement clearly stated and unambiguous?
2. Completeness: Does it have sufficient detail for implementation?
3. Testability: Can this requirement be verified/tested?
4. Consistency: Does it align with related requirements?
5. Feasibility: Is it realistic and achievable?

## Response Format
Respond ONLY with valid JSON in this exact format:
```json
{
  "quality_score": <1-10>,
  "issues": [
    {
      "type": "<vague_language|missing_criteria|ambiguous|incomplete|inconsistent|untestable>",
      "severity": "<low|medium|high>",
      "text": "<description of the issue>",
      "suggestion": "<how to fix it>"
    }
  ],
  "strengths": ["<strength1>", "<strength2>"],
  "suggested_improvements": {
    "description": "<improved description text if needed, or null>",
    "rationale": "<why this improvement helps>"
  }
}
```

Provide your evaluation now:"#;

/// Default template for finding duplicates
pub const DEFAULT_DUPLICATES_PROMPT: &str = r#"You are an expert requirements analyst identifying potential duplicate or overlapping requirements.

{global_context}
{project_context}

{req_context}

{all_reqs}
{additional_instructions}
## Task
Analyze the current requirement and compare it against all other requirements to find:
1. Exact duplicates (same functionality described differently)
2. Partial overlaps (requirements that cover similar ground)
3. Potential conflicts (requirements that contradict each other)

Only report requirements with similarity > 0.5 (50%).

## Response Format
Respond ONLY with valid JSON in this exact format:
```json
{
  "potential_duplicates": [
    {
      "spec_id": "<SPEC-ID of similar requirement>",
      "similarity": <0.0-1.0>,
      "reason": "<why these are similar>",
      "recommendation": "<merge|link|keep_separate|review>"
    }
  ]
}
```

If no duplicates found, return: {"potential_duplicates": []}

Provide your analysis now:"#;

/// Default template for suggesting relationships
pub const DEFAULT_RELATIONSHIPS_PROMPT: &str = r#"You are an expert requirements analyst identifying missing relationships between requirements.

{global_context}
{project_context}

{req_context}

{all_reqs}

## Available Relationship Types
- {rel_types}
{additional_instructions}
## Task
Analyze the current requirement and suggest relationships that should exist but don't:
1. Dependencies (what must be done first)
2. Parent/child relationships (decomposition)
3. Verification relationships (what tests/validates this)
4. References (related but not dependent)

Only suggest relationships with confidence > 0.7 (70%).

## Response Format
Respond ONLY with valid JSON in this exact format:
```json
{
  "suggested_relationships": [
    {
      "rel_type": "<relationship type>",
      "target_spec_id": "<SPEC-ID of target requirement>",
      "confidence": <0.0-1.0>,
      "rationale": "<why this relationship should exist>"
    }
  ]
}
```

If no relationships to suggest, return: {"suggested_relationships": []}

Provide your analysis now:"#;

/// Default template for improving descriptions
pub const DEFAULT_IMPROVE_PROMPT: &str = r#"You are an expert requirements analyst improving a requirement's description for clarity and completeness.

{global_context}
{project_context}

{req_context}

{related_context}

{examples}
{additional_instructions}{type_extra}
## Task
Improve the requirement's description to be:
1. Clear and unambiguous
2. Complete with acceptance criteria where appropriate
3. Testable/verifiable
4. Consistent with the requirement type ({req_type})
5. Professional and well-structured

Do NOT change the meaning or scope of the requirement.

## Response Format
Respond ONLY with valid JSON in this exact format:
```json
{
  "improved_description": "<the improved description text>",
  "changes_made": ["<change1>", "<change2>"],
  "rationale": "<why these improvements help>"
}
```

Provide your improved version now:"#;

/// Default template for generating child requirements
pub const DEFAULT_GENERATE_CHILDREN_PROMPT: &str = r#"You are an expert requirements analyst decomposing a high-level requirement into specific, actionable child requirements.

{global_context}
{project_context}

{req_context}

{existing_children}
{additional_instructions}{type_extra}
## Task
Break down this requirement into specific, actionable child requirements that together fulfill the parent requirement:

1. Each child should be independently implementable
2. Children should be specific and testable
3. Consider different aspects: functionality, UI, data, integration, error handling
4. Suggest appropriate requirement types for each child
5. Avoid duplicating existing children

Children start at `Approved` (or `Draft` if more discussion is needed). They flip to `In Progress` when an implementer picks them up, to `Done` when the work is finished on a branch (set by `aida queue done`), and to `Completed` only after the merge to the default branch — see the Lifecycle Vocabulary section above. trace:TASK-215 | ai:claude

## Response Format
Respond ONLY with valid JSON in this exact format:
```json
{
  "suggested_children": [
    {
      "title": "<concise title>",
      "description": "<detailed description with acceptance criteria>",
      "type": "<Functional|NonFunctional|Task|Story>",
      "priority": "<High|Medium|Low>",
      "rationale": "<why this child requirement is needed>"
    }
  ]
}
```

Provide your analysis now:"#;

// ============================================================================
// Prompt Loading with Database Fallback
// ============================================================================

/// Get a prompt template, checking database first, then falling back to embedded
pub fn get_prompt_template(store: &RequirementsStore, prompt_name: &str) -> String {
    // Look for a META requirement with MetaSubtype::Prompt and matching title
    if let Some(meta_req) = store.requirements.iter().find(|r| {
        r.req_type == RequirementType::Meta
            && r.meta_subtype == Some(MetaSubtype::Prompt)
            && r.title == prompt_name
    }) {
        // Use the requirement's description as the template
        if !meta_req.description.is_empty() {
            return meta_req.description.clone();
        }
    }

    // Fall back to embedded defaults
    match prompt_name {
        "Evaluate Requirement" => DEFAULT_EVALUATION_PROMPT.to_string(),
        "Find Duplicates" => DEFAULT_DUPLICATES_PROMPT.to_string(),
        "Suggest Relationships" => DEFAULT_RELATIONSHIPS_PROMPT.to_string(),
        "Improve Description" => DEFAULT_IMPROVE_PROMPT.to_string(),
        "Generate Children" => DEFAULT_GENERATE_CHILDREN_PROMPT.to_string(),
        _ => String::new(),
    }
}

// ============================================================================
// META Seeding
// ============================================================================

/// Seed META requirements in a new database
///
/// Creates a META-PROMPTS folder with default AI prompt templates.
/// This allows users to customize prompts by editing requirements.
pub fn seed_meta_requirements(store: &mut RequirementsStore) -> Result<()> {
    // Check if META requirements already exist
    let has_meta = store
        .requirements
        .iter()
        .any(|r| r.req_type == RequirementType::Meta);

    if has_meta {
        // Already seeded
        return Ok(());
    }

    // Create META-PROMPTS folder
    let mut prompts_folder = Requirement::new(
        "AI Prompts".to_string(),
        "Default AI prompt templates. Edit these to customize how AI analyzes and improves requirements.".to_string(),
    );
    prompts_folder.req_type = RequirementType::Meta;
    prompts_folder.meta_subtype = Some(MetaSubtype::Prompt);
    let prompts_folder_id = prompts_folder.id;

    // Add the folder
    store.add_requirement_with_id(prompts_folder, None, Some("META"));

    // Define prompt templates to seed
    let prompts = [
        (
            "Evaluate Requirement",
            DEFAULT_EVALUATION_PROMPT,
            "Template for evaluating requirement quality. Placeholders: {global_context}, {project_context}, {req_context}, {related_context}, {additional_instructions}, {type_extra}",
        ),
        (
            "Find Duplicates",
            DEFAULT_DUPLICATES_PROMPT,
            "Template for finding duplicate or overlapping requirements. Placeholders: {global_context}, {project_context}, {req_context}, {all_reqs}, {additional_instructions}",
        ),
        (
            "Suggest Relationships",
            DEFAULT_RELATIONSHIPS_PROMPT,
            "Template for suggesting missing relationships. Placeholders: {global_context}, {project_context}, {req_context}, {all_reqs}, {rel_types}, {additional_instructions}",
        ),
        (
            "Improve Description",
            DEFAULT_IMPROVE_PROMPT,
            "Template for improving requirement descriptions. Placeholders: {global_context}, {project_context}, {req_context}, {related_context}, {examples}, {additional_instructions}, {type_extra}, {req_type}",
        ),
        (
            "Generate Children",
            DEFAULT_GENERATE_CHILDREN_PROMPT,
            "Template for generating child requirements. Placeholders: {global_context}, {project_context}, {req_context}, {existing_children}, {additional_instructions}, {type_extra}",
        ),
    ];

    // Create prompt requirements
    for (title, template, help_text) in prompts {
        let description = format!("{}\n\n---\n\n{}", help_text, template);

        let mut prompt_req = Requirement::new(title.to_string(), description);
        prompt_req.req_type = RequirementType::Meta;
        prompt_req.meta_subtype = Some(MetaSubtype::Prompt);
        let prompt_id = prompt_req.id;

        store.add_requirement_with_id(prompt_req, None, Some("META"));

        // Link as child of prompts folder
        store.set_relationship(
            &prompt_id,
            RelationshipType::Parent,
            &prompts_folder_id,
            true,
        )?;
    }

    Ok(())
}

/// Check if META requirements need seeding
pub fn needs_meta_seeding(store: &RequirementsStore) -> bool {
    !store
        .requirements
        .iter()
        .any(|r| r.req_type == RequirementType::Meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_prompt_template_fallback() {
        let store = RequirementsStore::default();

        // Should return default templates when no META requirements exist
        let eval = get_prompt_template(&store, "Evaluate Requirement");
        assert!(eval.contains("quality_score"));

        let dups = get_prompt_template(&store, "Find Duplicates");
        assert!(dups.contains("potential_duplicates"));

        // Unknown prompt should return empty string
        let unknown = get_prompt_template(&store, "Unknown Prompt");
        assert!(unknown.is_empty());
    }

    #[test]
    fn test_seed_meta_requirements() {
        let mut store = RequirementsStore::default();

        assert!(needs_meta_seeding(&store));

        seed_meta_requirements(&mut store).unwrap();

        assert!(!needs_meta_seeding(&store));

        // Should have created 6 META requirements (1 folder + 5 prompts)
        let meta_count = store
            .requirements
            .iter()
            .filter(|r| r.req_type == RequirementType::Meta)
            .count();
        assert_eq!(meta_count, 6);

        // Seeding again should be a no-op
        seed_meta_requirements(&mut store).unwrap();
        let meta_count_after = store
            .requirements
            .iter()
            .filter(|r| r.req_type == RequirementType::Meta)
            .count();
        assert_eq!(meta_count_after, 6);
    }

    #[test]
    fn test_get_prompt_from_database() {
        let mut store = RequirementsStore::default();

        // Create a custom META prompt
        let mut custom_prompt = Requirement::new(
            "Evaluate Requirement".to_string(),
            "Custom evaluation template here".to_string(),
        );
        custom_prompt.req_type = RequirementType::Meta;
        custom_prompt.meta_subtype = Some(MetaSubtype::Prompt);
        store.requirements.push(custom_prompt);

        // Should return custom template
        let template = get_prompt_template(&store, "Evaluate Requirement");
        assert_eq!(template, "Custom evaluation template here");
    }
}
