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

/// Maximum number of protocol body lines carried into a pickup prompt.
pub const PROTOCOL_PICKUP_LINE_CAP: usize = 40;

/// The editable, substrate-backed protocol for a requirement type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeProtocol {
    pub meta_id: String,
    pub req_type: String,
    pub body: String,
}

const DEFAULT_PROTOCOLS: &[(&str, &str)] = &[
    ("spike", "Purpose: answer a bounded question with evidence.\nDeliverable: write the evidence-grounded report at the docs/spikes/*.md path named by the spec.\nBefore done: the report answers the question, records evidence and uncertainty, and names the recommended next step.\nReviewer: an advisor reads the report before anything builds on it.\nHuman boundary: decisions, taste calls, and credentials stay with the operator."),
    ("bug", "Purpose: restore intended behavior and prevent recurrence.\nDeliverable: a focused fix plus a regression test.\nBefore done: reproduce or characterize the failure, make the test fail before the fix where practical, and run the relevant suite.\nReviewer: checks the root cause, regression coverage, and blast radius.\nHuman boundary: product-policy changes are decisions, not bug fixes."),
    ("story", "Purpose: deliver the accepted user-visible capability.\nDeliverable: implementation and verification for every acceptance criterion.\nBefore done: acceptance is traceable to code or tests and relevant checks pass.\nReviewer: checks behavior, scope, compatibility, and documentation.\nHuman boundary: unresolved product or architecture forks return to the operator/advisor."),
    ("task", "Purpose: complete the bounded technical or operational outcome.\nDeliverable: the artifact or repository change named by the spec.\nBefore done: acceptance is satisfied and relevant checks pass.\nReviewer: checks completeness, focus, and unintended side effects.\nHuman boundary: expand scope only through a new or edited requirement."),
    ("decision", "Purpose: make and preserve an architecture decision.\nDeliverable: an ADR recording context, options, decision, and consequences.\nBefore done: status is accepted and references connect the decision to affected work.\nReviewer: checks alternatives, evidence, reversibility, and consequences.\nHuman boundary: the accountable human accepts consequential or taste-based choices."),
    ("doc", "Purpose: keep the durable documentation true and useful.\nDeliverable: the named documentation update.\nBefore done: examples and links are verified against current behavior.\nReviewer: checks audience fit, accuracy, discoverability, and drift risk.\nHuman boundary: policy claims require their accountable owner."),
];

/// Resolve a type protocol from editable META data.
// trace:STORY-1221 | ai:codex
pub fn get_type_protocol(store: &RequirementsStore, req_type: &str) -> Option<TypeProtocol> {
    let slug = format!("protocol:{}", req_type.trim().to_ascii_lowercase());
    store.requirements.iter().find_map(|r| {
        (r.req_type == RequirementType::Meta
            && r.tags.iter().any(|t| t.eq_ignore_ascii_case(&slug)))
        .then(|| TypeProtocol {
            meta_id: r.display_id().to_string(),
            req_type: req_type.trim().to_ascii_lowercase(),
            body: r.description.clone(),
        })
    })
}

impl TypeProtocol {
    /// Labeled pickup block. Acceptance is explicitly the higher-precedence layer.
    pub fn pickup_block(&self) -> String {
        let body = self
            .body
            .lines()
            .take(PROTOCOL_PICKUP_LINE_CAP)
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "## Type protocol: {} [{}]\nPrecedence: type protocol < spec acceptance\n{}",
            self.req_type, self.meta_id, body
        )
    }

    /// One line suitable for the per-turn notice hook.
    pub fn notice_line(&self) -> String {
        let summary = self
            .body
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("follow the type protocol");
        format!(
            "protocol: {} [{}] — {}",
            self.req_type,
            self.meta_id,
            summary.trim()
        )
    }
}

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
        seed_missing_type_protocols(store);
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

    seed_missing_type_protocols(store);

    Ok(())
}

/// Add absent built-in protocols without overwriting project-edited rows.
// trace:STORY-1221 | ai:codex
pub fn seed_missing_type_protocols(store: &mut RequirementsStore) -> usize {
    let mut seeded = 0;
    for (kind, body) in DEFAULT_PROTOCOLS {
        let tag = format!("protocol:{kind}");
        if store.requirements.iter().any(|r| {
            r.req_type == RequirementType::Meta
                && r.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag))
        }) {
            continue;
        }
        let mut protocol = Requirement::new(format!("{} protocol", kind), (*body).to_string());
        protocol.req_type = RequirementType::Meta;
        protocol.tags.insert(tag);
        store.add_requirement_with_id(protocol, None, Some("META"));
        seeded += 1;
    }
    seeded
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

        // Six prompt META rows plus one editable protocol per work type.
        let meta_count = store
            .requirements
            .iter()
            .filter(|r| r.req_type == RequirementType::Meta)
            .count();
        assert_eq!(meta_count, 12);

        // Seeding again should be a no-op
        seed_meta_requirements(&mut store).unwrap();
        let meta_count_after = store
            .requirements
            .iter()
            .filter(|r| r.req_type == RequirementType::Meta)
            .count();
        assert_eq!(meta_count_after, 12);
    }

    #[test]
    fn seeded_protocol_is_capped_and_live_editable() {
        let mut store = RequirementsStore::default();
        seed_meta_requirements(&mut store).unwrap();
        let spike = get_type_protocol(&store, "SPIKE").unwrap();
        assert!(spike.pickup_block().contains(&spike.meta_id));
        assert!(spike.body.contains("docs/spikes/*.md"));
        let row = store
            .requirements
            .iter_mut()
            .find(|r| r.tags.contains("protocol:spike"))
            .unwrap();
        row.description = (0..50)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let edited = get_type_protocol(&store, "spike").unwrap();
        assert_eq!(
            edited.pickup_block().lines().count(),
            PROTOCOL_PICKUP_LINE_CAP + 2
        );
        assert!(!edited.pickup_block().contains("line 40"));
    }

    #[test]
    fn seed_missing_protocols_preserves_two_and_adds_four() {
        let mut store = RequirementsStore::default();
        for kind in ["spike", "bug"] {
            let mut protocol = Requirement::new(
                format!("{kind} protocol"),
                format!("custom {kind} contract"),
            );
            protocol.req_type = RequirementType::Meta;
            protocol.tags.insert(format!("protocol:{kind}"));
            store.add_requirement_with_id(protocol, None, Some("META"));
        }

        assert_eq!(seed_missing_type_protocols(&mut store), 4);
        assert_eq!(seed_missing_type_protocols(&mut store), 0);
        assert_eq!(
            get_type_protocol(&store, "spike").unwrap().body,
            "custom spike contract"
        );
        assert_eq!(
            get_type_protocol(&store, "bug").unwrap().body,
            "custom bug contract"
        );
        for kind in ["spike", "bug", "story", "task", "decision", "doc"] {
            assert!(get_type_protocol(&store, kind).is_some(), "missing {kind}");
        }
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
