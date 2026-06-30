//! Response Parsing Module
//!
//! Parses JSON responses from AI into structured data types.

use crate::ai::client::AiError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs_forge::TS;

/// Issue found in a requirement
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct IssueReport {
    #[serde(rename = "type")]
    pub issue_type: String,
    pub severity: String,
    pub text: String,
    pub suggestion: String,
}

/// Suggested improvement to a requirement
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SuggestedImprovement {
    pub description: Option<String>,
    pub rationale: String,
}

/// Response from evaluation action
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct EvaluationResponse {
    pub quality_score: u8,
    pub issues: Vec<IssueReport>,
    pub strengths: Vec<String>,
    pub suggested_improvements: Option<SuggestedImprovement>,
}

/// Stored AI evaluation with metadata
/// This struct wraps evaluation results with timestamps to track when
/// evaluations need to be refreshed (when requirement changes).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct StoredAiEvaluation {
    /// The evaluation results from AI
    pub evaluation: EvaluationResponse,
    /// When the evaluation was performed
    pub evaluated_at: DateTime<Utc>,
    /// Hash of the requirement content at time of evaluation
    /// Used to detect if requirement has changed since evaluation
    pub content_hash: String,
}

impl StoredAiEvaluation {
    /// Create a new stored evaluation from a response
    pub fn new(evaluation: EvaluationResponse, content_hash: String) -> Self {
        Self {
            evaluation,
            evaluated_at: Utc::now(),
            content_hash,
        }
    }

    /// Check if the evaluation is stale (content has changed)
    pub fn is_stale(&self, current_hash: &str) -> bool {
        self.content_hash != current_hash
    }
}

/// A potential duplicate requirement
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DuplicateResult {
    pub spec_id: String,
    pub similarity: f64,
    pub reason: String,
    pub recommendation: String,
}

/// Response from find duplicates action
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DuplicatesResponse {
    pub potential_duplicates: Vec<DuplicateResult>,
}

/// A suggested relationship
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RelationshipSuggestion {
    pub rel_type: String,
    pub target_spec_id: String,
    pub confidence: f64,
    pub rationale: String,
}

/// Response from suggest relationships action
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SuggestRelationshipsResponse {
    pub suggested_relationships: Vec<RelationshipSuggestion>,
}

/// Response from improve description action
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ImproveDescriptionResponse {
    pub improved_description: String,
    pub changes_made: Vec<String>,
    pub rationale: String,
}

/// Response from drafting a full spec out of a free-text thought.
///
/// The structured result of `build_draft_spec_prompt`: a crisp title, an
/// expanded description, and genuine acceptance criteria — the payload the
/// thought-to-spec front door files as a draft.
// trace:STORY-725 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DraftSpecResponse {
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
}

/// A generated child requirement
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GeneratedChild {
    pub title: String,
    pub description: String,
    #[serde(rename = "type")]
    pub req_type: String,
    pub rationale: String,
}

/// Response from generate children action
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GenerateChildrenResponse {
    pub suggested_children: Vec<GeneratedChild>,
}

/// Extract JSON from a response that may contain markdown code blocks
fn extract_json(response: &str) -> &str {
    // Look for JSON in markdown code block
    if let Some(start) = response.find("```json") {
        let json_start = start + 7; // Skip "```json"
        if let Some(end) = response[json_start..].find("```") {
            return response[json_start..json_start + end].trim();
        }
    }

    // Look for generic code block
    if let Some(start) = response.find("```") {
        let code_start = start + 3;
        // Skip language identifier if present
        let json_start = if let Some(newline) = response[code_start..].find('\n') {
            code_start + newline + 1
        } else {
            code_start
        };
        if let Some(end) = response[json_start..].find("```") {
            return response[json_start..json_start + end].trim();
        }
    }

    // Try to find JSON object directly
    if let Some(start) = response.find('{') {
        if let Some(end) = response.rfind('}') {
            if end > start {
                return &response[start..=end];
            }
        }
    }

    response.trim()
}

/// Parse evaluation response from AI
pub fn parse_evaluation_response(response: &str) -> Result<EvaluationResponse, AiError> {
    let json_str = extract_json(response);
    serde_json::from_str(json_str).map_err(|e| {
        AiError::InvalidResponse(format!(
            "Failed to parse evaluation response: {}. JSON: {}",
            e,
            &json_str[..json_str.len().min(200)]
        ))
    })
}

/// Parse duplicates response from AI
pub fn parse_duplicates_response(response: &str) -> Result<DuplicatesResponse, AiError> {
    let json_str = extract_json(response);
    serde_json::from_str(json_str).map_err(|e| {
        AiError::InvalidResponse(format!(
            "Failed to parse duplicates response: {}. JSON: {}",
            e,
            &json_str[..json_str.len().min(200)]
        ))
    })
}

/// Parse relationships response from AI
pub fn parse_relationships_response(
    response: &str,
) -> Result<SuggestRelationshipsResponse, AiError> {
    let json_str = extract_json(response);
    serde_json::from_str(json_str).map_err(|e| {
        AiError::InvalidResponse(format!(
            "Failed to parse relationships response: {}. JSON: {}",
            e,
            &json_str[..json_str.len().min(200)]
        ))
    })
}

/// Parse improve description response from AI
pub fn parse_improve_response(response: &str) -> Result<ImproveDescriptionResponse, AiError> {
    let json_str = extract_json(response);
    serde_json::from_str(json_str).map_err(|e| {
        AiError::InvalidResponse(format!(
            "Failed to parse improve response: {}. JSON: {}",
            e,
            &json_str[..json_str.len().min(200)]
        ))
    })
}

/// Parse a drafted spec response from AI.
// trace:STORY-725 | ai:claude
pub fn parse_draft_spec_response(response: &str) -> Result<DraftSpecResponse, AiError> {
    let json_str = extract_json(response);
    serde_json::from_str(json_str).map_err(|e| {
        AiError::InvalidResponse(format!(
            "Failed to parse draft spec response: {}. JSON: {}",
            e,
            &json_str[..json_str.len().min(200)]
        ))
    })
}

/// Parse generate children response from AI
pub fn parse_generate_children_response(
    response: &str,
) -> Result<GenerateChildrenResponse, AiError> {
    let json_str = extract_json(response);
    serde_json::from_str(json_str).map_err(|e| {
        AiError::InvalidResponse(format!(
            "Failed to parse generate children response: {}. JSON: {}",
            e,
            &json_str[..json_str.len().min(200)]
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_from_markdown() {
        let response = r#"Here's my analysis:

```json
{
  "quality_score": 7,
  "issues": [],
  "strengths": ["Clear title"],
  "suggested_improvements": null
}
```

That's my evaluation."#;

        let json = extract_json(response);
        assert!(json.starts_with('{'));
        assert!(json.ends_with('}'));
        assert!(json.contains("quality_score"));
    }

    #[test]
    fn test_extract_json_direct() {
        let response = r#"{"quality_score": 8, "issues": [], "strengths": [], "suggested_improvements": null}"#;
        let json = extract_json(response);
        assert_eq!(json, response);
    }

    #[test]
    fn test_parse_evaluation_response() {
        let response = r#"```json
{
  "quality_score": 7,
  "issues": [
    {
      "type": "vague_language",
      "severity": "medium",
      "text": "Description uses vague terms",
      "suggestion": "Add specific criteria"
    }
  ],
  "strengths": ["Clear title", "Good type"],
  "suggested_improvements": {
    "description": "Improved text here",
    "rationale": "Makes it clearer"
  }
}
```"#;

        let result = parse_evaluation_response(response).unwrap();
        assert_eq!(result.quality_score, 7);
        assert_eq!(result.issues.len(), 1);
        assert_eq!(result.strengths.len(), 2);
        assert!(result.suggested_improvements.is_some());
    }

    #[test]
    fn test_parse_duplicates_response() {
        let response = r#"{"potential_duplicates": []}"#;
        let result = parse_duplicates_response(response).unwrap();
        assert!(result.potential_duplicates.is_empty());
    }

    #[test]
    fn test_parse_duplicates_with_results() {
        let response = r#"```json
{
  "potential_duplicates": [
    {
      "spec_id": "FR-002",
      "similarity": 0.85,
      "reason": "Both describe login",
      "recommendation": "merge"
    }
  ]
}
```"#;

        let result = parse_duplicates_response(response).unwrap();
        assert_eq!(result.potential_duplicates.len(), 1);
        assert_eq!(result.potential_duplicates[0].spec_id, "FR-002");
    }

    #[test]
    fn test_parse_relationships_response() {
        let response = r#"{"suggested_relationships": []}"#;
        let result = parse_relationships_response(response).unwrap();
        assert!(result.suggested_relationships.is_empty());
    }

    #[test]
    fn test_parse_improve_response() {
        let response = r#"```json
{
  "improved_description": "Better description here",
  "changes_made": ["Added criteria", "Clarified scope"],
  "rationale": "Improves clarity"
}
```"#;

        let result = parse_improve_response(response).unwrap();
        assert_eq!(result.improved_description, "Better description here");
        assert_eq!(result.changes_made.len(), 2);
    }

    #[test]
    fn test_parse_draft_spec_response() {
        let response = r#"```json
{
  "title": "Show the parent title in the tree header",
  "description": "The tree view header should display the parent spec's title for orientation.",
  "acceptance_criteria": [
    "The tree header renders the parent's title text",
    "A root node with no parent shows no parent title and does not error"
  ]
}
```"#;

        let result = parse_draft_spec_response(response).unwrap();
        assert_eq!(result.title, "Show the parent title in the tree header");
        assert_eq!(result.acceptance_criteria.len(), 2);
    }

    #[test]
    fn test_parse_draft_spec_response_tolerates_missing_acceptance() {
        let response = r#"{"title": "T", "description": "D"}"#;
        let result = parse_draft_spec_response(response).unwrap();
        assert_eq!(result.title, "T");
        assert!(result.acceptance_criteria.is_empty());
    }

    #[test]
    fn test_parse_generate_children_response() {
        let response = r#"```json
{
  "suggested_children": [
    {
      "title": "Child 1",
      "description": "Description 1",
      "type": "Task",
      "rationale": "Reason 1"
    }
  ]
}
```"#;

        let result = parse_generate_children_response(response).unwrap();
        assert_eq!(result.suggested_children.len(), 1);
        assert_eq!(result.suggested_children[0].title, "Child 1");
    }
}
