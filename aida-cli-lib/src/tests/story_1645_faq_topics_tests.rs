//! STORY-1645: unit coverage for the `aida faq topics` helpers.
// trace:STORY-1645 | ai:claude

use super::*;

fn faq(id: &str, status: &str, archived: bool, tags: &[&str]) -> aida_core::RequirementSummary {
    row(id, "Faq", status, archived, tags)
}

fn row(
    id: &str,
    req_type: &str,
    status: &str,
    archived: bool,
    tags: &[&str],
) -> aida_core::RequirementSummary {
    aida_core::RequirementSummary {
        id: uuid::Uuid::new_v4(),
        spec_id: Some(id.to_string()),
        agreed_id: None,
        title: format!("{id} question?"),
        description: String::new(),
        status: status.to_string(),
        priority: "Medium".to_string(),
        owner: String::new(),
        assignee: None,
        feature: String::new(),
        req_type: req_type.to_string(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        created_at: String::new(),
        modified_at: String::new(),
        archived,
        archived_at: None,
        deferred: false,
        deferred_at: None,
        deferred_until: None,
        deferred_reason: None,
        in_degree: 0,
        out_degree: 0,
        heft: 0,
        blocked: false,
        has_pending_decision: false,
        execution_mode: None,
        weight: None,
        origin: None,
        completed_at: None,
        yaml_path: String::new(),
    }
}

fn topic(tag: &str, count: usize) -> FaqTopic {
    FaqTopic {
        tag: tag.to_string(),
        count,
    }
}

// Acceptance 1 + 3: three FAQs, one superseded and one archived — only the
// live FAQ's tags are counted; tags carried only by the superseded or
// archived FAQ (or by a non-FAQ row) do not appear.
#[test]
fn faq_topics_counts_live_faqs_only() {
    let rows = vec![
        faq("FAQ-1", "Completed", false, &["setup", "shared"]),
        faq("FAQ-2", "Superseded", false, &["old-way", "shared"]),
        faq("FAQ-3", "Completed", true, &["archived-topic", "shared"]),
        row("TASK-1", "Task", "Approved", false, &["work-tag"]),
    ];
    assert_eq!(
        faq_topic_counts(&rows),
        vec![topic("setup", 1), topic("shared", 1)]
    );
}

// Acceptance 1: sorted by count descending, then tag name ascending; a tag
// repeated on one FAQ counts once.
#[test]
fn faq_topics_sort_count_desc_then_name() {
    let rows = vec![
        faq("FAQ-1", "Completed", false, &["beta", "alpha", "zeta"]),
        faq("FAQ-2", "Completed", false, &["zeta", "beta", "zeta"]),
        faq("FAQ-3", "Approved", false, &["zeta", " gamma "]),
    ];
    assert_eq!(
        faq_topic_counts(&rows),
        vec![
            topic("zeta", 3),
            topic("beta", 2),
            topic("alpha", 1),
            topic("gamma", 1),
        ]
    );
}

// Acceptance 2: the agent render is a compact table with no spec ids.
#[test]
fn faq_topics_agent_output_has_no_ids() {
    let rows = vec![
        faq("FAQ-1", "Completed", false, &["setup", "cli"]),
        faq("FAQ-2", "Completed", false, &["cli"]),
    ];
    let out = render_faq_topics_agent(&faq_topic_counts(&rows));
    assert_eq!(out, "topics[2]{tag,count}:\n  cli,2\n  setup,1");
    assert!(!out.contains("FAQ-"), "{out}");
    let human = render_faq_topics_human(&faq_topic_counts(&rows));
    assert!(!human.contains("FAQ-"), "{human}");
    assert!(
        human.lines().any(|l| l.split_whitespace().eq(["cli", "2"])),
        "{human}"
    );
}

// Acceptance 3: no live FAQ topics prints the one-line notice in both modes.
#[test]
fn faq_topics_empty_notice() {
    let none: Vec<FaqTopic> = faq_topic_counts(&[
        faq("FAQ-1", "Completed", false, &[]),
        faq("FAQ-2", "Superseded", false, &["gone"]),
    ]);
    assert!(none.is_empty());
    assert_eq!(render_faq_topics_agent(&none), "no FAQ topics");
    assert_eq!(render_faq_topics_human(&none), "no FAQ topics");
}

// `aida faq topics` is the type-word spelling of `aida list --type faq --topics`.
#[test]
fn faq_topics_type_word_rewrite() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
    assert_eq!(
        crate::rewrite_type_alias(&s(&["aida", "faq", "topics"])),
        s(&["aida", "list", "--type", "faq", "--topics"]),
    );
    assert_eq!(
        crate::rewrite_type_alias(&s(&["aida", "FAQ", "topics", "--format", "json"])),
        s(&["aida", "list", "--type", "faq", "--topics", "--format", "json"]),
    );
    // Only FAQs have topics; other type words keep their old behavior.
    let bug = s(&["aida", "bug", "topics"]);
    assert_eq!(crate::rewrite_type_alias(&bug), bug);
}
