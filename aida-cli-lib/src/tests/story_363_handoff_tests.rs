use super::*;

// trace:STORY-363 | ai:claude
#[test]
fn render_advisor_handoff_has_frontmatter_and_five_sections() {
    let body = render_advisor_handoff(
        "aida",
        std::path::Path::new("/home/joe/ai/aida"),
        "git-canonical store",
        "2026-06-06",
        "claude",
    );

    // Frontmatter carries the auto-filled identity + focus.
    assert!(body.starts_with("---\n"), "must open with YAML frontmatter");
    assert!(body.contains("kind: advisor-handoff\n"));
    assert!(body.contains("parent: aida\n"));
    assert!(body.contains("focus: 'git-canonical store'\n"));
    assert!(body.contains("date: 2026-06-06\n"));
    assert!(body.contains("authored_by: claude\n"));
    assert!(body.contains("status: draft\n"));

    // All five named sections are present, in order.
    let s1 = body.find("## 1. Parent identity (auto)").unwrap();
    let s2 = body.find("## 2. Vision (operator)").unwrap();
    let s3 = body.find("## 3. Decided things (operator-pruned)").unwrap();
    let s4 = body
        .find("## 4. Substrate slice — git-canonical store")
        .unwrap();
    let s5 = body.find("## 5. Latitude (operator)").unwrap();
    assert!(
        s1 < s2 && s2 < s3 && s3 < s4 && s4 < s5,
        "sections out of order"
    );

    // Auto-filled parent identity section names the parent + root + focus.
    assert!(body.contains("Parent project: **aida**"));
    assert!(body.contains("`/home/joe/ai/aida`"));
    assert!(body.contains("Handoff focus: **git-canonical store**"));

    // The operator-authored sections ship as TODO placeholders.
    assert_eq!(body.matches("- TODO:").count(), 4);
}

// trace:STORY-363 | ai:claude
#[test]
fn render_advisor_handoff_quotes_focus_with_special_chars_in_frontmatter() {
    let body = render_advisor_handoff(
        "parent",
        std::path::Path::new("/tmp/parent"),
        "orchestrator: drain & lease",
        "2026-06-06",
        "tester",
    );
    // yaml_scalar must quote a focus containing spaces/colons/ampersands.
    assert!(body.contains("focus: 'orchestrator: drain & lease'\n"));
    // The same focus flows into the substrate-section heading verbatim.
    assert!(body.contains("## 4. Substrate slice — orchestrator: drain & lease"));
}
