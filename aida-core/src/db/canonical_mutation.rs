//! Closed canonical-only operations. No paths, arbitrary replacement objects,
//! status transitions, claim/queue effects or client-supplied authors.
//! trace:TASK-3223 | ai:codex
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "op",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CanonicalOperation {
    CommentAdd {
        spec_id: String,
        expected_revision: String,
        content: String,
    },
    SpecEdit {
        spec_id: String,
        expected_revision: String,
        fields: EditFields,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditFields {
    #[serde(
        default,
        deserialize_with = "present_text",
        skip_serializing_if = "Option::is_none"
    )]
    pub title: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_text",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_text",
        skip_serializing_if = "Option::is_none"
    )]
    pub priority: Option<String>,
    /// Missing leaves tags unchanged; an empty list clears nonstructural tags.
    #[serde(
        default,
        deserialize_with = "present_tags",
        skip_serializing_if = "Option::is_none"
    )]
    pub tags: Option<Vec<String>>,
    /// Missing leaves weight unchanged; explicit null clears it.
    #[serde(
        default,
        deserialize_with = "present_weight",
        skip_serializing_if = "Option::is_none"
    )]
    pub weight: Option<Option<f32>>,
}
fn present_tags<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
    Vec::<String>::deserialize(d).map(Some)
}
fn present_weight<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<f32>>, D::Error> {
    Option::<f32>::deserialize(d).map(Some)
}
// Missing text fields mean unchanged; explicit null is not a text edit verb.
// Match the broker's closed schema independently in the trusted host.
fn present_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}
// trace:TASK-3223 | ai:codex
impl EditFields {
    pub fn parsed_priority(&self) -> anyhow::Result<Option<crate::RequirementPriority>> {
        self.priority
            .as_deref()
            .map(|value| match value.to_lowercase().as_str() {
                "high" => Ok(crate::RequirementPriority::High),
                "medium" | "med" => Ok(crate::RequirementPriority::Medium),
                "low" => Ok(crate::RequirementPriority::Low),
                _ => anyhow::bail!("InvalidRequest"),
            })
            .transpose()
    }
}

impl CanonicalOperation {
    pub fn target(&self) -> (&str, &str) {
        match self {
            Self::CommentAdd {
                spec_id,
                expected_revision,
                ..
            }
            | Self::SpecEdit {
                spec_id,
                expected_revision,
                ..
            } => (spec_id, expected_revision),
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        let (sid, rev) = self.target();
        anyhow::ensure!(
            sid.len() <= 128
                && rev.len() == 64
                && rev
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "InvalidRequest"
        );
        crate::object_store::relative_object_path(sid)?;
        let text = |s: &str| {
            !s.is_empty()
                && s.len() <= 16384
                && !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
        };
        match self {
            Self::CommentAdd { content, .. } => {
                anyhow::ensure!(
                    text(content) && !content.trim().is_empty(),
                    "InvalidRequest"
                )
            }
            Self::SpecEdit { fields, .. } => {
                anyhow::ensure!(
                    fields.title.is_some()
                        || fields.description.is_some()
                        || fields.priority.is_some()
                        || fields.weight.is_some()
                        || fields.tags.is_some(),
                    "InvalidRequest"
                );
                fields.parsed_priority()?;
                if let Some(tags) = &fields.tags {
                    anyhow::ensure!(tags.len() <= 256, "InvalidRequest");
                    for tag in tags {
                        anyhow::ensure!(
                            tag.len() <= 512 && !tag.chars().any(char::is_control),
                            "InvalidRequest"
                        );
                        anyhow::ensure!(
                            !tag.trim().chars().any(char::is_whitespace),
                            "InvalidRequest"
                        );
                    }
                }
                if let Some(Some(weight)) = fields.weight {
                    anyhow::ensure!(weight.is_finite(), "InvalidRequest");
                }
                if let Some(t) = &fields.title {
                    anyhow::ensure!(text(t) && t.len() <= 512, "InvalidRequest");
                }
                if let Some(t) = &fields.description {
                    anyhow::ensure!(
                        t.len() <= 16384
                            && !t.chars().any(|c| c.is_control() && c != '\n' && c != '\t'),
                        "InvalidRequest"
                    );
                }
            }
        }
        Ok(())
    }
}

/// Shared replacement protection for routing and graph-integrity tags.
// trace:TASK-3223 | ai:codex
pub fn dropped_structural_tags(
    old_tags: &std::collections::HashSet<String>,
    new_tags: &std::collections::HashSet<String>,
) -> Vec<String> {
    const PREFIXES: &[&str] = &[
        "parent:",
        "batch:",
        "lane:",
        "severity:",
        "lifecycle:",
        "aida:",
    ];
    let mut dropped: Vec<String> = old_tags
        .difference(new_tags)
        .filter(|tag| PREFIXES.iter().any(|prefix| tag.starts_with(prefix)))
        .cloned()
        .collect();
    dropped.sort();
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_reject_whitespace_across_all_entry_points() {
        for content in ["", " ", "\n\t", "\u{2003}"] {
            assert!(CanonicalOperation::CommentAdd {
                spec_id: "TASK-1".into(),
                expected_revision: "a".repeat(64),
                content: content.into(),
            }
            .validate()
            .is_err());
        }
    }

    #[test]
    fn canonical_schema_matches_broker_revision_and_field_rules() {
        let operation = |revision: &str, fields: serde_json::Value| {
            serde_json::from_value::<CanonicalOperation>(serde_json::json!({
                "op": "spec_edit", "data": {
                    "spec_id": "TASK-1", "expected_revision": revision, "fields": fields
                }
            }))
        };
        let revision = "a".repeat(64);
        let valid = serde_json::json!({"description": "first\n\tsecond"});
        let decoded = operation(&revision, valid.clone()).unwrap();
        decoded.validate().unwrap();
        serde_json::from_value::<CanonicalOperation>(serde_json::to_value(decoded).unwrap())
            .unwrap()
            .validate()
            .unwrap();
        assert!(operation(&"A".repeat(64), valid)
            .unwrap()
            .validate()
            .is_err());
        for fields in [
            serde_json::json!({"tags": null}),
            serde_json::json!({"tags": "one,two"}),
            serde_json::json!({"tags": [1]}),
            serde_json::json!({"priority": null}),
            serde_json::json!({"priority": 1}),
            serde_json::json!({"weight": "42"}),
            serde_json::json!({"weight": true}),
            serde_json::json!({"title": null, "description": "text"}),
            serde_json::json!({"description": null, "title": "title"}),
            serde_json::json!({"title": "title", "status": "Completed"}),
        ] {
            assert!(operation(&revision, fields).is_err());
        }
        for tags in [
            serde_json::json!([]),
            serde_json::json!(["alpha", " beta ", "alpha"]),
        ] {
            let decoded = operation(&revision, serde_json::json!({"tags":tags})).unwrap();
            decoded.validate().unwrap();
        }
        for tags in [
            serde_json::json!(["one two"]),
            serde_json::json!(["x\u{7f}"]),
            serde_json::json!(["x".repeat(513)]),
            serde_json::json!(vec!["x"; 257]),
        ] {
            assert!(operation(&revision, serde_json::json!({"tags":tags}))
                .unwrap()
                .validate()
                .is_err());
        }
        for weight in [serde_json::json!(0.75), serde_json::Value::Null] {
            let decoded = operation(&revision, serde_json::json!({"weight": weight})).unwrap();
            decoded.validate().unwrap();
            assert_eq!(
                serde_json::to_value(decoded).unwrap()["data"]["fields"]["weight"],
                weight
            );
        }
        assert!(operation(&revision, serde_json::json!({"weight": 1e100}))
            .unwrap()
            .validate()
            .is_err());
        assert!(operation(&revision, serde_json::json!({}))
            .unwrap()
            .validate()
            .is_err());
    }
}
