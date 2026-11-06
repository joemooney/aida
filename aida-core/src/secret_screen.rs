//! Shared conservative credential token screening for provenance surfaces.
// trace:STORY-1468 | ai:codex
fn token_pattern() -> &'static regex::Regex {
    static TOKENS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\b(?:gh[pousr]_|github_pat_|sk-|xox[bp]-)|\bAKIA[A-Z0-9]{16}\b")
            .unwrap()
    });
    &TOKENS
}

/// Match the beginning of a whitespace token, as the processing-record redactor does.
pub fn looks_secret_token(tok: &str) -> bool {
    let t = tok.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
    token_pattern()
        .find(t)
        .is_some_and(|matched| matched.start() == 0)
}

/// Scan prose too, including tokens inside URLs, quotes or punctuation.
pub fn contains_secret_token(text: &str) -> bool {
    token_pattern().is_match(text)
}
