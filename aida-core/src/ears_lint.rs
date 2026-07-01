//! Opt-in EARS-style quality linting for requirement text.
//!
//! TASK-0417: a heuristic, deterministic, LLM-free lint pass that helps AIDA
//! requirements adopt Kiro-style EARS (Easy Approach to Requirements Syntax)
//! clarity *without* forcing EARS as the canonical schema. AIDA remains a
//! **graph-first substrate** — stable IDs, typed relationships, trace comments;
//! EARS is offered here as an **optional quality lens** layered on top, never a
//! required shape.
//!
//! The linter scores a piece of requirement text (a description and/or its
//! acceptance criteria) against four heuristics drawn from EARS practice:
//!
//! - **vague-trigger** — the requirement names a stimulus/condition with a fuzzy
//!   word ("appropriate", "as needed", "etc.") instead of a concrete trigger.
//!   EARS templates are anchored on a precise `WHEN <trigger>` / `IF <condition>`.
//! - **missing-behavior** — no observable "the system shall <do X>" style
//!   expected response is present; the text states a wish or a noun phrase but
//!   never an action the system performs.
//! - **conflicting-constraint** — two clauses pull in opposite directions
//!   (`must` … `must not` on the same subject, "always" + "except", "all" +
//!   "none"), a classic source of untestable specs.
//! - **low-testability** — subjective / unmeasurable wording ("fast", "user
//!   friendly", "robust", "scalable") with no measurable criterion attached.
//!
//! Every finding carries a **suggested rewrite** — a draft, never an edit. The
//! caller (CLI / MCP) is responsible for *printing* suggestions or filing them
//! as comments; this module never mutates a spec and never touches a store.
//! The analysis is a pure function over a `&str`, so it is unit-testable and
//! identical under CLI and MCP.
//!
//! trace:TASK-0417 | ai:claude

use std::fmt;

/// The four heuristic categories an EARS lint finding can fall into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// The spec body is empty or so thin there is nothing to specify against —
    /// no description and no acceptance criteria. The worst-specified spec of
    /// all, and exactly what the lens exists to catch. trace:TASK-884
    EmptyBody,
    /// A fuzzy stimulus/condition word instead of a concrete EARS trigger.
    VagueTrigger,
    /// No observable "shall <do X>" expected behavior present.
    MissingBehavior,
    /// Two clauses contradict each other (must / must not, always / except).
    ConflictingConstraint,
    /// Subjective / unmeasurable wording with no measurable criterion.
    LowTestability,
}

impl Category {
    /// Stable machine token (used in `--json` output and tests).
    pub fn slug(&self) -> &'static str {
        match self {
            Category::EmptyBody => "empty-body",
            Category::VagueTrigger => "vague-trigger",
            Category::MissingBehavior => "missing-behavior",
            Category::ConflictingConstraint => "conflicting-constraint",
            Category::LowTestability => "low-testability",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.slug())
    }
}

/// A single lint finding against a piece of requirement text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Which heuristic fired.
    pub category: Category,
    /// Human-readable explanation of *what* tripped the heuristic.
    pub message: String,
    /// The offending fragment (word or short clause), when one can be named.
    pub evidence: Option<String>,
    /// A suggested rewrite — a draft only; the caller never auto-applies it.
    pub suggestion: String,
}

/// EARS-style lint findings for one spec's text, grouped by category.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LintReport {
    pub findings: Vec<Finding>,
}

impl LintReport {
    /// True when the text passed every heuristic.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// Count of findings in a given category.
    pub fn count(&self, category: Category) -> usize {
        self.findings
            .iter()
            .filter(|f| f.category == category)
            .count()
    }
}

// Fuzzy words that signal a vague trigger/condition rather than a concrete one.
// trace:TASK-0417
const VAGUE_TRIGGER_WORDS: &[&str] = &[
    "appropriate",
    "appropriately",
    "as needed",
    "as necessary",
    "as required",
    "if needed",
    "when needed",
    "where appropriate",
    "etc.",
    "and so on",
    "various",
    "certain",
    "relevant",
    "suitable",
    "reasonable",
    "properly",
    "correctly",
    "somehow",
];

// Subjective / unmeasurable adjectives that need a measurable criterion to be
// testable. trace:TASK-0417
const SUBJECTIVE_WORDS: &[&str] = &[
    "fast",
    "quickly",
    "slow",
    "performant",
    "efficient",
    "scalable",
    "robust",
    "reliable",
    "user friendly",
    "user-friendly",
    "intuitive",
    "easy to use",
    "seamless",
    "smooth",
    "modern",
    "nice",
    "optimal",
    "flexible",
    "lightweight",
    "minimal",
];

// Words that indicate an observable, testable system behavior is present.
// trace:TASK-0417
const BEHAVIOR_MARKERS: &[&str] = &[
    "shall",
    "must",
    "will ",
    "returns",
    "return ",
    "displays",
    "display ",
    "emits",
    "emit ",
    "writes",
    "write ",
    "rejects",
    "reject ",
    "accepts",
    "accept ",
    "validates",
    "validate ",
    "creates",
    "create ",
    "updates",
    "update ",
    "deletes",
    "delete ",
    "logs ",
    "exits",
    "exit ",
    "prints",
    "print ",
    "responds",
    "respond ",
    "blocks",
    "block ",
    "allows",
    "allow ",
    "stores",
    "store ",
    "sends",
    "send ",
];

// Units / digits that, when present near a subjective word, satisfy testability
// (a measurable criterion is attached). trace:TASK-0417
const MEASURABLE_MARKERS: &[&str] = &[
    "ms",
    "msec",
    "millisecond",
    "second",
    "sec ",
    "minute",
    "%",
    "percent",
    "byte",
    "request",
    "rps",
    "qps",
    "p95",
    "p99",
    "within ",
    "at most",
    "at least",
    "no more than",
    "fewer than",
    "less than ",
];

/// Lint a requirement's text against the EARS quality heuristics.
///
/// `text` is the combined material to evaluate — typically a description plus
/// its acceptance criteria. The result is a pure function of the input: no
/// store, no filesystem, no LLM, so the CLI and MCP surfaces score identically.
/// trace:TASK-0417 | ai:claude
pub fn lint_text(text: &str) -> LintReport {
    let mut findings = Vec::new();
    let lower = text.to_ascii_lowercase();

    // --- empty-body ----------------------------------------------------------
    // A spec with no body (or one too thin to specify against) is the worst-
    // specified spec there is — and prior to TASK-884 it slipped through with a
    // clean tick because every downstream heuristic is guarded on non-empty
    // text. Flag it first and short-circuit: a one-word body has nothing for the
    // other heuristics to chew on, so a single actionable finding is clearer
    // than a pile of spurious ones. "Near-empty" = fewer than 3 meaningful
    // tokens once markdown scaffolding (headings, list bullets) is stripped.
    // trace:TASK-884
    if is_essentially_empty(text) {
        findings.push(Finding {
            category: Category::EmptyBody,
            message: "spec body is empty or too thin to specify against — no description \
                      and no acceptance criteria for the lens to evaluate"
                .to_string(),
            evidence: None,
            suggestion: "Write at least a one-sentence description and an `## Acceptance` \
                         section with a concrete, testable criterion, e.g. \"WHEN <event> \
                         THE SYSTEM SHALL <observable response>\"."
                .to_string(),
        });
        return LintReport { findings };
    }

    // --- vague-trigger -------------------------------------------------------
    for word in VAGUE_TRIGGER_WORDS {
        if contains_word(&lower, word) {
            findings.push(Finding {
                category: Category::VagueTrigger,
                message: format!(
                    "vague trigger/condition wording \"{}\" — name a concrete stimulus",
                    word.trim()
                ),
                evidence: Some(word.trim().to_string()),
                suggestion: format!(
                    "Replace \"{}\" with a concrete EARS trigger, e.g. \
                     \"WHEN <specific event> THE SYSTEM SHALL <response>\".",
                    word.trim()
                ),
            });
            // One finding per distinct vague word is enough signal.
            break;
        }
    }

    // --- missing-behavior ----------------------------------------------------
    // Treat the text as having an observable behavior if any behavior marker
    // appears. EARS' core is a "<system> shall <action>" response clause.
    let has_behavior = BEHAVIOR_MARKERS.iter().any(|m| lower.contains(m));
    if !has_behavior && !lower.trim().is_empty() {
        findings.push(Finding {
            category: Category::MissingBehavior,
            message: "no observable expected behavior — the text never states what the \
                      system *does* (no \"shall/returns/rejects/...\" response clause)"
                .to_string(),
            evidence: None,
            suggestion: "Add an EARS response clause naming the observable behavior, e.g. \
                         \"THE SYSTEM SHALL <verb> <object>\" (returns/rejects/displays/...)."
                .to_string(),
        });
    }

    // --- conflicting-constraint ----------------------------------------------
    if let Some(evidence) = detect_conflict(&lower) {
        findings.push(Finding {
            category: Category::ConflictingConstraint,
            message: format!("conflicting constraints detected ({evidence})"),
            evidence: Some(evidence),
            suggestion: "Split into separate, non-overlapping clauses or qualify the \
                         exception precisely (\"WHEN <case> ... ; OTHERWISE ...\") so each \
                         clause is independently testable."
                .to_string(),
        });
    }

    // --- low-testability -----------------------------------------------------
    let measurable = has_measurable_criterion(&lower);
    for word in SUBJECTIVE_WORDS {
        if contains_word(&lower, word) && !measurable {
            findings.push(Finding {
                category: Category::LowTestability,
                message: format!(
                    "subjective/unmeasurable wording \"{word}\" with no measurable criterion"
                ),
                evidence: Some(word.to_string()),
                suggestion: format!(
                    "Attach a measurable criterion to \"{word}\", e.g. a latency budget \
                     (\"p95 < 200ms\"), a size bound, or a pass/fail threshold."
                ),
            });
            break;
        }
    }

    LintReport { findings }
}

/// True when a spec body carries essentially no specifiable content: empty,
/// whitespace-only, or fewer than 3 meaningful word-tokens once markdown
/// scaffolding (heading hashes, list bullets/numbers, blockquote markers) is
/// stripped. A bare `## Acceptance` heading with nothing under it, or a stub
/// like "fix the thing", lands here. trace:TASK-884
fn is_essentially_empty(text: &str) -> bool {
    let mut tokens = 0usize;
    for raw_line in text.lines() {
        // Strip leading markdown scaffolding so a heading or empty bullet does
        // not count as content.
        let line = raw_line
            .trim()
            .trim_start_matches('#')
            .trim_start_matches('>')
            .trim_start_matches(|c: char| c == '-' || c == '*' || c == '+')
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')')
            .trim();
        for word in line.split_whitespace() {
            // A "word" must contain at least one alphanumeric char to count —
            // bare punctuation / checkbox brackets (`[ ]`) do not.
            if word.chars().any(|c| c.is_alphanumeric()) {
                tokens += 1;
            }
        }
    }
    tokens < 3
}

/// Detect a pair of clauses that contradict each other. Heuristic and
/// conservative — only fires on well-known contradiction shapes to keep false
/// positives low. trace:TASK-0417
fn detect_conflict(lower: &str) -> Option<String> {
    // must X ... must not X (a positive "must" obligation alongside a "must not").
    if lower.contains("must not") {
        let must_positive = lower
            .match_indices("must ")
            .any(|(idx, _)| !lower[idx..].starts_with("must not"));
        if must_positive {
            return Some("\"must\" and \"must not\" both present".to_string());
        }
    }
    // "always ... except" / "always ... but not".
    if lower.contains("always") && (lower.contains("except") || lower.contains("but not")) {
        return Some("\"always\" qualified by an exception".to_string());
    }
    // "all ... none" — a universal ("all X") contradicted by "none" applied to
    // the SAME subject. Two guards keep this from false-positing on specs where
    // "all" and "none" describe DIFFERENT, non-overlapping clauses:
    //   1. Whole-word matching, so the "all" inside "shall"/"call"/"install"
    //      never triggers ("each glyph SHALL be registered" + "none appear as
    //      raw unicode" must NOT flag).
    //   2. Same-clause co-occurrence: both words must land in ONE clause
    //      (sentence / bullet / line), a conservative proxy for "same subject".
    // A genuine self-contradiction ("accepts all inputs but none are accepted")
    // keeps both words in one clause and still fires.
    // trace:BUG-686
    for clause in lower.split(['.', ';', '\n', '\r']) {
        if contains_word(clause, "all") && contains_word(clause, "none") {
            return Some("\"all\" and \"none\" both present".to_string());
        }
    }
    None
}

/// True when a measurable marker (a unit, digit, or bound phrase) is present,
/// satisfying testability for an otherwise-subjective word. trace:TASK-0417
fn has_measurable_criterion(lower: &str) -> bool {
    if lower.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    MEASURABLE_MARKERS.iter().any(|m| lower.contains(m))
}

/// Word-ish containment: matches `needle` as a substring, but for short
/// alphabetic needles requires a word boundary on each side so "some" doesn't
/// match "something" and "secure" doesn't match "insecurely". For needles that
/// already contain spaces/punctuation we fall back to plain `contains`.
/// trace:TASK-0417
fn contains_word(haystack: &str, needle: &str) -> bool {
    let needle = needle.trim();
    if needle.is_empty() {
        return false;
    }
    // Multi-word or punctuation-bearing needles: plain substring is fine.
    if needle.contains(' ') || needle.chars().any(|c| !c.is_ascii_alphabetic()) {
        return haystack.contains(needle);
    }
    let bytes = haystack.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = haystack[search_from..].find(needle) {
        let start = search_from + rel;
        let end = start + needle.len();
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        search_from = start + 1;
        if search_from >= haystack.len() {
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_ears_text_passes() {
        let text = "WHEN the user submits an empty form THE SYSTEM SHALL reject the \
                    submission and display a validation error within 200ms.";
        let report = lint_text(text);
        assert!(
            report.is_clean(),
            "well-formed EARS text should be clean, got {:?}",
            report.findings
        );
    }

    #[test]
    fn detects_vague_trigger() {
        let report =
            lint_text("The system shall validate input as appropriate and return an error code.");
        assert_eq!(report.count(Category::VagueTrigger), 1);
        let f = report
            .findings
            .iter()
            .find(|f| f.category == Category::VagueTrigger)
            .unwrap();
        assert!(f.suggestion.to_lowercase().contains("when"));
    }

    #[test]
    fn detects_missing_behavior() {
        // A noun-phrase wish with no observable response clause.
        let report = lint_text("Great support for large projects and many users.");
        assert_eq!(report.count(Category::MissingBehavior), 1);
    }

    #[test]
    fn behavior_marker_suppresses_missing_behavior() {
        let report = lint_text("The system shall return a 404 when the id is unknown.");
        assert_eq!(report.count(Category::MissingBehavior), 0);
    }

    #[test]
    fn detects_conflicting_constraint() {
        let report =
            lint_text("The endpoint must return cached data and must not query the cache.");
        assert_eq!(report.count(Category::ConflictingConstraint), 1);
    }

    // The "all"/"none" conflict heuristic must not fire when the two words
    // describe DIFFERENT, non-overlapping clauses. The "all" inside "SHALL" used
    // to trip the old substring check. trace:BUG-686
    #[test]
    fn each_none_in_separate_clauses_is_not_flagged() {
        let report = lint_text(
            "Each glyph SHALL be registered in the glyph table.\n\
             None of the glyphs appear as raw unicode in the source.",
        );
        assert_eq!(
            report.count(Category::ConflictingConstraint),
            0,
            "\"each ... SHALL\" + \"none ... raw\" describe different subjects and \
             must not be flagged as a conflict, got {:?}",
            report.findings
        );
    }

    // A genuine same-clause "all X ... none X" contradiction must STILL fire.
    // trace:BUG-686
    #[test]
    fn all_and_none_same_clause_still_flags() {
        let report =
            lint_text("The system SHALL accept all inputs but none of the inputs are accepted.");
        assert_eq!(
            report.count(Category::ConflictingConstraint),
            1,
            "a same-clause all/none contradiction must still be flagged, got {:?}",
            report.findings
        );
    }

    #[test]
    fn detects_low_testability_without_measure() {
        let report = lint_text("The dashboard shall be fast and user friendly.");
        assert!(report.count(Category::LowTestability) >= 1);
    }

    #[test]
    fn measurable_criterion_suppresses_low_testability() {
        let report = lint_text("The dashboard shall load fast, with p95 latency under 200ms.");
        assert_eq!(report.count(Category::LowTestability), 0);
    }

    #[test]
    fn word_boundary_avoids_substring_false_positive() {
        // "various" must not match inside "variousness"-style words; a clean
        // word should still match.
        assert!(!contains_word("do somethingvarioustoken", "various"));
        assert!(contains_word("fix various bugs", "various"));
    }

    // --- empty-body (TASK-884) ----------------------------------------------
    // Pre-TASK-884 an empty body got a clean tick — the worst-specified spec
    // passing the quality lens. It must now be flagged.

    #[test]
    fn whitespace_only_text_is_flagged_empty_body() {
        let report = lint_text("   ");
        assert!(!report.is_clean(), "an empty body must not pass clean");
        assert_eq!(report.count(Category::EmptyBody), 1);
    }

    #[test]
    fn empty_string_is_flagged_empty_body() {
        let report = lint_text("");
        assert_eq!(report.count(Category::EmptyBody), 1);
    }

    #[test]
    fn near_empty_stub_is_flagged_empty_body() {
        // "fix the thing with the search" has content; a true stub does not.
        let report = lint_text("fix it");
        assert_eq!(report.count(Category::EmptyBody), 1);
    }

    #[test]
    fn bare_acceptance_heading_is_flagged_empty_body() {
        // Markdown scaffolding with no real content under it.
        let report = lint_text("## Acceptance\n\n- [ ]");
        assert_eq!(report.count(Category::EmptyBody), 1);
    }

    #[test]
    fn empty_body_short_circuits_other_heuristics() {
        // A flagged empty body emits exactly one finding (no spurious pile).
        let report = lint_text("   ");
        assert_eq!(report.findings.len(), 1);
    }

    #[test]
    fn substantive_body_is_not_flagged_empty_body() {
        let text = "WHEN the user submits an empty form THE SYSTEM SHALL reject the \
                    submission and display a validation error within 200ms.";
        let report = lint_text(text);
        assert_eq!(report.count(Category::EmptyBody), 0);
    }
}
