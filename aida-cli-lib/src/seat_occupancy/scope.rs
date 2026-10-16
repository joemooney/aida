//! Child review scope from local persisted requirements, never title trailers,
//! caller role labels, PR bodies or a network/forge query.
// trace:TASK-1607 | ai:codex

use std::collections::BTreeSet;

use aida_core::{RelationshipType, RequirementsStore};

/// Each row is one persisted review story and its direct `implements` targets.
/// Build this snapshot before taking a seat lock. Gateways can supply their
/// existing local read snapshot; absence of evidence grants no extra scope.
#[derive(Debug, Default)]
pub(crate) struct ChildScopeGraph {
    reviews: Vec<ReviewScope>,
}

#[derive(Debug)]
struct ReviewScope {
    review: String,
    pr: String,
    specs: BTreeSet<String>,
}

impl ChildScopeGraph {
    pub fn from_store(store: &RequirementsStore) -> Self {
        let mut reviews = Vec::new();
        for req in &store.requirements {
            let Some((forge, number)) = req
                .title
                .split_once(':')
                .and_then(|(label, _)| label.strip_prefix("Review "))
                .and_then(crate::parse_review_scope)
            else {
                continue;
            };
            let Some(review) = req.agreed_id.as_ref().or(req.spec_id.as_ref()) else {
                continue;
            };
            // Ambiguous IDs never authorize a different requirement.
            if store
                .get_requirement_unambiguous(review)
                .ok()
                .flatten()
                .map(|r| r.id)
                != Some(req.id)
            {
                continue;
            }
            let specs = req.relationships.iter().filter(|rel| {
                matches!(&rel.rel_type, RelationshipType::Custom(n) if n.eq_ignore_ascii_case("implements"))
            }).filter_map(|rel| store.get_requirement_by_id(&rel.target_id))
                .filter_map(|target| {
                    let id = target.agreed_id.as_ref().or(target.spec_id.as_ref())?;
                    (store.get_requirement_unambiguous(id).ok().flatten().map(|r| r.id) == Some(target.id))
                        .then(|| id.clone())
                }).collect();
            reviews.push(ReviewScope {
                review: review.clone(),
                pr: crate::format_review_label(forge, number),
                specs,
            });
        }
        Self { reviews }
    }

    /// No transitive walk: sharing a reviewed spec must not authorize a child
    /// to route a different PR's work through chains of overlapping reviews.
    pub fn contains(&self, assigned: &str, target: &str) -> bool {
        self.reviews.iter().any(|r| {
            (assigned == r.review || assigned == r.pr || r.specs.contains(assigned))
                && (target == r.review || r.specs.contains(target))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::{Relationship, Requirement};

    #[test]
    fn review_scope_requires_persisted_edges_and_does_not_walk_overlapping_prs() {
        let mut a = Requirement::new("a".into(), String::new());
        a.spec_id = Some("TASK-1".into());
        let mut b = Requirement::new("b".into(), String::new());
        b.spec_id = Some("TASK-2".into());
        let mut c = Requirement::new("c".into(), String::new());
        c.spec_id = Some("TASK-3".into());
        let review = |id: &str, title: &str, targets: &[&Requirement]| {
            let mut r = Requirement::new(title.into(), String::new());
            r.spec_id = Some(id.into());
            r.relationships = targets
                .iter()
                .map(|t| Relationship {
                    rel_type: RelationshipType::Custom("implements".into()),
                    target_id: t.id,
                    created_at: None,
                    created_by: None,
                })
                .collect();
            r
        };
        let first = review(
            "STORY-1",
            "Review PR-42: TASK-3 in prose confers no scope",
            &[&a, &b],
        );
        let second = review("STORY-2", "Review MR-42: other forge", &[&b, &c]);
        let mut store = RequirementsStore::new();
        store.requirements = vec![a, b, c, first, second];
        let graph = ChildScopeGraph::from_store(&store);
        assert!(graph.contains("TASK-1", "STORY-1"));
        assert!(graph.contains("PR-42", "TASK-2"));
        assert!(graph.contains("MR-42", "TASK-3"));
        assert!(!graph.contains("PR-42", "TASK-3"), "forge/PR boundary");
        assert!(
            !graph.contains("TASK-1", "TASK-3"),
            "no transitive scope through overlapping PR"
        );
        assert!(!graph.contains("TASK-1", "STORY-2"));
        assert!(!graph.contains("PR-99", "TASK-1"));
        store.requirements[3].relationships.clear();
        let graph = ChildScopeGraph::from_store(&store);
        assert!(!graph.contains("TASK-1", "STORY-1"));
        assert!(!graph.contains("PR-42", "TASK-1"));
    }
}
