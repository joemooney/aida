//! Shared conservative credential token screening for provenance surfaces.
// trace:STORY-1468 | ai:codex
const TOKEN_PREFIXES: &str = r"gh[pousr]_|github_pat_|sk-|xox[bp]-";

fn token_pattern() -> &'static regex::Regex {
    static TOKENS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(&format!(r"\b(?:{TOKEN_PREFIXES})|\bAKIA[A-Z0-9]{{16}}\b")).unwrap()
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
    // Keep prefix-only redaction conservative, but avoid refusing ordinary prose
    // such as sk-learn. Share prefixes while requiring a token-sized payload.
    static PROSE_TOKENS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(&format!(
            r"\b(?:{TOKEN_PREFIXES})[A-Za-z0-9_-]{{20,}}|\bAKIA[A-Z0-9]{{16}}\b"
        ))
        .unwrap()
    });
    PROSE_TOKENS.is_match(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_requires_token_sized_payload_but_redaction_keeps_short_prefixes() {
        for text in [
            "Use sk-learn",
            "sk-SE",
            "xoxb-example-token",
            "gho_short-token",
        ] {
            assert!(!contains_secret_token(text), "{text}");
        }
        for prefix in [
            "ghp_",
            "gho_",
            "ghu_",
            "ghs_",
            "ghr_",
            "github_pat_",
            "sk-",
            "xoxb-",
            "xoxp-",
        ] {
            assert!(looks_secret_token(&format!("{prefix}short")));
            assert!(!contains_secret_token(&format!(
                "{prefix}{}",
                "a".repeat(19)
            )));
            assert!(contains_secret_token(&format!(
                "https://example.test/{prefix}{}",
                "a".repeat(20)
            )));
        }
        assert!(contains_secret_token("quoted 'AKIA1234567890123456'"));
    }
}
