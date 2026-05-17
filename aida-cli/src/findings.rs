//! `aida findings` — triage view over review findings filed by the headless
//! reviewer as draft TASKs tagged `from-review:PR-N`. The reviewer side is
//! skill-driven (`/aida-review` step 7b runs `aida add`); this module owns the
//! deterministic *query* side the advisor uses to triage.
//!
//! "Findings" is a query, not a taxonomy — a finding is any draft requirement
//! carrying a `from-review:` tag. trace:STORY-278 | ai:claude

use aida_core::RequirementSummary;

/// Tag prefix every reviewer-filed finding carries.
pub const FROM_REVIEW_PREFIX: &str = "from-review:";
/// Tag prefix carrying a finding's PR number, e.g. `from-review:PR-64`.
const PR_TAG_PREFIX: &str = "from-review:PR-";
/// Tag prefix carrying a finding's severity, e.g. `severity:major`.
const SEVERITY_PREFIX: &str = "severity:";

/// A finding's severity, parsed from its `severity:<level>` tag. Ordered so
/// [`Severity::rank`] sorts a triage view major → minor → cosmetic, with
/// unknown (no/garbled tag) last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Major,
    Minor,
    Cosmetic,
    Unknown,
}

impl Severity {
    /// Sort key for the triage view — lower sorts first.
    pub fn rank(self) -> u8 {
        match self {
            Self::Major => 0,
            Self::Minor => 1,
            Self::Cosmetic => 2,
            Self::Unknown => 3,
        }
    }

    /// Display label for the triage view.
    pub fn label(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
            Self::Cosmetic => "cosmetic",
            Self::Unknown => "unknown",
        }
    }

    /// Parse the value of a `severity:` tag. Tolerant of casing/whitespace;
    /// anything unrecognised resolves to [`Severity::Unknown`].
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "major" => Self::Major,
            "minor" => Self::Minor,
            "cosmetic" => Self::Cosmetic,
            _ => Self::Unknown,
        }
    }
}

/// Extract the PR number from a `from-review:PR-<n>` tag. Returns `None` for
/// any other tag or an unparseable suffix.
pub fn pr_number_from_tag(tag: &str) -> Option<u32> {
    tag.strip_prefix(PR_TAG_PREFIX)?.parse().ok()
}

/// True when `tags` carries any `from-review:` tag — i.e. the requirement is a
/// reviewer-filed finding. Guards `aida findings dismiss/promote` so they only
/// act on real findings.
pub fn is_review_finding(tags: &[String]) -> bool {
    tags.iter().any(|t| t.starts_with(FROM_REVIEW_PREFIX))
}

/// The PR a finding was raised against, from its `from-review:PR-<n>` tag.
fn finding_pr(tags: &[String]) -> Option<u32> {
    tags.iter().find_map(|t| pr_number_from_tag(t))
}

/// A finding's severity, from its `severity:<level>` tag (`Unknown` if absent).
fn finding_severity(tags: &[String]) -> Severity {
    tags.iter()
        .find_map(|t| t.strip_prefix(SEVERITY_PREFIX))
        .map(Severity::parse)
        .unwrap_or(Severity::Unknown)
}

/// One row in the triage view.
#[derive(Debug, Clone)]
pub struct FindingRow {
    pub display_id: String,
    pub title: String,
    pub severity: Severity,
}

/// Findings grouped under the PR that raised them.
#[derive(Debug, Clone)]
pub struct PrGroup {
    /// `None` when the finding's `from-review:` tag carried no parseable PR.
    pub pr: Option<u32>,
    pub rows: Vec<FindingRow>,
}

/// Build the triage view: keep only `from-review:` findings, optionally narrow
/// to one PR, group by PR (PR-number descending so the most recent merge leads,
/// `None`/unparseable last) and sort each group major → minor → cosmetic.
///
/// `summaries` arrives modified_at-DESC from `list_summaries`; the severity
/// sort is stable, so within one severity the freshest finding still leads.
/// trace:STORY-278 | ai:claude
pub fn build_findings_view(
    summaries: &[RequirementSummary],
    pr_filter: Option<u32>,
) -> Vec<PrGroup> {
    use std::cmp::Reverse;
    use std::collections::BTreeMap;

    // Key on (group-class, Reverse(pr)) so the BTreeMap iterates real PRs
    // descending, then the no-PR bucket last. The class byte keeps the no-PR
    // bucket from colliding with a real `PR-0`.
    let mut groups: BTreeMap<(u8, Reverse<u32>), Vec<FindingRow>> = BTreeMap::new();
    for s in summaries {
        if !is_review_finding(&s.tags) {
            continue;
        }
        let pr = finding_pr(&s.tags);
        if let Some(want) = pr_filter {
            if pr != Some(want) {
                continue;
            }
        }
        let key = match pr {
            Some(n) => (0u8, Reverse(n)),
            None => (1u8, Reverse(0)),
        };
        let display_id = s
            .agreed_id
            .clone()
            .or_else(|| s.spec_id.clone())
            .unwrap_or_else(|| "?".to_string());
        groups.entry(key).or_default().push(FindingRow {
            display_id,
            title: s.title.clone(),
            severity: finding_severity(&s.tags),
        });
    }

    groups
        .into_iter()
        .map(|((class, rev), mut rows)| {
            rows.sort_by_key(|r| r.severity.rank());
            let pr = if class == 0 { Some(rev.0) } else { None };
            PrGroup { pr, rows }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(spec_id: &str, title: &str, tags: &[&str]) -> RequirementSummary {
        RequirementSummary {
            id: uuid::Uuid::nil(),
            spec_id: Some(spec_id.to_string()),
            agreed_id: None,
            title: title.to_string(),
            description: String::new(),
            status: "Draft".to_string(),
            priority: "Medium".to_string(),
            owner: "tester".to_string(),
            feature: "Uncategorized".to_string(),
            req_type: "Task".to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            created_at: String::new(),
            modified_at: String::new(),
            archived: false,
            yaml_path: String::new(),
        }
    }

    #[test]
    fn severity_rank_orders_major_minor_cosmetic() {
        assert!(Severity::Major.rank() < Severity::Minor.rank());
        assert!(Severity::Minor.rank() < Severity::Cosmetic.rank());
        assert!(Severity::Cosmetic.rank() < Severity::Unknown.rank());
    }

    #[test]
    fn severity_parse_is_tolerant() {
        assert_eq!(Severity::parse("major"), Severity::Major);
        assert_eq!(Severity::parse("  Cosmetic "), Severity::Cosmetic);
        assert_eq!(Severity::parse("MINOR"), Severity::Minor);
        assert_eq!(Severity::parse("bogus"), Severity::Unknown);
        assert_eq!(Severity::parse(""), Severity::Unknown);
    }

    #[test]
    fn pr_number_from_tag_parses_and_rejects() {
        assert_eq!(pr_number_from_tag("from-review:PR-64"), Some(64));
        assert_eq!(pr_number_from_tag("from-review:PR-7"), Some(7));
        assert_eq!(pr_number_from_tag("from-review:PR-x"), None);
        assert_eq!(pr_number_from_tag("from-review:64"), None);
        assert_eq!(pr_number_from_tag("severity:major"), None);
        assert_eq!(pr_number_from_tag("clippy"), None);
    }

    #[test]
    fn is_review_finding_requires_from_review_tag() {
        assert!(is_review_finding(&["from-review:PR-1".to_string()]));
        assert!(is_review_finding(&[
            "clippy".to_string(),
            "from-review:PR-9".to_string(),
        ]));
        assert!(!is_review_finding(&[
            "severity:major".to_string(),
            "clippy".to_string(),
        ]));
        assert!(!is_review_finding(&[]));
    }

    #[test]
    fn build_findings_view_groups_by_pr_and_sorts_by_severity() {
        let summaries = vec![
            summary(
                "TASK-1",
                "pr64 cosmetic",
                &["from-review:PR-64", "severity:cosmetic"],
            ),
            summary(
                "TASK-2",
                "pr64 major",
                &["from-review:PR-64", "severity:major"],
            ),
            summary(
                "TASK-3",
                "pr64 minor",
                &["from-review:PR-64", "severity:minor"],
            ),
            summary(
                "TASK-4",
                "pr70 minor",
                &["from-review:PR-70", "severity:minor"],
            ),
        ];
        let view = build_findings_view(&summaries, None);
        // Two groups, PR-70 first (descending), PR-64 second.
        assert_eq!(view.len(), 2);
        assert_eq!(view[0].pr, Some(70));
        assert_eq!(view[1].pr, Some(64));
        // PR-64 group sorted major → minor → cosmetic.
        let pr64: Vec<&str> = view[1].rows.iter().map(|r| r.display_id.as_str()).collect();
        assert_eq!(pr64, vec!["TASK-2", "TASK-3", "TASK-1"]);
    }

    #[test]
    fn build_findings_view_pr_filter_narrows() {
        let summaries = vec![
            summary("TASK-1", "pr64", &["from-review:PR-64", "severity:major"]),
            summary("TASK-2", "pr70", &["from-review:PR-70", "severity:major"]),
        ];
        let view = build_findings_view(&summaries, Some(64));
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].pr, Some(64));
        assert_eq!(view[0].rows.len(), 1);
        assert_eq!(view[0].rows[0].display_id, "TASK-1");
    }

    #[test]
    fn build_findings_view_drops_non_finding_drafts() {
        let summaries = vec![
            summary(
                "TASK-1",
                "real finding",
                &["from-review:PR-64", "severity:minor"],
            ),
            summary("STORY-9", "ordinary draft", &["clippy", "severity:major"]),
        ];
        let view = build_findings_view(&summaries, None);
        let total: usize = view.iter().map(|g| g.rows.len()).sum();
        assert_eq!(total, 1);
        assert_eq!(view[0].rows[0].display_id, "TASK-1");
    }
}
