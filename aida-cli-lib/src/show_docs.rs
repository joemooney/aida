//! Linked documentation for the human `aida show` detail view.
//!
//! A narrative Doc entry is linked to a spec either *directly* (the spec
//! carries a `References` edge to the Doc) or in *reverse* (the Doc was filed
//! with `aida doc add --about <SPEC>`, which writes a `References` edge from
//! the Doc to the spec). The reverse edge lives on the Doc's record, so the
//! spec's own relationship list never shows it; this module resolves both
//! directions into one labelled section without touching either record.
//!
//! Only the human view renders it. TOON and JSON return before this point, so
//! machine formats keep their existing schema.
// trace:TASK-1471 | ai:claude

use aida_core::models::{RelationshipType, Requirement, RequirementStatus, RequirementType};
use colored::Colorize;

/// Where a Doc's prose came from, read from its tags. Authorship is a
/// convention recorded in tags (`human`; `generated` / `ai-generated`), not a
/// stored field, so an untagged Doc is reported as unrecorded rather than
/// guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DocProvenance {
    HumanExposition,
    Generated,
    Unrecorded,
}

impl DocProvenance {
    fn from_tags<'a>(tags: impl IntoIterator<Item = &'a String>) -> Self {
        let mut generated = false;
        for t in tags {
            match t.to_ascii_lowercase().as_str() {
                "human" | "human-authored" => return DocProvenance::HumanExposition,
                "generated" | "ai-generated" => generated = true,
                _ => {}
            }
        }
        if generated {
            DocProvenance::Generated
        } else {
            DocProvenance::Unrecorded
        }
    }

    fn heading(self) -> &'static str {
        match self {
            DocProvenance::HumanExposition => "Human-authored explanations",
            DocProvenance::Generated => "Generated documentation (not human-reviewed prose)",
            DocProvenance::Unrecorded => "Other documentation (authorship not recorded)",
        }
    }
}

/// Which side of the graph carries the link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocLink {
    /// This spec references the Doc.
    Direct,
    /// The Doc references this spec (`aida doc add --about`).
    Reverse,
    Both,
}

impl DocLink {
    fn phrase(self) -> &'static str {
        match self {
            DocLink::Direct => "referenced by this spec",
            DocLink::Reverse => "written about this spec",
            DocLink::Both => "written about and referenced by this spec",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkedDoc {
    pub id: String,
    pub title: String,
    pub audience: Option<String>,
    pub scenario: Option<String>,
    pub tags: Vec<String>,
    pub provenance: DocProvenance,
    pub link: DocLink,
}

/// Resolve the Docs linked to `subject` from a candidate set. Candidates that
/// are not live Docs, or that are not linked in either direction, are
/// dropped; duplicates collapse to one entry. Sorted by provenance group,
/// then id.
pub(crate) fn collect_linked_docs(
    subject: &Requirement,
    candidates: impl IntoIterator<Item = Requirement>,
) -> Vec<LinkedDoc> {
    let mut out: Vec<LinkedDoc> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for doc in candidates {
        // Archived, declined (Rejected) and replaced (Superseded) Docs are
        // not current documentation, so they are not listed.
        // trace:TASK-1471 | ai:claude
        if doc.req_type != RequirementType::Doc
            || doc.archived
            || matches!(
                doc.status,
                RequirementStatus::Rejected | RequirementStatus::Superseded
            )
            || doc.id == subject.id
        {
            continue;
        }
        if !seen.insert(doc.id) {
            continue;
        }
        let direct = subject
            .relationships
            .iter()
            .any(|r| r.rel_type == RelationshipType::References && r.target_id == doc.id);
        let reverse = doc
            .relationships
            .iter()
            .any(|r| r.rel_type == RelationshipType::References && r.target_id == subject.id);
        let link = match (direct, reverse) {
            (true, true) => DocLink::Both,
            (true, false) => DocLink::Direct,
            (false, true) => DocLink::Reverse,
            (false, false) => continue,
        };
        let mut tags: Vec<String> = doc.tags.iter().cloned().collect();
        tags.sort();
        let non_empty = |k: &str| {
            doc.custom_fields
                .get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        out.push(LinkedDoc {
            id: doc.friendly_id(),
            title: doc.title.clone(),
            audience: non_empty("audience"),
            scenario: non_empty("scenario"),
            provenance: DocProvenance::from_tags(&tags),
            tags,
            link,
        });
    }
    out.sort_by(|a, b| (a.provenance, &a.id).cmp(&(b.provenance, &b.id)));
    out
}

/// Render the section as plain lines (no trailing newline). Empty input
/// renders nothing, so a spec without Docs shows no section at all.
pub(crate) fn render_linked_docs(docs: &[LinkedDoc]) -> Vec<String> {
    let mut lines = Vec::new();
    if docs.is_empty() {
        return lines;
    }
    lines.push(format!("{}:", "Documentation".bold()));
    let mut current: Option<DocProvenance> = None;
    for d in docs {
        if current != Some(d.provenance) {
            current = Some(d.provenance);
            lines.push(format!("  {}:", d.provenance.heading()));
        }
        lines.push(format!("    {}  {}", d.id.yellow(), d.title));
        let mut meta: Vec<String> = Vec::new();
        if let Some(a) = &d.audience {
            meta.push(format!("audience: {a}"));
        }
        if let Some(s) = &d.scenario {
            meta.push(format!("scenario: {s}"));
        }
        if !d.tags.is_empty() {
            meta.push(format!("tags: {}", d.tags.join(", ")));
        }
        if !meta.is_empty() {
            lines.push(format!("      {}", meta.join(" · ")));
        }
        lines.push(format!(
            "      {} · read: {}",
            d.link.phrase(),
            format!("aida doc show {}", d.id).cyan()
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::models::Relationship;

    fn spec(spec_id: &str, ty: RequirementType) -> Requirement {
        let mut r = Requirement::new(format!("{spec_id} title"), String::new());
        r.spec_id = Some(spec_id.to_string());
        r.req_type = ty;
        r
    }

    fn references(from: &mut Requirement, to: &Requirement) {
        from.relationships.push(Relationship {
            rel_type: RelationshipType::References,
            target_id: to.id,
            created_at: None,
            created_by: None,
        });
    }

    /// Render with colour disabled BEFORE formatting, so no ANSI codes are
    /// embedded even under CLICOLOR_FORCE=1 or a TTY.
    fn plain(docs: &[LinkedDoc]) -> String {
        colored::control::set_override(false);
        render_linked_docs(docs).join("\n")
    }

    /// STORY-1472 is described by a human explanation filed with
    /// `aida doc add --about STORY-1472` (DOC-1-174 in the original store).
    #[test]
    fn reverse_link_story_1472_to_human_explanation() {
        let story = spec("STORY-1472", RequirementType::Story);
        let mut doc = spec("DOC-1-174", RequirementType::Doc);
        doc.title = "Human explanation: reusable Jev questions and recorded judgments".into();
        doc.custom_fields.insert("audience".into(), "user".into());
        doc.custom_fields
            .insert("scenario".into(), "human orientation".into());
        for t in ["jev", "human", "exposition", "example"] {
            doc.tags.insert(t.into());
        }
        references(&mut doc, &story);
        let doc_id = doc.friendly_id();
        let story_rels_before = story.relationships.clone();

        let docs = collect_linked_docs(&story, vec![doc]);
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, doc_id);
        assert_eq!(docs[0].link, DocLink::Reverse);
        assert_eq!(docs[0].provenance, DocProvenance::HumanExposition);
        // Resolving the reverse link never edits the spec.
        assert_eq!(story.relationships, story_rels_before);

        let out = plain(&docs);
        assert!(out.starts_with("Documentation:"), "{out}");
        assert!(out.contains("Human-authored explanations:"), "{out}");
        assert!(
            out.contains(&format!("{doc_id}  Human explanation: reusable Jev")),
            "{out}"
        );
        assert!(
            out.contains("audience: user · scenario: human orientation · tags: example, exposition, human, jev"),
            "{out}"
        );
        assert!(
            out.contains(&format!(
                "written about this spec · read: aida doc show {doc_id}"
            )),
            "{out}"
        );
    }

    #[test]
    fn direct_link_and_both_directions() {
        let mut task = spec("TASK-9", RequirementType::Task);
        let mut generated = spec("DOC-2", RequirementType::Doc);
        generated.tags.insert("generated".into());
        let mut both = spec("DOC-3", RequirementType::Doc);
        references(&mut task, &generated);
        references(&mut task, &both);
        references(&mut both, &task);
        let (gen_id, both_id) = (generated.friendly_id(), both.friendly_id());

        let docs = collect_linked_docs(&task, vec![generated, both]);
        let by_id = |id: &str| docs.iter().find(|d| d.id == id).unwrap().clone();
        assert_eq!(by_id(&gen_id).link, DocLink::Direct);
        assert_eq!(by_id(&gen_id).provenance, DocProvenance::Generated);
        assert_eq!(by_id(&both_id).link, DocLink::Both);
        assert_eq!(by_id(&both_id).provenance, DocProvenance::Unrecorded);

        let out = plain(&docs);
        assert!(out.contains("Generated documentation"), "{out}");
        assert!(
            out.contains("Other documentation (authorship not recorded)"),
            "{out}"
        );
        assert!(
            out.contains(&format!(
                "referenced by this spec · read: aida doc show {gen_id}"
            )),
            "{out}"
        );
        // Generated groups ahead of unrecorded; human-authored would lead.
        assert!(
            out.find(&gen_id).unwrap() < out.find(&both_id).unwrap(),
            "{out}"
        );
    }

    #[test]
    fn no_section_without_linked_docs() {
        let task = spec("TASK-9", RequirementType::Task);
        let mut other = spec("TASK-10", RequirementType::Task);
        references(&mut other, &task); // linked, but not a Doc
        let unrelated_doc = spec("DOC-4", RequirementType::Doc);
        let mut archived = spec("DOC-5", RequirementType::Doc);
        references(&mut archived, &task);
        archived.archived = true;

        let docs = collect_linked_docs(&task, vec![other, unrelated_doc, archived]);
        assert!(docs.is_empty());
        assert!(render_linked_docs(&docs).is_empty());
    }

    #[test]
    fn rejected_and_superseded_docs_are_not_current_documentation() {
        let task = spec("TASK-9", RequirementType::Task);
        let mut rejected = spec("DOC-7", RequirementType::Doc);
        rejected.status = RequirementStatus::Rejected;
        references(&mut rejected, &task);
        let mut superseded = spec("DOC-8", RequirementType::Doc);
        superseded.status = RequirementStatus::Superseded;
        references(&mut superseded, &task);
        let mut live = spec("DOC-9", RequirementType::Doc);
        live.status = RequirementStatus::Completed;
        references(&mut live, &task);
        let live_id = live.friendly_id();

        let docs = collect_linked_docs(&task, vec![rejected, superseded, live]);
        assert_eq!(
            docs.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            vec![live_id.as_str()]
        );
    }

    #[test]
    fn duplicate_candidates_collapse() {
        let task = spec("TASK-9", RequirementType::Task);
        let mut doc = spec("DOC-6", RequirementType::Doc);
        references(&mut doc, &task);
        let docs = collect_linked_docs(&task, vec![doc.clone(), doc]);
        assert_eq!(docs.len(), 1);
    }
}
