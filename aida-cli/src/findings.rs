//! `aida findings` — triage view over findings filed by headless drain phases
//! and (STORY-467) the advisor seat:
//! the reviewer (phase 3, STORY-278) as draft TASKs tagged `from-review:PR-N`,
//! the implementer (phase 1, STORY-285) as draft TASKs tagged
//! `from-implementer:SPEC-ID`, and the advisor (`aida findings add`,
//! STORY-467) as draft TASKs tagged `from-advisor:<origin>`. The drain phases
//! are skill-driven (`/aida-review` step 7b and `/aida-pickup` step 5b run
//! `aida add`); the advisor entry is a first-class subcommand so live-session
//! observations don't decay with context. This module owns the deterministic
//! *query* side the advisor uses to triage every source.
//!
//! "Findings" is a query, not a taxonomy — a finding is any draft requirement
//! carrying a `from-review:`, `from-implementer:`, or `from-advisor:` tag.
//! trace:STORY-278 trace:STORY-285 trace:STORY-467 | ai:claude

use aida_core::RequirementSummary;

/// Tag prefix every reviewer-filed finding carries (the value is `PR-<n>`).
pub const FROM_REVIEW_PREFIX: &str = "from-review:";
/// Tag prefix every implementer-filed finding carries (the value is the
/// SPEC-ID the implementer was working when it raised the finding).
/// trace:STORY-285
pub const FROM_IMPLEMENTER_PREFIX: &str = "from-implementer:";
/// Tag prefix every advisor-filed observation carries. The value is the
/// origin bucket: the first `--linked-specs` value (a SPEC-ID), or
/// `general` when the observation isn't anchored to one spec.
/// trace:STORY-467
pub const FROM_ADVISOR_PREFIX: &str = "from-advisor:";
/// Tag prefix carrying a review finding's PR number, e.g. `from-review:PR-64`.
const PR_TAG_PREFIX: &str = "from-review:PR-";
/// Tag prefix carrying a finding's severity, e.g. `severity:major`.
pub(crate) const SEVERITY_PREFIX: &str = "severity:";
/// Tag prefix carrying an implementer or advisor finding's category, e.g.
/// `kind:bug-spotted`, `kind:observation`. trace:STORY-285 trace:STORY-467
pub(crate) const KIND_PREFIX: &str = "kind:";
/// Tag prefix carrying a recurrence counter, e.g. `recurrence:3`. Filed
/// findings start without the tag (implicit recurrence:1); the first
/// `aida findings recur <ID>` writes `recurrence:2`. trace:STORY-467
pub const RECURRENCE_PREFIX: &str = "recurrence:";
/// Tag prefix carrying an additional linked spec when an advisor observation
/// names more than one (the first goes into `from-advisor:<spec>`, the rest
/// become `linked:<spec>` so they survive grep and show up in the spec's
/// own relationship-by-tag view). trace:STORY-467
pub const LINKED_PREFIX: &str = "linked:";

/// A finding's severity, parsed from its `severity:<level>` tag. Ordered so
/// [`Severity::rank`] sorts a triage view major → minor → cosmetic, with
/// unknown (no/garbled tag) last.
///
/// Vocabulary (in descending sort-priority order):
///   - major     — should fix; the finding describes a real defect or risk
///   - minor     — should fix; smaller scope than major
///   - cosmetic  — could fix; nice-to-have polish
///   - observation — pattern noted, not a defect; capture-only (TASK-120)
///   - note      — informational only, low-priority capture (TASK-120)
///   - unknown   — no/garbled tag
///
/// trace:STORY-467 trace:TASK-120 | ai:claude
// trace:TASK-714
#[derive(Debug, Clone, Copy, PartialEq, Eq, ts_rs_forge::TS)]
pub enum Severity {
    Major,
    Minor,
    Cosmetic,
    Observation,
    Note,
    Unknown,
}

impl Severity {
    /// Sort key for the triage view — lower sorts first.
    pub fn rank(self) -> u8 {
        match self {
            Self::Major => 0,
            Self::Minor => 1,
            Self::Cosmetic => 2,
            Self::Observation => 3,
            Self::Note => 4,
            Self::Unknown => 5,
        }
    }

    /// Display label for the triage view.
    pub fn label(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
            Self::Cosmetic => "cosmetic",
            Self::Observation => "observation",
            Self::Note => "note",
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
            "observation" => Self::Observation,
            "note" => Self::Note,
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
    /// Filed by the advisor via `aida findings add` — a `from-advisor:`
    /// observation.
    // trace:STORY-467 | ai:claude
    Advisor,
}

impl FindingSource {
    /// Display label for the triage view's section header.
    pub fn label(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Implementer => "implementer",
            Self::Advisor => "advisor",
        }
    }

    /// The tag prefix findings from this source carry.
    fn tag_prefix(self) -> &'static str {
        match self {
            Self::Review => FROM_REVIEW_PREFIX,
            Self::Implementer => FROM_IMPLEMENTER_PREFIX,
            Self::Advisor => FROM_ADVISOR_PREFIX,
        }
    }
}

/// Extract the PR number from a `from-review:PR-<n>` tag. Returns `None` for
/// any other tag or an unparseable suffix.
pub fn pr_number_from_tag(tag: &str) -> Option<u32> {
    tag.strip_prefix(PR_TAG_PREFIX)?.parse().ok()
}

/// The phase that filed a finding, or `None` when `tags` carries no finding
/// tag (an ordinary draft, not a finding). Tie-break order: review → implementer
/// → advisor. STORY-467: the advisor variant is checked last so a drain-filed
/// finding that picks up a `from-advisor:` linked-spec tag (unlikely in
/// practice — drains don't write that prefix — but defensive against future
/// changes) keeps its drain-source identity.
pub fn finding_source(tags: &[String]) -> Option<FindingSource> {
    if tags.iter().any(|t| t.starts_with(FROM_REVIEW_PREFIX)) {
        Some(FindingSource::Review)
    } else if tags.iter().any(|t| t.starts_with(FROM_IMPLEMENTER_PREFIX)) {
        Some(FindingSource::Implementer)
    } else if tags.iter().any(|t| t.starts_with(FROM_ADVISOR_PREFIX)) {
        Some(FindingSource::Advisor)
    } else {
        None
    }
}

/// True when `tags` marks the requirement as a finding. Guards
/// `aida findings dismiss/promote/recur` so they only act on real findings —
/// review, implementer, or (STORY-467) advisor. trace:STORY-285 trace:STORY-467
pub fn is_finding(tags: &[String]) -> bool {
    finding_source(tags).is_some()
}

/// The PR a review finding was raised against, from its `from-review:PR-<n>`
/// tag. Used by the `--pr` filter (a review-only axis).
fn finding_pr(tags: &[String]) -> Option<u32> {
    tags.iter().find_map(|t| pr_number_from_tag(t))
}

/// The thing a finding was raised against, for the triage view's sub-header:
/// `PR-<n>` for a review finding, the SPEC-ID for an implementer finding,
/// the linked-spec or `general` for an advisor observation. Falls back to a
/// placeholder when the source tag carries no value.
fn finding_origin(tags: &[String], source: FindingSource) -> String {
    let prefix = source.tag_prefix();
    tags.iter()
        .find_map(|t| t.strip_prefix(prefix))
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .unwrap_or_else(|| match source {
            FindingSource::Review => "(no PR tag)".to_string(),
            FindingSource::Implementer => "(no spec tag)".to_string(),
            FindingSource::Advisor => "(no origin tag)".to_string(),
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

/// A finding's recurrence count: the value of its `recurrence:N` tag, or `1`
/// when the tag is absent (the first sighting is implicit). STORY-467: the
/// counter increments via `aida findings recur <ID>`; the triage view shows
/// `×N` next to any row with N > 1 so frequently-recurring observations
/// stand out without needing an extra column for the common N=1 case.
pub fn finding_recurrence(tags: &[String]) -> u32 {
    tags.iter()
        .find_map(|t| t.strip_prefix(RECURRENCE_PREFIX))
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(1)
}

/// Default recurrence count above which the recur handler emits the
/// promote-it hint. Configurable via `[findings] promote_threshold = N`
/// in `.aida/config.toml`. trace:TASK-37 | ai:claude
pub const DEFAULT_PROMOTE_THRESHOLD: u32 = 3;

/// Resolve the promote-it threshold for a project. Reads
/// `[findings] promote_threshold = N` from `.aida/config.toml` (clamped
/// to >= 1; values <= 0 fall back to the default). Returns the default
/// (`DEFAULT_PROMOTE_THRESHOLD`) when the file is absent or the key
/// isn't set. Errors are swallowed — config reading never blocks the
/// caller. trace:TASK-37 | ai:claude
pub fn promote_threshold_for_project(project_dir: Option<&std::path::Path>) -> u32 {
    let Some(dir) = project_dir else {
        return DEFAULT_PROMOTE_THRESHOLD;
    };
    let path = dir.join(".aida").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return DEFAULT_PROMOTE_THRESHOLD;
    };
    parse_promote_threshold(&content).unwrap_or(DEFAULT_PROMOTE_THRESHOLD)
}

/// Parse `[findings] promote_threshold = N` out of a TOML string.
/// Returns `Some(N)` for any N >= 1, `None` otherwise. Mirrors
/// `parse_telemetry_enabled` in `usage.rs` — a hand-rolled TOML-ish
/// parser so the read path never pulls a full TOML dependency just to
/// look up one value. trace:TASK-37 | ai:claude
pub fn parse_promote_threshold(content: &str) -> Option<u32> {
    let mut in_findings = false;
    for raw in content.lines() {
        let line = raw.split('#').next()?.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_findings = rest.trim_end_matches(']').trim() == "findings";
            continue;
        }
        if !in_findings {
            continue;
        }
        if let Some(rest) = line.strip_prefix("promote_threshold") {
            let val = rest.split('=').nth(1)?.trim().trim_matches('"');
            return val.parse::<u32>().ok().filter(|n| *n >= 1);
        }
    }
    None
}

/// One row in the triage view.
// trace:TASK-714
#[derive(Debug, Clone, ts_rs_forge::TS)]
pub struct FindingRow {
    pub display_id: String,
    pub title: String,
    pub severity: Severity,
    /// The `kind:` category, when the finding carries one (implementer
    /// findings do; review findings don't; advisor observations do).
    /// trace:STORY-285 trace:STORY-467
    pub kind: Option<String>,
    /// Recurrence count (`recurrence:N` tag value). Defaults to 1 for a
    /// freshly-filed finding; `aida findings recur <ID>` increments it.
    /// trace:STORY-467
    pub recurrence: u32,
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
    for source in [
        FindingSource::Review,
        FindingSource::Implementer,
        FindingSource::Advisor,
    ] {
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
            recurrence: finding_recurrence(&s.tags),
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

/// Label prefixing an origin-group header — the spec (or PR) the findings in
/// the group are ABOUT. Distinct from [`FINDING_LABEL`] so a reader can't
/// mistake the linked spec for the finding's own id.
// trace:BUG-641 | ai:claude
pub const ABOUT_LABEL: &str = "about";

/// Label prefixing a finding row — the finding's OWN id.
// trace:BUG-641 | ai:claude
pub const FINDING_LABEL: &str = "finding";

/// Render the origin-group header for the triage view: the spec or PR the
/// findings in the group are ABOUT, prefixed with [`ABOUT_LABEL`] so it is
/// plainly the *linked* spec, never the finding's own id. The caller colours
/// the returned string.
// trace:BUG-641 | ai:claude
pub fn render_origin_header(origin: &str) -> String {
    format!("{ABOUT_LABEL} {origin}")
}

/// Render one labeled triage row. The finding's OWN id leads the line behind
/// the [`FINDING_LABEL`] prefix, so paired with the [`render_origin_header`]
/// `about <spec>` line above it the two ids are unambiguous: `about STORY-9`
/// is the linked spec, `finding TASK-12` is the finding itself. Returns plain
/// (uncoloured) text; the caller indents and prints it.
// trace:BUG-641 | ai:claude
pub fn render_finding_row(row: &FindingRow) -> String {
    // STORY-467: recurrence >= 2 prints a `×N` suffix; the common N=1 case
    // stays clean.
    let recur_suffix = if row.recurrence > 1 {
        format!(" ×{}", row.recurrence)
    } else {
        String::new()
    };
    match &row.kind {
        // Implementer + advisor findings carry a `kind:` category; review
        // findings don't. trace:STORY-285 trace:STORY-467
        Some(k) => format!(
            "{FINDING_LABEL} {:<14} {:<9} {:<20} {}{}",
            row.display_id,
            row.severity.label(),
            k,
            row.title,
            recur_suffix,
        ),
        None => format!(
            "{FINDING_LABEL} {:<14} {:<9} {}{}",
            row.display_id,
            row.severity.label(),
            row.title,
            recur_suffix,
        ),
    }
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
            assignee: None,
            feature: "Uncategorized".to_string(),
            req_type: "Task".to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            created_at: String::new(),
            modified_at: String::new(),
            archived: false,
            archived_at: None,
            deferred: false,
            deferred_at: None,
            deferred_until: None,
            in_degree: 0,
            out_degree: 0,
            heft: 0,
            // trace:TASK-902 | ai:claude
            blocked: false,
            // trace:TASK-1065 | ai:claude
            has_pending_decision: false,
            yaml_path: String::new(),
        }
    }

    #[test]
    fn severity_rank_orders_major_minor_cosmetic() {
        assert!(Severity::Major.rank() < Severity::Minor.rank());
        assert!(Severity::Minor.rank() < Severity::Cosmetic.rank());
        // TASK-120: lighter-weight findings rank below cosmetic but
        // above unknown, so they still surface in the triage view.
        assert!(Severity::Cosmetic.rank() < Severity::Observation.rank());
        assert!(Severity::Observation.rank() < Severity::Note.rank());
        assert!(Severity::Note.rank() < Severity::Unknown.rank());
    }

    #[test]
    fn severity_parse_is_tolerant() {
        assert_eq!(Severity::parse("major"), Severity::Major);
        assert_eq!(Severity::parse("  Cosmetic "), Severity::Cosmetic);
        assert_eq!(Severity::parse("MINOR"), Severity::Minor);
        // TASK-120: observation + note added to the vocabulary.
        assert_eq!(Severity::parse("observation"), Severity::Observation);
        assert_eq!(Severity::parse("  Observation "), Severity::Observation);
        assert_eq!(Severity::parse("OBSERVATION"), Severity::Observation);
        assert_eq!(Severity::parse("note"), Severity::Note);
        assert_eq!(Severity::parse("NOTE"), Severity::Note);
        assert_eq!(Severity::parse("bogus"), Severity::Unknown);
        assert_eq!(Severity::parse(""), Severity::Unknown);
    }

    #[test]
    fn severity_label_round_trips() {
        // Each variant's label parses back to the same variant — guards
        // against future drift between label() and parse(). trace:TASK-120
        for sev in [
            Severity::Major,
            Severity::Minor,
            Severity::Cosmetic,
            Severity::Observation,
            Severity::Note,
        ] {
            assert_eq!(Severity::parse(sev.label()), sev);
        }
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
    fn finding_source_recognises_advisor_tag() {
        assert_eq!(
            finding_source(&["from-advisor:STORY-9".to_string()]),
            Some(FindingSource::Advisor)
        );
        assert_eq!(
            finding_source(&["from-advisor:general".to_string()]),
            Some(FindingSource::Advisor)
        );
        // Review still wins when both prefixes are present — drains never
        // write `from-advisor:` but the defensive ordering keeps a
        // drain-source identity if they ever do.
        assert_eq!(
            finding_source(&[
                "from-review:PR-1".to_string(),
                "from-advisor:STORY-9".to_string(),
            ]),
            Some(FindingSource::Review)
        );
    }

    #[test]
    fn is_finding_accepts_advisor_source() {
        assert!(is_finding(&["from-advisor:STORY-9".to_string()]));
        assert!(is_finding(&["from-advisor:general".to_string()]));
    }

    #[test]
    fn finding_recurrence_defaults_to_one_and_parses_tag() {
        assert_eq!(finding_recurrence(&[]), 1);
        assert_eq!(finding_recurrence(&["clippy".to_string()]), 1);
        assert_eq!(finding_recurrence(&["recurrence:3".to_string()]), 3);
        // Garbled or zero values fall back to the implicit count.
        assert_eq!(finding_recurrence(&["recurrence:0".to_string()]), 1);
        assert_eq!(finding_recurrence(&["recurrence:abc".to_string()]), 1);
    }

    #[test]
    fn parse_promote_threshold_finds_explicit_value() {
        let toml = "[findings]\npromote_threshold = 5\n";
        assert_eq!(parse_promote_threshold(toml), Some(5));
    }

    #[test]
    fn parse_promote_threshold_absent_returns_none() {
        let toml = "[findings]\nother_key = 42\n";
        assert_eq!(parse_promote_threshold(toml), None);
        // Wrong section is also a miss.
        let toml = "[telemetry]\npromote_threshold = 5\n";
        assert_eq!(parse_promote_threshold(toml), None);
    }

    #[test]
    fn parse_promote_threshold_rejects_zero_and_garbage() {
        // 0 isn't a meaningful threshold — fall back to default.
        let toml = "[findings]\npromote_threshold = 0\n";
        assert_eq!(parse_promote_threshold(toml), None);
        let toml = "[findings]\npromote_threshold = abc\n";
        assert_eq!(parse_promote_threshold(toml), None);
    }

    #[test]
    fn parse_promote_threshold_ignores_comments_and_blank_lines() {
        let toml = "# comment\n\n[findings]\n# threshold comment\npromote_threshold = 7\n";
        assert_eq!(parse_promote_threshold(toml), Some(7));
    }

    #[test]
    fn build_findings_view_groups_advisor_by_origin() {
        let summaries = vec![
            summary(
                "TASK-100",
                "advisor obs on STORY-467",
                &[
                    "from-advisor:STORY-467",
                    "kind:observation",
                    "severity:minor",
                ],
            ),
            summary(
                "TASK-101",
                "advisor obs on STORY-467 major",
                &[
                    "from-advisor:STORY-467",
                    "kind:observation",
                    "severity:major",
                ],
            ),
            summary(
                "TASK-102",
                "advisor general",
                &["from-advisor:general", "kind:observation"],
            ),
        ];
        let view = build_findings_view(&summaries, &FindingsFilter::default());
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].source, FindingSource::Advisor);
        // Two origin groups: STORY-467 (first to appear in modified_at-DESC
        // ordering) and `general`.
        assert_eq!(view[0].groups.len(), 2);
        assert_eq!(view[0].groups[0].origin, "STORY-467");
        // Within the STORY-467 group, major sorts before minor.
        let ids: Vec<&str> = view[0].groups[0]
            .rows
            .iter()
            .map(|r| r.display_id.as_str())
            .collect();
        assert_eq!(ids, vec!["TASK-101", "TASK-100"]);
    }

    #[test]
    fn build_findings_view_orders_review_implementer_advisor() {
        let summaries = vec![
            summary("TASK-1", "advisor", &["from-advisor:STORY-9"]),
            summary("TASK-2", "review", &["from-review:PR-7"]),
            summary("TASK-3", "impl", &["from-implementer:STORY-9"]),
        ];
        let view = build_findings_view(&summaries, &FindingsFilter::default());
        assert_eq!(view.len(), 3);
        assert_eq!(view[0].source, FindingSource::Review);
        assert_eq!(view[1].source, FindingSource::Implementer);
        assert_eq!(view[2].source, FindingSource::Advisor);
    }

    #[test]
    fn build_findings_view_source_filter_isolates_advisor() {
        let summaries = vec![
            summary("TASK-1", "review", &["from-review:PR-7"]),
            summary(
                "TASK-2",
                "advisor",
                &["from-advisor:STORY-9", "kind:observation"],
            ),
        ];
        let view = build_findings_view(
            &summaries,
            &FindingsFilter {
                source: Some(FindingSource::Advisor),
                ..Default::default()
            },
        );
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].source, FindingSource::Advisor);
        assert_eq!(view[0].groups[0].rows[0].display_id, "TASK-2");
    }

    #[test]
    fn finding_row_carries_recurrence_count() {
        let summaries = vec![summary(
            "TASK-1",
            "recurring observation",
            &["from-advisor:STORY-9", "kind:observation", "recurrence:4"],
        )];
        let view = build_findings_view(&summaries, &FindingsFilter::default());
        assert_eq!(view[0].groups[0].rows[0].recurrence, 4);
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

    // BUG-641: the SPIKE-73 `find_finding` benchmark failed because the
    // finding's own id and the spec it is linked to rendered as two bare,
    // adjacent ids — a reader couldn't tell which was which. The render now
    // labels both: the origin header is `about <linked-spec>`, every row is
    // `finding <finding-id> …`. These assertions pin the labeled positions.
    #[test]
    fn origin_header_labels_the_linked_spec() {
        // The implementer was working STORY-9 when it filed the finding, so
        // STORY-9 is the *linked* spec the group is ABOUT — it must render
        // behind the `about` label, never as a bare id.
        let header = render_origin_header("STORY-9");
        assert_eq!(header, "about STORY-9");
        assert!(header.starts_with(ABOUT_LABEL));
        assert!(header.contains("STORY-9"));
    }

    #[test]
    fn finding_row_labels_finding_id_distinct_from_linked_spec() {
        // TASK-12 is the finding's OWN id; STORY-9 (above, in the origin
        // header) is the spec it is about. The row leads with `finding TASK-12`
        // so the finding-id and the linked-spec sit in clearly labeled,
        // non-confusable positions.
        let row = FindingRow {
            display_id: "TASK-12".to_string(),
            title: "null deref in parser".to_string(),
            severity: Severity::Major,
            kind: Some("bug-spotted".to_string()),
            recurrence: 1,
        };
        let rendered = render_finding_row(&row);
        // The finding-id is labeled and leads the row.
        assert!(rendered.starts_with(&format!("{FINDING_LABEL} TASK-12")));
        // The linked spec from the header is NOT in the row — they can't be
        // conflated.
        assert!(!rendered.contains("STORY-9"));
        assert!(rendered.contains("major"));
        assert!(rendered.contains("bug-spotted"));
        assert!(rendered.contains("null deref in parser"));
    }

    #[test]
    fn finding_row_without_kind_still_labels_finding_id() {
        // Review findings carry no `kind:` category; the label + leading
        // finding-id position must survive that branch too.
        let row = FindingRow {
            display_id: "TASK-1".to_string(),
            title: "unchecked unwrap".to_string(),
            severity: Severity::Minor,
            kind: None,
            recurrence: 1,
        };
        let rendered = render_finding_row(&row);
        assert!(rendered.starts_with(&format!("{FINDING_LABEL} TASK-1")));
        assert!(rendered.contains("minor"));
        assert!(rendered.contains("unchecked unwrap"));
    }

    #[test]
    fn finding_row_keeps_recurrence_suffix() {
        let row = FindingRow {
            display_id: "TASK-7".to_string(),
            title: "flaky test".to_string(),
            severity: Severity::Minor,
            kind: Some("followup-suggestion".to_string()),
            recurrence: 4,
        };
        let rendered = render_finding_row(&row);
        assert!(rendered.starts_with(&format!("{FINDING_LABEL} TASK-7")));
        assert!(rendered.contains("×4"));
    }
}
