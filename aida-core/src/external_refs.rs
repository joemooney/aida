//! One-way external issue references (STORY-476).
//!
//! AIDA composes with PM systems (Linear / Jira / GitHub) by recording a
//! validated `provider:id` string on a spec, rendering it as a clickable link
//! via configured base URLs, and making it searchable. This is a deliberately
//! *one-way* reference: AIDA stores the pointer; it does NOT sync state back to
//! the external system.
//!
//! A ref has the shape `<provider>:<id>` where `<provider>` is one of the
//! recognized providers (`linear`, `jira`, `github`) and `<id>` is the
//! provider-specific issue identifier:
//!
//! - `linear:LIN-123`
//! - `jira:PROJ-456`
//! - `github:owner/repo#123`
//!
//! trace:STORY-476 | ai:claude

use std::collections::HashMap;

/// The providers AIDA recognizes for external issue refs. Provider names are
/// matched case-insensitively and normalized to lowercase on store.
pub const KNOWN_PROVIDERS: &[&str] = &["linear", "jira", "github"];

/// A parsed external reference: `<provider>:<id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRef {
    /// Lowercased provider name (one of [`KNOWN_PROVIDERS`]).
    pub provider: String,
    /// The provider-specific issue id, preserved as the user entered it
    /// (e.g. `LIN-123`, `owner/repo#123`).
    pub id: String,
}

impl ExternalRef {
    /// The canonical stored form: `<provider>:<id>` with the provider
    /// lowercased and the id left untouched.
    pub fn canonical(&self) -> String {
        format!("{}:{}", self.provider, self.id)
    }
}

/// Validate and normalize a raw `provider:id` ref string.
///
/// Returns the parsed [`ExternalRef`] (provider lowercased, id trimmed) or an
/// error message describing what was wrong. Validation rules:
///
/// - exactly one `:` separating a non-empty provider from a non-empty id
///   (the id may itself contain `:` — only the first `:` is the separator);
/// - the provider (case-insensitive) must be one of [`KNOWN_PROVIDERS`];
/// - the id must be non-empty after trimming.
pub fn parse_ref(raw: &str) -> Result<ExternalRef, String> {
    let trimmed = raw.trim();
    let (provider_part, id_part) = trimmed.split_once(':').ok_or_else(|| {
        format!(
            "invalid external ref '{}': expected `provider:id` (e.g. linear:LIN-123, \
             jira:PROJ-456, github:owner/repo#123)",
            raw
        )
    })?;
    let provider = provider_part.trim().to_ascii_lowercase();
    let id = id_part.trim().to_string();
    if provider.is_empty() {
        return Err(format!(
            "invalid external ref '{}': provider is empty (expected one of: {})",
            raw,
            KNOWN_PROVIDERS.join(", ")
        ));
    }
    if id.is_empty() {
        return Err(format!(
            "invalid external ref '{}': id is empty (e.g. {}:ISSUE-123)",
            raw, provider
        ));
    }
    if !KNOWN_PROVIDERS.contains(&provider.as_str()) {
        return Err(format!(
            "unknown external-ref provider '{}' in '{}': expected one of: {}",
            provider,
            raw,
            KNOWN_PROVIDERS.join(", ")
        ));
    }
    Ok(ExternalRef { provider, id })
}

/// The built-in default base URL for a provider, used when the project hasn't
/// configured one under `[external_refs]` in `.aida/config.toml`. Returns
/// `None` for `github` because a github ref already encodes `owner/repo`, so a
/// fixed `https://github.com/` prefix resolves it without configuration.
fn default_base_url(provider: &str) -> Option<&'static str> {
    match provider {
        "github" => Some("https://github.com/"),
        // Linear / Jira workspaces are per-org, so there's no universal
        // default — the project configures these.
        _ => None,
    }
}

/// Render a single validated ref into a URL, given the project's configured
/// base URLs (provider -> base url). Returns `None` when no base URL is known
/// for the provider (neither configured nor a built-in default), in which case
/// the caller should show the bare `provider:id` text.
///
/// URL construction:
/// - `github:owner/repo#123` -> `<base>owner/repo/issues/123`
/// - any other provider      -> `<base><id>`
///
/// A configured base URL is used verbatim aside from ensuring a single
/// trailing `/` before the id is appended.
pub fn render_ref_url(r: &ExternalRef, base_urls: &HashMap<String, String>) -> Option<String> {
    let base = base_urls
        .get(&r.provider)
        .map(|s| s.as_str())
        .or_else(|| default_base_url(&r.provider))?;
    let base = base.trim();
    if base.is_empty() {
        return None;
    }
    let base_slash = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{}/", base)
    };
    let tail = match r.provider.as_str() {
        "github" => github_issue_path(&r.id),
        _ => r.id.clone(),
    };
    Some(format!("{}{}", base_slash, tail))
}

/// Translate a github ref id of the form `owner/repo#123` into the URL path
/// `owner/repo/issues/123`. If the id doesn't match that shape it's returned
/// unchanged (best-effort — the bare id still appends to the base).
fn github_issue_path(id: &str) -> String {
    if let Some((repo, num)) = id.split_once('#') {
        let repo = repo.trim_end_matches('/');
        if !repo.is_empty() && !num.is_empty() {
            return format!("{}/issues/{}", repo, num);
        }
    }
    id.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parse_accepts_each_known_provider() {
        assert_eq!(
            parse_ref("linear:LIN-123").unwrap(),
            ExternalRef {
                provider: "linear".into(),
                id: "LIN-123".into()
            }
        );
        assert_eq!(
            parse_ref("jira:PROJ-456").unwrap(),
            ExternalRef {
                provider: "jira".into(),
                id: "PROJ-456".into()
            }
        );
        assert_eq!(
            parse_ref("github:owner/repo#123").unwrap(),
            ExternalRef {
                provider: "github".into(),
                id: "owner/repo#123".into()
            }
        );
    }

    #[test]
    fn parse_lowercases_provider_and_trims() {
        let r = parse_ref("  Linear : LIN-9  ").unwrap();
        assert_eq!(r.provider, "linear");
        assert_eq!(r.id, "LIN-9");
        assert_eq!(r.canonical(), "linear:LIN-9");
    }

    #[test]
    fn parse_rejects_unknown_provider() {
        let err = parse_ref("asana:TASK-1").unwrap_err();
        assert!(err.contains("unknown external-ref provider"), "{err}");
    }

    #[test]
    fn parse_rejects_missing_colon() {
        let err = parse_ref("LIN-123").unwrap_err();
        assert!(err.contains("expected `provider:id`"), "{err}");
    }

    #[test]
    fn parse_rejects_empty_id() {
        let err = parse_ref("linear:").unwrap_err();
        assert!(err.contains("id is empty"), "{err}");
    }

    #[test]
    fn parse_rejects_empty_provider() {
        let err = parse_ref(":LIN-1").unwrap_err();
        assert!(err.contains("provider is empty"), "{err}");
    }

    #[test]
    fn parse_keeps_extra_colons_in_id() {
        // Only the first colon is the separator.
        let r = parse_ref("jira:PROJ-1:sub").unwrap();
        assert_eq!(r.id, "PROJ-1:sub");
    }

    #[test]
    fn render_uses_configured_base_url() {
        let bases = base_map(&[("linear", "https://linear.app/acme/issue/")]);
        let r = parse_ref("linear:LIN-123").unwrap();
        assert_eq!(
            render_ref_url(&r, &bases).unwrap(),
            "https://linear.app/acme/issue/LIN-123"
        );
    }

    #[test]
    fn render_adds_missing_trailing_slash() {
        let bases = base_map(&[("jira", "https://acme.atlassian.net/browse")]);
        let r = parse_ref("jira:PROJ-456").unwrap();
        assert_eq!(
            render_ref_url(&r, &bases).unwrap(),
            "https://acme.atlassian.net/browse/PROJ-456"
        );
    }

    #[test]
    fn render_github_uses_builtin_default_and_issue_path() {
        let bases = HashMap::new();
        let r = parse_ref("github:owner/repo#123").unwrap();
        assert_eq!(
            render_ref_url(&r, &bases).unwrap(),
            "https://github.com/owner/repo/issues/123"
        );
    }

    #[test]
    fn render_github_respects_configured_override() {
        let bases = base_map(&[("github", "https://ghe.acme.com/")]);
        let r = parse_ref("github:owner/repo#7").unwrap();
        assert_eq!(
            render_ref_url(&r, &bases).unwrap(),
            "https://ghe.acme.com/owner/repo/issues/7"
        );
    }

    #[test]
    fn render_returns_none_without_base_for_linear() {
        let bases = HashMap::new();
        let r = parse_ref("linear:LIN-1").unwrap();
        assert!(render_ref_url(&r, &bases).is_none());
    }

    #[test]
    fn render_github_non_issue_id_falls_back_to_bare_append() {
        let bases = base_map(&[("github", "https://github.com/")]);
        let r = ExternalRef {
            provider: "github".into(),
            id: "owner/repo".into(),
        };
        assert_eq!(
            render_ref_url(&r, &bases).unwrap(),
            "https://github.com/owner/repo"
        );
    }
}
