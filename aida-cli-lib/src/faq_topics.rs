//! STORY-1645: `aida faq topics` — browse FAQs by topic.
//!
//! Lists every tag in use on a live FAQ with how many live FAQs carry it, so a
//! reader can pick a topic and then run `aida faq list --tags <topic>`.
//!
//! "Live" is the same rule the FAQ listing uses
//! ([`crate::knowledge_list::hide_superseded_faqs`] plus the default archive
//! axis): a FAQ that is neither superseded nor archived. Counts are per tag;
//! a FAQ carrying a tag twice still counts once.
//!
//! trace:STORY-1645 | ai:claude

use anyhow::Result;
use serde::Serialize;

/// One topic row: a tag and how many live FAQs carry it.
// trace:STORY-1645 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FaqTopic {
    pub(crate) tag: String,
    pub(crate) count: usize,
}

/// The one-line notice printed when no live FAQ carries a tag.
// trace:STORY-1645 | ai:claude
pub(crate) const NO_FAQ_TOPICS: &str = "no FAQ topics";

/// Is this row a live FAQ — FAQ-typed, not archived, not superseded?
// trace:STORY-1645 | ai:claude
pub(crate) fn is_live_faq(r: &aida_core::RequirementSummary) -> bool {
    r.req_type.eq_ignore_ascii_case("faq")
        && !r.archived
        && !r.status.eq_ignore_ascii_case("superseded")
}

/// Count the tags on live FAQs. Sorted by count descending, then tag name
/// ascending. Tags with no live FAQ never appear.
// trace:STORY-1645 | ai:claude
pub(crate) fn faq_topic_counts(reqs: &[aida_core::RequirementSummary]) -> Vec<FaqTopic> {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for r in reqs.iter().filter(|r| is_live_faq(r)) {
        let distinct: std::collections::BTreeSet<&str> = r
            .tags
            .iter()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .collect();
        for tag in distinct {
            *counts.entry(tag.to_string()).or_insert(0) += 1;
        }
    }
    let mut topics: Vec<FaqTopic> = counts
        .into_iter()
        .map(|(tag, count)| FaqTopic { tag, count })
        .collect();
    topics.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
    topics
}

/// Agent (TOON) render: a compact `topics[N]{tag,count}` table with no spec
/// ids, or the one-line empty notice.
// trace:STORY-1645 | ai:claude
pub(crate) fn render_faq_topics_agent(topics: &[FaqTopic]) -> String {
    if topics.is_empty() {
        return NO_FAQ_TOPICS.to_string();
    }
    let rows: Vec<Vec<String>> = topics
        .iter()
        .map(|t| vec![t.tag.clone(), t.count.to_string()])
        .collect();
    crate::toon::table_raw("topics", &["tag", "count"], &rows)
}

/// Human render: an aligned topic / count table plus a hint at the next step,
/// or the one-line empty notice.
// trace:STORY-1645 | ai:claude
pub(crate) fn render_faq_topics_human(topics: &[FaqTopic]) -> String {
    if topics.is_empty() {
        return NO_FAQ_TOPICS.to_string();
    }
    let width = topics
        .iter()
        .map(|t| t.tag.chars().count())
        .max()
        .unwrap_or(0)
        .max("TOPIC".len());
    let mut out = format!("{:<width$}  FAQS", "TOPIC");
    for t in topics {
        out.push('\n');
        out.push_str(&format!("{:<width$}  {}", t.tag, t.count));
    }
    out.push_str("\n\nBrowse a topic: aida faq list --tags <topic>");
    out
}

/// `aida faq topics` (== `aida list --type faq --topics`): read the live FAQs
/// and print their topics in the caller's output mode.
// trace:STORY-1645 | ai:claude
pub(crate) fn run_faq_topics(backend: &aida_core::CachedGitBackend, json: bool) -> Result<()> {
    // Its own filter: FAQs only, the default (non-archived, non-deferred)
    // axes, and no status lens, role scope or focus — topics describe the
    // whole live FAQ set.
    let filter = aida_core::ListFilter {
        req_type: Some("faq".to_string()),
        ..Default::default()
    };
    let reqs = backend.list_summaries(&filter)?;
    let topics = faq_topic_counts(&reqs);
    if json {
        println!("{}", crate::cache_output::json_pretty(&topics)?);
    } else if crate::output_format::agent_output_mode() {
        println!("{}", render_faq_topics_agent(&topics));
    } else {
        println!("{}", render_faq_topics_human(&topics));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_1645_faq_topics_tests.rs"]
mod story_1645_faq_topics_tests;
