// trace:STORY-3120 | ai:claude
//! Per-vendor review-runner contract for `launch::review_runner`: the program,
//! a fixed argv that never carries the prompt, and the only auth variables the
//! runner may inherit.

use aida_core::vendors::LaunchVendor;
use anyhow::{anyhow, bail, Result};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReviewVendor {
    pub program: &'static str,
    pub argv: &'static [&'static str],
    pub auth: &'static [&'static str],
}

/// Claude: user settings only, hooks off, no MCP servers, no tools, no
/// commands; `--print` reads the prompt from stdin.
const CLAUDE: ReviewVendor = ReviewVendor {
    program: "claude",
    argv: &[
        "--print",
        "--setting-sources=user",
        "--settings",
        r#"{"disableAllHooks":true}"#,
        "--strict-mcp-config",
        "--tools",
        "",
        "--disable-slash-commands",
        "--no-session-persistence",
    ],
    auth: &[
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CONFIG_DIR",
    ],
};

/// Codex: read-only sandbox, no session file; `-` reads the prompt from stdin.
const CODEX: ReviewVendor = ReviewVendor {
    program: "codex",
    argv: &[
        "exec",
        "--sandbox=read-only",
        "--skip-git-repo-check",
        "--ephemeral",
        "--",
        "-",
    ],
    auth: &["OPENAI_API_KEY", "CODEX_HOME"],
};

/// The review contract for `raw`. Unknown names, and vendors without a
/// hardened review contract, are refused, never defaulted.
pub(crate) fn review_vendor(raw: &str) -> Result<&'static ReviewVendor> {
    match LaunchVendor::parse(raw).map_err(|e| anyhow!("{e}"))? {
        LaunchVendor::Claude => Ok(&CLAUDE),
        LaunchVendor::Codex => Ok(&CODEX),
        other => bail!(
            "{} has no review runner; use claude or codex",
            other.as_str()
        ),
    }
}
