//! STORY-1642: unit coverage for the knowledge-list helpers.
// trace:STORY-1642 | ai:claude

use super::*;

fn summary(id: &str, req_type: &str, status: &str) -> aida_core::RequirementSummary {
    aida_core::RequirementSummary {
        id: uuid::Uuid::new_v4(),
        spec_id: Some(id.to_string()),
        agreed_id: None,
        title: format!("{id} title"),
        description: String::new(),
        status: status.to_string(),
        priority: "Medium".to_string(),
        owner: String::new(),
        assignee: None,
        feature: String::new(),
        req_type: req_type.to_string(),
        tags: Vec::new(),
        created_at: String::new(),
        modified_at: String::new(),
        archived: false,
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

fn mixed_rows() -> Vec<aida_core::RequirementSummary> {
    vec![
        summary("TASK-1", "Task", "Approved"),
        summary("DOC-1", "Doc", "Approved"),
        summary("FAQ-1", "Faq", "Completed"),
        summary("ADR-1", "Decision", "Draft"),
        summary("VIS-1", "Vision", "InProgress"),
        summary("TERM-1", "Term", "Approved"),
        summary("PRIN-1", "Principle", "Approved"),
    ]
}

fn ids(rows: &[aida_core::RequirementSummary]) -> Vec<&str> {
    rows.iter().filter_map(|r| r.spec_id.as_deref()).collect()
}

// Item 2: the default list drops every knowledge row (and counts them), keeps
// work rows, and keeps Doc (item 6).
#[test]
fn default_list_hides_knowledge_and_counts_them() {
    let mut rows = mixed_rows();
    let hidden = hide_knowledge_rows(&mut rows, false, None);
    assert_eq!(hidden, 5);
    assert_eq!(ids(&rows), vec!["TASK-1", "DOC-1"]);
}

// Item 2: `--include-knowledge` and a knowledge `--type` (which is also what
// `aida <type> list` rewrites to) both keep the rows.
#[test]
fn knowledge_shown_when_asked_for() {
    let mut rows = mixed_rows();
    assert_eq!(hide_knowledge_rows(&mut rows, true, None), 0);
    assert_eq!(rows.len(), 7);
    for t in ["faq", "FAQ", "decision", "vision", "term", "principle"] {
        let mut rows = mixed_rows();
        assert_eq!(hide_knowledge_rows(&mut rows, false, Some(t)), 0, "{t}");
    }
    // A work `--type` does not opt into knowledge rows.
    let mut rows = mixed_rows();
    assert_eq!(hide_knowledge_rows(&mut rows, false, Some("task")), 5);
}

// Item 3: knowledge-typed listings bypass the open lens; work types do not.
#[test]
fn knowledge_type_filter_detection() {
    assert!(type_filter_is_knowledge(Some("faq")));
    assert!(type_filter_is_knowledge(Some("Decision")));
    assert!(!type_filter_is_knowledge(Some("doc")));
    assert!(!type_filter_is_knowledge(Some("task")));
    assert!(!type_filter_is_knowledge(None));
    // A comma list is a knowledge listing when any listed type is knowledge.
    assert!(type_filter_is_knowledge(Some("faq,decision")));
    assert!(type_filter_is_knowledge(Some("task, faq")));
    assert!(!type_filter_is_knowledge(Some("task,bug")));
    let mut rows = mixed_rows();
    assert_eq!(
        hide_knowledge_rows(&mut rows, false, Some("faq,decision")),
        0
    );
}

// Item 7: a type word in the positional reads as `--type`, and no type word
// collides with a status, alias, lens or user word.
#[test]
fn positional_type_words() {
    assert_eq!(positional_type_word(Some("faq")).as_deref(), Some("faq"));
    assert_eq!(positional_type_word(Some("FAQ")).as_deref(), Some("faq"));
    assert_eq!(positional_type_word(Some("bug")).as_deref(), Some("bug"));
    for not_type in [
        "draft",
        "approved",
        "planned",
        "in-progress",
        "needs-attention",
        "done",
        "completed",
        "rejected",
        "superseded",
        "open",
        "closed",
        "shelved",
        "needs-decision",
        "deferred",
        "human",
        "queue",
        "advisor",
        "why",
        "inflight",
        "me",
        "user:joe",
    ] {
        assert_eq!(positional_type_word(Some(not_type)), None, "{not_type}");
    }
    assert_eq!(positional_type_word(None), None);
}

// Item 7: every built-in type has its word in the shared table.
#[test]
fn every_builtin_type_has_a_type_word() {
    for t in aida_core::models::RequirementType::ALL.iter() {
        let word = match t {
            aida_core::models::RequirementType::NonFunctional => "non-functional".to_string(),
            aida_core::models::RequirementType::ChangeRequest => "change-request".to_string(),
            other => format!("{other:?}").to_ascii_lowercase(),
        };
        assert!(
            BUILTIN_TYPE_WORDS.contains(&word.as_str()),
            "{word} missing from BUILTIN_TYPE_WORDS"
        );
    }
}

// Item 9: superseded FAQs are hidden from the FAQ listing unless `--all`.
#[test]
fn superseded_faqs_hidden_unless_all() {
    let rows = || {
        vec![
            summary("FAQ-1", "Faq", "Completed"),
            summary("FAQ-2", "Faq", "Superseded"),
            summary("FAQ-3", "Faq", "Draft"),
        ]
    };
    let mut r = rows();
    assert_eq!(hide_superseded_faqs(&mut r, Some("faq"), false), 1);
    assert_eq!(ids(&r), vec!["FAQ-1", "FAQ-3"]);
    let mut r = rows();
    assert_eq!(hide_superseded_faqs(&mut r, Some("faq"), true), 0);
    // A superseded decision keeps showing: its status means something.
    let mut r = vec![summary("ADR-1", "Decision", "Superseded")];
    assert_eq!(hide_superseded_faqs(&mut r, Some("decision"), false), 0);
}

// Item 9: the FAQ table is ID + question; explicit `--fields` wins; other
// knowledge types keep their Status column.
#[test]
fn faq_table_fields() {
    assert_eq!(faq_default_fields(Some("faq"), None), Some("id,title"));
    assert_eq!(faq_default_fields(Some("faq"), Some("id,status")), None);
    assert_eq!(faq_default_fields(Some("decision"), None), None);
    assert_eq!(faq_default_fields(None, None), None);
}

// Item 9: the ID + question projection renders no Type/Status/Priority header.
#[test]
fn faq_table_renders_id_and_question_only() {
    let selected =
        crate::output_format::toon_list_fields(faq_default_fields(Some("faq"), None)).unwrap();
    assert_eq!(selected, vec!["id", "title"]);
}

// Item 2: the footer says how many knowledge rows were hidden and how to show
// them; silent when none were.
#[test]
fn knowledge_footer_lines() {
    assert_eq!(knowledge_hidden_hint_line(0), None);
    let line = knowledge_hidden_hint_line(3).unwrap();
    assert!(line.contains("3 knowledge rows hidden"), "{line}");
    assert!(line.contains("--include-knowledge"), "{line}");
    assert!(line.contains("aida faq list"), "{line}");
    assert!(line.contains("faq"), "{line}");
    assert!(knowledge_hidden_hint_line(1)
        .unwrap()
        .contains("1 knowledge row hidden"));
    assert_eq!(knowledge_hidden_agent_note(0), None);
    assert!(knowledge_hidden_agent_note(2)
        .unwrap()
        .contains("--include-knowledge"));
    let one = knowledge_hidden_agent_note(1).unwrap();
    assert!(one.contains("1 knowledge row hidden"), "{one}");
    assert!(knowledge_hidden_agent_note(2)
        .unwrap()
        .contains("2 knowledge rows hidden"));
}
