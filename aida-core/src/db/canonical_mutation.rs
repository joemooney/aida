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
    pub title: Option<String>,
    pub description: Option<String>,
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
            sid.len() <= 128 && rev.len() == 64 && rev.bytes().all(|b| b.is_ascii_hexdigit()),
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
