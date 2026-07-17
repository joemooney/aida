use super::parse_bg_session_id;

#[test]
fn parses_short_id_from_real_claude_bg_output() {
    // Captured live 2026-05-29 against claude 2.1.156 — the operative
    // shape the bg-dispatch path joins on. trace:SPIKE-34 | ai:claude
    let sample = "Starting background service…\n\
                      backgrounded · c9021427\n  \
                      claude agents             list sessions\n  \
                      claude attach c9021427    open in this terminal\n  \
                      claude logs c9021427      show recent output\n  \
                      claude stop c9021427      stop this session\n";
    assert_eq!(parse_bg_session_id(sample).as_deref(), Some("c9021427"));
}

#[test]
fn returns_none_when_no_backgrounded_line() {
    assert!(parse_bg_session_id("some unrelated output\n").is_none());
    assert!(parse_bg_session_id("").is_none());
}

#[test]
fn tolerates_leading_whitespace_and_non_separator_chars() {
    // Defensive: claude could change separator from `·` to `:` or
    // drop the bullet entirely without changing the contract.
    assert_eq!(
        parse_bg_session_id("backgrounded: deadbeef").as_deref(),
        Some("deadbeef")
    );
    assert_eq!(
        parse_bg_session_id("  backgrounded   abc12345  ").as_deref(),
        Some("abc12345")
    );
}

#[test]
fn rejects_non_hex_id() {
    // If claude prints `backgrounded · ZZZ` we'd rather return None
    // than emit a bogus sessionId onto a lease.
    assert!(parse_bg_session_id("backgrounded · ZZZZZZ").is_none());
}
