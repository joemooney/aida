use super::*;
use aida_core::models::Comment;

fn comment(content: &str) -> Comment {
    Comment::new("codex".to_string(), content.to_string())
}

#[test]
fn review_prompt_extracts_newest_stable_marked_implementer_approach() {
    let comments = vec![
        comment("## Implementer approach\n\n### Expected files\n- old.rs"),
        comment("I mentioned the implementer approach, but this is not the marker."),
        comment(
            "## Implementer approach\n\n### Acceptance criteria as tests\n- **Unit:** regression\n\n### Expected files\n- new.rs\n\n### Ambiguities / conflicts as questions\n- None",
        ),
    ];

    let approach = extract_implementer_approach(&comments).expect("marked approach");
    assert!(approach.contains("**Unit:** regression"));
    assert!(approach.contains("new.rs"));
    assert!(!approach.contains("old.rs"));

    let mut prompt = String::new();
    append_implementer_approach(&mut prompt, &comments);
    assert!(prompt.contains("#### Recorded implementer approach"));
    assert!(prompt.contains("## Implementer approach"));
    assert!(prompt.contains("**Unit:** regression"));
    assert!(prompt.contains("new.rs"));
}

#[test]
fn review_prompt_ignores_unmarked_approach_discussion() {
    let comments = vec![comment(
        "Reviewer note: the implementer approach should have covered the caller.",
    )];

    assert_eq!(extract_implementer_approach(&comments), None);
}
