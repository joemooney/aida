//! SPIKE-33: build `claude-cli://open` deep-link URLs.
//!
//! Claude Code v2.1.91+ registers the `claude-cli://` URL scheme with the
//! OS. A click opens a new terminal, starts Claude Code in the right
//! working directory (via `cwd=` or `repo=`), and pre-fills the prompt
//! box from `q=` — INERT until the operator presses Enter. AIDA emits
//! these from `aida brief` and `aida goal` so a "paste this prompt into
//! Claude" workflow collapses to "click this link → review → press Enter."
//!
//! Hard limits per the official docs:
//!   - `q` max 5,000 characters AFTER encoding
//!   - `cwd` must be absolute; network/UNC paths rejected (caller's job)
//!   - `repo` is `owner/name`; resolves to the most-recently-used clone
//!
//! Operator-in-the-loop is preserved: the URL is inert until Enter,
//! prompts > 1,000 chars get a scroll-and-review banner from Claude
//! Code itself.
//!
//! trace:SPIKE-33 | ai:claude

use std::fmt::Write as _;
use std::path::Path;

/// Maximum length of the `q` parameter's encoded value (per Claude Code
/// docs). The caller MUST check `would_exceed_q_limit` before passing
/// large prompts — we surface this rather than truncate so the operator
/// sees the boundary.
pub const Q_MAX: usize = 5000;

/// One claude-cli:// open URL.
#[derive(Debug, Clone, Default)]
pub struct DeepLink {
    pub q: Option<String>,
    pub cwd: Option<String>,
    pub repo: Option<String>,
}

impl DeepLink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_prompt<S: Into<String>>(mut self, q: S) -> Self {
        self.q = Some(q.into());
        self
    }

    pub fn with_cwd<P: AsRef<Path>>(mut self, cwd: P) -> Self {
        self.cwd = Some(cwd.as_ref().display().to_string());
        self
    }

    pub fn with_repo<S: Into<String>>(mut self, repo: S) -> Self {
        self.repo = Some(repo.into());
        self
    }

    /// Render the URL. Returns the encoded URL plus a warning when the
    /// `q=` value exceeds the documented 5,000-character ceiling — the
    /// caller can show the warning or refuse to emit.
    pub fn render(&self) -> RenderedDeepLink {
        let mut url = String::from("claude-cli://open");
        let mut first = true;
        let mut q_len = 0usize;
        let mut push_param = |key: &str, val: &str| {
            let encoded = percent_encode(val);
            url.push(if first { '?' } else { '&' });
            first = false;
            let _ = write!(&mut url, "{}={}", key, encoded);
            if key == "q" {
                q_len = encoded.len();
            }
        };
        if let Some(q) = &self.q {
            push_param("q", q);
        }
        if let Some(cwd) = &self.cwd {
            push_param("cwd", cwd);
        }
        if let Some(repo) = &self.repo {
            push_param("repo", repo);
        }
        let exceeds = q_len > Q_MAX;
        RenderedDeepLink {
            url,
            q_len,
            exceeds,
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)] // q_len + would_exceed_q_limit are part of the module's
                    // public surface for future callers (ultraplan deep
                    // links, etc.); current consumers read only `exceeds`.
pub struct RenderedDeepLink {
    pub url: String,
    /// Length of the encoded `q` value (0 when q is absent).
    pub q_len: usize,
    /// True iff `q_len` > Q_MAX.
    pub exceeds: bool,
}

/// Quick check before assembling the link.
#[allow(dead_code)]
pub fn would_exceed_q_limit(prompt: &str) -> bool {
    percent_encode(prompt).len() > Q_MAX
}

/// Percent-encode per RFC 3986: unreserved (alpha / digit / `-_.~`)
/// pass through; everything else becomes `%XX` of its UTF-8 bytes.
/// Spaces become `%20` (not `+`) — the docs example uses `%20`.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        let b = *byte;
        let unreserved =
            b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~';
        if unreserved {
            out.push(b as char);
        } else {
            let _ = write!(&mut out, "%{:02X}", b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn percent_encode_passes_unreserved_through() {
        assert_eq!(percent_encode("abc-XYZ_0.9~"), "abc-XYZ_0.9~");
    }

    #[test]
    fn percent_encode_spaces_become_pct20() {
        assert_eq!(percent_encode("hello world"), "hello%20world");
    }

    #[test]
    fn percent_encode_newline_pct0a() {
        assert_eq!(percent_encode("a\nb"), "a%0Ab");
    }

    #[test]
    fn percent_encode_unicode_uses_utf8_bytes() {
        // U+2014 EM DASH → E2 80 94 in UTF-8
        assert_eq!(percent_encode("—"), "%E2%80%94");
    }

    #[test]
    fn render_minimal() {
        let r = DeepLink::new().render();
        assert_eq!(r.url, "claude-cli://open");
        assert!(!r.exceeds);
        assert_eq!(r.q_len, 0);
    }

    #[test]
    fn render_with_q_and_cwd_matches_docs_example_shape() {
        let r = DeepLink::new()
            .with_prompt("review open PRs")
            .with_cwd(PathBuf::from("/home/u/proj"))
            .render();
        assert!(r.url.starts_with("claude-cli://open?q="));
        assert!(r.url.contains("q=review%20open%20PRs"));
        assert!(r.url.contains("cwd=%2Fhome%2Fu%2Fproj"));
        assert!(!r.exceeds);
    }

    #[test]
    fn render_with_repo_only() {
        let r = DeepLink::new()
            .with_repo("acme/payments")
            .with_prompt("review open PRs")
            .render();
        assert!(r.url.contains("repo=acme%2Fpayments"));
        assert!(r.url.contains("q=review%20open%20PRs"));
    }

    #[test]
    fn q_max_boundary_is_5000() {
        // Pure ASCII that doesn't get percent-encoded.
        let s: String = "a".repeat(5000);
        assert!(!would_exceed_q_limit(&s));
        let s: String = "a".repeat(5001);
        assert!(would_exceed_q_limit(&s));
    }

    #[test]
    fn q_max_counts_encoded_bytes_not_chars() {
        // 5000 spaces → 15000 chars encoded → over limit.
        let s: String = " ".repeat(5000);
        assert!(would_exceed_q_limit(&s));
    }

    #[test]
    fn render_reports_exceeds_when_q_too_long() {
        let s: String = "a".repeat(5001);
        let r = DeepLink::new().with_prompt(s).render();
        assert!(r.exceeds);
    }
}
