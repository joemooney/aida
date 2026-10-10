//! STORY-1642: knowledge types (FAQ, decision, vision, term, principle) are
//! reference material, not work.
//!
//! The rule for "what is knowledge" lives in ONE place,
//! [`aida_core::lifecycle::is_work_item_type`]. This module holds the pure
//! `aida list` helpers that apply it:
//!
//! * the default list hides knowledge rows (like META) unless the caller asks
//!   for them with `--type <knowledge-type>` / `aida <type> list` /
//!   `--include-knowledge`, and the footer says how many were hidden;
//! * a knowledge-typed listing shows every status (FAQs live as `completed`, so
//!   the open lens would hide them all);
//! * `aida list faq` reads the type word as `--type faq`;
//! * the FAQ table is just `ID` + question, and superseded FAQs are hidden
//!   unless `--all`.
//!
//! trace:STORY-1642 | ai:claude

/// Every built-in type word `aida <type> list` and `aida list <type>` accept.
/// One table for both spellings so they cannot drift.
// trace:STORY-1642 | ai:claude
pub(crate) const BUILTIN_TYPE_WORDS: &[&str] = &[
    "functional",
    "non-functional",
    "system",
    "user",
    "change-request",
    "bug",
    "epic",
    "story",
    "task",
    "spike",
    "sprint",
    "folder",
    "meta",
    "principle",
    "vision",
    "constraint",
    "decision",
    "term",
    "doc",
    "faq",
    "cr",
    "fr",
];

/// The `aida list` positional, read as a built-in type word: `aida list faq`
/// == `aida list --type faq`. Returns the lowercase type word, or `None` when
/// the token is not a type word (it is then a status / lens / user token).
///
/// No type word collides with a status, alias or lens word, so this can run
/// before the status expansion without shadowing anything.
// trace:STORY-1642 | ai:claude
pub(crate) fn positional_type_word(shortcut: Option<&str>) -> Option<String> {
    let token = shortcut?.trim().to_ascii_lowercase();
    BUILTIN_TYPE_WORDS
        .contains(&token.as_str())
        .then_some(token)
}

/// Does the `--type` filter name a knowledge-class type?
// trace:STORY-1642 | ai:claude
pub(crate) fn type_filter_is_knowledge(type_filter: Option<&str>) -> bool {
    // A comma list (`--type faq,decision`) is a knowledge listing when any
    // listed type is knowledge — hiding the very rows asked for would be wrong.
    type_filter.is_some_and(|t| {
        t.split(',')
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .any(aida_core::lifecycle::is_knowledge_type_str)
    })
}

/// Does the `--type` filter name the FAQ type?
// trace:STORY-1642 | ai:claude
pub(crate) fn type_filter_is_faq(type_filter: Option<&str>) -> bool {
    type_filter.is_some_and(|t| t.trim().eq_ignore_ascii_case("faq"))
}

/// Default `aida list` hides knowledge rows, the way META is hidden: shown
/// only for `--include-knowledge` or a knowledge `--type`. Returns how many
/// rows were removed so the footer can say so.
// trace:STORY-1642 | ai:claude
pub(crate) fn hide_knowledge_rows(
    reqs: &mut Vec<aida_core::RequirementSummary>,
    include_knowledge: bool,
    type_filter: Option<&str>,
) -> usize {
    if include_knowledge || type_filter_is_knowledge(type_filter) {
        return 0;
    }
    let before = reqs.len();
    reqs.retain(|r| !aida_core::lifecycle::is_knowledge_type_str(&r.req_type));
    before - reqs.len()
}

/// The FAQ listing hides superseded FAQs unless `--all` (archived ones are
/// already off the default archive axis). Returns the number removed.
// trace:STORY-1642 | ai:claude
pub(crate) fn hide_superseded_faqs(
    reqs: &mut Vec<aida_core::RequirementSummary>,
    type_filter: Option<&str>,
    all: bool,
) -> usize {
    if all || !type_filter_is_faq(type_filter) {
        return 0;
    }
    let before = reqs.len();
    reqs.retain(|r| {
        !(r.req_type.eq_ignore_ascii_case("faq") && r.status.eq_ignore_ascii_case("superseded"))
    });
    before - reqs.len()
}

/// The FAQ table drops Type, Status and Priority (constant for FAQs): `ID` and
/// the question only. An explicit `--fields` always wins.
// trace:STORY-1642 | ai:claude
pub(crate) fn faq_default_fields(
    type_filter: Option<&str>,
    explicit_fields: Option<&str>,
) -> Option<&'static str> {
    (explicit_fields.is_none() && type_filter_is_faq(type_filter)).then_some("id,title")
}

/// Human footer line for hidden knowledge rows, or `None` when none were.
// trace:STORY-1642 | ai:claude
pub(crate) fn knowledge_hidden_hint_line(hidden: usize) -> Option<String> {
    (hidden > 0).then(|| {
        format!(
            "  ({hidden} knowledge row{} hidden ({}) — pass --include-knowledge or `aida <type> list`, e.g. `aida faq list`)",
            if hidden == 1 { "" } else { "s" },
            aida_core::lifecycle::knowledge_type_names().join(", ")
        )
    })
}

/// Agent-mode (TOON) note for hidden knowledge rows.
// trace:STORY-1642 | ai:claude
pub(crate) fn knowledge_hidden_agent_note(hidden: usize) -> Option<String> {
    (hidden > 0).then(|| {
        format!(
            "note: {hidden} knowledge row{} hidden ({}) — `aida list --include-knowledge` or `aida faq list`",
            if hidden == 1 { "" } else { "s" },
            aida_core::lifecycle::knowledge_type_names().join(", ")
        )
    })
}

#[cfg(test)]
#[path = "tests/story_1642_knowledge_list_tests.rs"]
mod story_1642_knowledge_list_tests;
