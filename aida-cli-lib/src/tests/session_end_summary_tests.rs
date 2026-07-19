use super::{format_claude_session_line, format_session_end_summary};

#[test]
fn summary_includes_role_for_implementer() {
    let (head, claude) = format_session_end_summary(
        "019e2bd3abcd",
        Some("implementer"),
        "TASK-9",
        "task-9",
        None,
        None,
    );
    assert!(head.contains("role:implementer"), "got: {}", head);
    assert!(head.contains("scope TASK-9"));
    assert!(head.contains("branch task-9"));
    assert!(claude.is_none(), "no claude id → no claude line");
}

#[test]
fn summary_includes_role_for_reviewer() {
    let (head, _) = format_session_end_summary(
        "019e2bd3abcd",
        Some("reviewer"),
        "PR-30",
        "pr-30",
        None,
        None,
    );
    assert!(head.contains("role:reviewer"), "got: {}", head);
}

#[test]
fn summary_role_falls_back_to_unset_when_missing() {
    // manifest-missing / role-unset case.
    let (head, _) = format_session_end_summary("abcd1234", None, "x", "y", None, None);
    assert!(head.contains("role:unset"), "got: {}", head);
}

#[test]
fn summary_surfaces_claude_session_when_present() {
    let (_, claude) = format_session_end_summary(
        "abcd1234",
        Some("reviewer"),
        "PR-30",
        "pr-30",
        Some("f82f45c1-9978-4fad-8bb7-974103803afe"),
        Some(12),
    );
    let line = claude.expect("claude line present when manifest has the id");
    assert!(line.contains("f82f45c1"), "got: {}", line);
    assert!(line.contains("last active"), "got: {}", line);
}

#[test]
fn claude_line_omits_age_when_jsonl_unstattable() {
    let line = format_claude_session_line("abcdefghijklmnop", None);
    assert!(line.contains("abcdefgh"), "got: {}", line);
    assert!(!line.contains("last active"), "got: {}", line);
}
