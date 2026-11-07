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
}
// Missing fields mean unchanged; explicit null is not an edit verb. Match the
// broker's closed schema when decoding independently in the trusted host.
fn present_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
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
            Self::CommentAdd { content, .. } => anyhow::ensure!(text(content), "InvalidRequest"),
            Self::SpecEdit { fields, .. } => {
                anyhow::ensure!(
                    fields.title.is_some() || fields.description.is_some(),
                    "InvalidRequest"
                );
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

#[cfg(test)]
mod tests {
    use super::*;

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
            serde_json::json!({"title": null, "description": "text"}),
            serde_json::json!({"description": null, "title": "title"}),
            serde_json::json!({"title": "title", "status": "Completed"}),
        ] {
            assert!(operation(&revision, fields).is_err());
        }
        assert!(operation(&revision, serde_json::json!({}))
            .unwrap()
            .validate()
            .is_err());
    }
}
