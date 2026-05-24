//! `aida findings` — triage view over findings filed by headless drain phases:
//! the reviewer (phase 3, STORY-278) as draft TASKs tagged `from-review:PR-N`,
//! and the implementer (phase 1, STORY-285) as draft TASKs tagged
//! `from-implementer:SPEC-ID`. Both phases are skill-driven (`/aida-review`
//! step 7b and `/aida-pickup` step 5b run `aida add`); this module owns the
//! deterministic *query* side the advisor uses to triage either source.
//!
//! "Findings" is a query, not a taxonomy — a finding is any draft requirement
//! carrying a `from-review:` or `from-implementer:` tag.
//! trace:STORY-278 trace:STORY-285 | ai:claude

use aida_core::RequirementSummary;

/// Tag prefix every reviewer-filed finding carries (the value is `PR-<n>`).
pub const FROM_REVIEW_PREFIX: &str = "from-review:";
/// Tag prefix every implementer-filed finding carries (the value is the
/// SPEC-ID the implementer was working when it raised the finding).
/// trace:STORY-285
pub const FROM_IMPLEMENTER_PREFIX: &str = "from-implementer:";
/// Tag prefix carrying a review finding's PR number, e.g. `from-review:PR-64`.
const PR_TAG_PREFIX: &str = "from-review:PR-";
/// Tag prefix carrying a finding's severity, e.g. `severity:major`.
const SEVERITY_PREFIX: &str = "severity:";
/// Tag prefix carrying an implementer finding's category, e.g.
/// `kind:bug-spotted`. trace:STORY-285
const KIND_PREFIX: &str = "kind:";

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

/// Which headless phase filed a finding — the top-level grouping axis of the
/// triage view, and the `--source` filter value.
//
// Variant doc comments below are clap `ValueEnum` `--help` text — keep SPEC-IDs
// out of them; the `trace:` markers stay plain `//`. trace:STORY-285 (TASK-268)
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum FindingSource {
    /// Filed by the headless reviewer (phase 3) — a `from-review:` finding.
    // trace:STORY-278 | ai:claude
    Review,
    /// Filed by the headless implementer (phase 1) — a `from-implementer:`
    /// finding.
    // trace:STORY-285 | ai:claude
    Implementer,
}

impl FindingSource {
    /// Display label for the triage view's section header.
    pub fn label(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Implementer => "implementer",
        }
    }

    /// The tag prefix findings from this source carry.
    fn tag_prefix(self) -> &'static str {
        match self {
            Self::Review => FROM_REVIEW_PREFIX,
            Self::Implementer => FROM_IMPLEMENTER_PREFIX,
        }
    }
}

/// Extract the PR number from a `from-review:PR-<n>` tag. Returns `None` for
/// any other tag or an unparseable suffix.
pub fn pr_number_from_tag(tag: &str) -> Option<u32> {
    tag.strip_prefix(PR_TAG_PREFIX)?.parse().ok()
}

/// The phase that filed a finding, or `None` when `tags` carries neither
/// finding tag (an ordinary draft, not a finding). Review wins if — against
/// expectation — both prefixes are present.
pub fn finding_source(tags: &[String]) -> Option<FindingSource> {
    if tags.iter().any(|t| t.starts_with(FROM_REVIEW_PREFIX)) {
        Some(FindingSource::Review)
    } else if tags.iter().any(|t| t.starts_with(FROM_IMPLEMENTER_PREFIX)) {
        Some(FindingSource::Implementer)
    } else {
        None
    }
}

/// True when `tags` marks the requirement as a finding from either headless
/// phase. Guards `aida findings dismiss/promote` so they only act on real
/// findings — review *or* implementer. trace:STORY-285
pub fn is_finding(tags: &[String]) -> bool {
    finding_source(tags).is_some()
}

/// The PR a review finding was raised against, from its `from-review:PR-<n>`
/// tag. Used by the `--pr` filter (a review-only axis).
fn finding_pr(tags: &[String]) -> Option<u32> {
    tags.iter().find_map(|t| pr_number_from_tag(t))
}

/// The thing a finding was raised against, for the triage view's sub-header:
/// `PR-<n>` for a review finding, the SPEC-ID for an implementer finding.
/// Falls back to a placeholder when the source tag carries no value.
fn finding_origin(tags: &[String], source: FindingSource) -> String {
    let prefix = source.tag_prefix();
    tags.iter()
        .find_map(|t| t.strip_prefix(prefix))
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .unwrap_or_else(|| match source {
            FindingSource::Review => "(no PR tag)".to_string(),
            FindingSource::Implementer => "(no spec tag)".to_string(),
        })
}

/// A finding's `kind:<category>` tag value. Implementer findings carry one
/// (`deviation`, `design-choice`, `bug-spotted`, `followup-suggestion`);
/// review findings do not. trace:STORY-285
fn finding_kind(tags: &[String]) -> Option<String> {
    tags.iter()
        .find_map(|t| t.strip_prefix(KIND_PREFIX))
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
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
    /// The `kind:` category, when the finding carries one (implementer
    /// findings do; review findings don't). trace:STORY-285
    pub kind: Option<String>,
}

/// Findings grouped under the origin that raised them — a PR for review
/// findings, a SPEC-ID for implementer findings.
#[derive(Debug, Clone)]
pub struct OriginGroup {
    /// `PR-<n>` or a SPEC-ID; a `(no … tag)` placeholder when unparseable.
    pub origin: String,
    pub rows: Vec<FindingRow>,
}

/// All findings from one source (review / implementer), origin-grouped — the
/// top-level section of the triage view. trace:STORY-285
#[derive(Debug, Clone)]
pub struct SourceSection {
    pub source: FindingSource,
    pub groups: Vec<OriginGroup>,
}

/// Filters applied to the triage view by `aida findings list`. Every field
/// defaults to "no filter". trace:STORY-285
#[derive(Debug, Clone, Default)]
pub struct FindingsFilter {
    /// Narrow review findings to one PR number (excludes implementer
    /// findings — they have no PR).
    pub pr: Option<u32>,
    /// Narrow to one filing source.
    pub source: Option<FindingSource>,
    /// Narrow to findings carrying `kind:<value>`.
    pub kind: Option<String>,
}

/// Build the triage view: keep only finding requirements, apply `filter`, and
/// group source → origin → rows.
///
/// Sections are ordered Review then Implementer. Within Review, origin groups
/// are PR-number descending (most recent merge leads, `(no PR tag)` last);
/// within Implementer they preserve `summaries` order (modified_at-DESC —
/// freshest spec first). Each origin group's rows sort major → minor →
/// cosmetic; the severity sort is stable, so within one severity the freshest
/// finding still leads. Empty sections are dropped.
/// trace:STORY-278 trace:STORY-285 | ai:claude
pub fn build_findings_view(
    summaries: &[RequirementSummary],
    filter: &FindingsFilter,
) -> Vec<SourceSection> {
    let mut sections = Vec::new();
    for source in [FindingSource::Review, FindingSource::Implementer] {
        if let Some(want) = filter.source {
            if want != source {
                continue;
            }
        }
        let section = build_source_section(summaries, source, filter);
        if !section.groups.is_empty() {
            sections.push(section);
        }
    }
    sections
}

/// Build one source's section: collect its findings, apply the `--pr` /
/// `--kind` filters, and group by origin.
fn build_source_section(
    summaries: &[RequirementSummary],
    source: FindingSource,
    filter: &FindingsFilter,
) -> SourceSection {
    // `--pr` is a review-only axis: an implementer finding has no PR, so a
    // PR filter excludes the implementer section entirely.
    if filter.pr.is_some() && source == FindingSource::Implementer {
        return SourceSection {
            source,
            groups: Vec::new(),
        };
    }

    // Record first-appearance order of origins (summaries arrive
    // modified_at-DESC) so review origins can be re-sorted by PR afterwards
    // while implementer origins keep recency order.
    let mut order: Vec<String> = Vec::new();
    let mut by_origin: std::collections::HashMap<String, Vec<FindingRow>> =
        std::collections::HashMap::new();

    for s in summaries {
        if finding_source(&s.tags) != Some(source) {
            continue;
        }
        if let Some(want) = filter.pr {
            if finding_pr(&s.tags) != Some(want) {
                continue;
            }
        }
        let kind = finding_kind(&s.tags);
        if let Some(want) = &filter.kind {
            if kind.as_deref() != Some(want.as_str()) {
                continue;
            }
        }
        let origin = finding_origin(&s.tags, source);
        let display_id = s
            .agreed_id
            .clone()
            .or_else(|| s.spec_id.clone())
            .unwrap_or_else(|| "?".to_string());
        let row = FindingRow {
            display_id,
            title: s.title.clone(),
            severity: finding_severity(&s.tags),
            kind,
        };
        if !by_origin.contains_key(&origin) {
            order.push(origin.clone());
        }
        by_origin.entry(origin).or_default().push(row);
    }

    // Review origins re-sort by PR descending (the `(no PR tag)` bucket and
    // any unparseable origin sort last); implementer origins keep the
    // first-appearance recency order.
    if source == FindingSource::Review {
        order.sort_by_key(|o| {
            std::cmp::Reverse(o.strip_prefix("PR-").and_then(|n| n.parse::<u32>().ok()))
        });
    }

    let groups = order
        .into_iter()
        .map(|origin| {
            let mut rows = by_origin.remove(&origin).unwrap_or_default();
            rows.sort_by_key(|r| r.severity.rank());
            OriginGroup { origin, rows }
        })
        .collect();

    SourceSection { source, groups }
}

/// Total finding rows across every section — what `aida findings list
/// --count` prints for session-start surfacing.
pub fn count_findings(sections: &[SourceSection]) -> usize {
    sections
        .iter()
        .flat_map(|s| &s.groups)
        .map(|g| g.rows.len())
        .sum()
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
            archived_at: None,
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
    fn finding_source_classifies_by_tag() {
        assert_eq!(
            finding_source(&["from-review:PR-1".to_string()]),
            Some(FindingSource::Review)
        );
        assert_eq!(
            finding_source(&["from-implementer:STORY-9".to_string()]),
            Some(FindingSource::Implementer)
        );
        assert_eq!(
            finding_source(&["severity:major".to_string(), "clippy".to_string()]),
            None
        );
        assert_eq!(finding_source(&[]), None);
    }

    #[test]
    fn is_finding_accepts_either_source() {
        assert!(is_finding(&["from-review:PR-1".to_string()]));
        assert!(is_finding(&["from-implementer:TASK-5".to_string()]));
        assert!(!is_finding(&[
            "severity:major".to_string(),
            "clippy".to_string(),
        ]));
        assert!(!is_finding(&[]));
    }

    #[test]
    fn build_findings_view_groups_review_by_pr_and_sorts_by_severity() {
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
        let view = build_findings_view(&summaries, &FindingsFilter::default());
        // One Review section, two origin groups, PR-70 first (descending).
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].source, FindingSource::Review);
        assert_eq!(view[0].groups.len(), 2);
        assert_eq!(view[0].groups[0].origin, "PR-70");
        assert_eq!(view[0].groups[1].origin, "PR-64");
        // PR-64 group sorted major → minor → cosmetic.
        let pr64: Vec<&str> = view[0].groups[1]
            .rows
            .iter()
            .map(|r| r.display_id.as_str())
            .collect();
        assert_eq!(pr64, vec!["TASK-2", "TASK-3", "TASK-1"]);
    }

    #[test]
    fn build_findings_view_groups_implementer_by_spec() {
        let summaries = vec![
            summary(
                "TASK-9",
                "story285 bug",
                &[
                    "from-implementer:STORY-285",
                    "kind:bug-spotted",
                    "severity:major",
                ],
            ),
            summary(
                "TASK-10",
                "story285 deviation",
                &[
                    "from-implementer:STORY-285",
                    "kind:deviation",
                    "severity:minor",
                ],
            ),
        ];
        let view = build_findings_view(&summaries, &FindingsFilter::default());
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].source, FindingSource::Implementer);
        assert_eq!(view[0].groups.len(), 1);
        assert_eq!(view[0].groups[0].origin, "STORY-285");
        assert_eq!(view[0].groups[0].rows.len(), 2);
        assert_eq!(
            view[0].groups[0].rows[0].kind.as_deref(),
            Some("bug-spotted")
        );
    }

    #[test]
    fn build_findings_view_separates_sources_review_first() {
        let summaries = vec![
            summary("TASK-1", "review", &["from-review:PR-64", "severity:major"]),
            summary(
                "TASK-2",
                "impl",
                &["from-implementer:STORY-9", "kind:deviation"],
            ),
        ];
        let view = build_findings_view(&summaries, &FindingsFilter::default());
        assert_eq!(view.len(), 2);
        assert_eq!(view[0].source, FindingSource::Review);
        assert_eq!(view[1].source, FindingSource::Implementer);
        assert_eq!(count_findings(&view), 2);
    }

    #[test]
    fn build_findings_view_source_filter_narrows_to_one_phase() {
        let summaries = vec![
            summary("TASK-1", "review", &["from-review:PR-64"]),
            summary("TASK-2", "impl", &["from-implementer:STORY-9"]),
        ];
        let view = build_findings_view(
            &summaries,
            &FindingsFilter {
                source: Some(FindingSource::Implementer),
                ..Default::default()
            },
        );
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].source, FindingSource::Implementer);
        assert_eq!(view[0].groups[0].rows[0].display_id, "TASK-2");
    }

    #[test]
    fn build_findings_view_kind_filter_narrows_to_category() {
        let summaries = vec![
            summary(
                "TASK-1",
                "bug",
                &["from-implementer:STORY-9", "kind:bug-spotted"],
            ),
            summary(
                "TASK-2",
                "deviation",
                &["from-implementer:STORY-9", "kind:deviation"],
            ),
            summary("TASK-3", "review (no kind)", &["from-review:PR-64"]),
        ];
        let view = build_findings_view(
            &summaries,
            &FindingsFilter {
                kind: Some("bug-spotted".to_string()),
                ..Default::default()
            },
        );
        // Only the bug-spotted implementer finding survives — the review
        // finding has no `kind:` tag, so a kind filter drops it.
        assert_eq!(count_findings(&view), 1);
        assert_eq!(view[0].source, FindingSource::Implementer);
        assert_eq!(view[0].groups[0].rows[0].display_id, "TASK-1");
    }

    #[test]
    fn build_findings_view_pr_filter_narrows_and_excludes_implementer() {
        let summaries = vec![
            summary("TASK-1", "pr64", &["from-review:PR-64", "severity:major"]),
            summary("TASK-2", "pr70", &["from-review:PR-70", "severity:major"]),
            summary("TASK-3", "impl", &["from-implementer:STORY-9"]),
        ];
        let view = build_findings_view(
            &summaries,
            &FindingsFilter {
                pr: Some(64),
                ..Default::default()
            },
        );
        // Only the PR-64 review finding — PR-70 filtered out, implementer
        // findings have no PR so they cannot match.
        assert_eq!(count_findings(&view), 1);
        assert_eq!(view[0].source, FindingSource::Review);
        assert_eq!(view[0].groups[0].origin, "PR-64");
        assert_eq!(view[0].groups[0].rows[0].display_id, "TASK-1");
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
        let view = build_findings_view(&summaries, &FindingsFilter::default());
        assert_eq!(count_findings(&view), 1);
        assert_eq!(view[0].groups[0].rows[0].display_id, "TASK-1");
    }
}
