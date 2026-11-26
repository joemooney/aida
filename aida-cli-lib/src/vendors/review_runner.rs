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
    /// Reasoning-effort levels this vendor accepts. trace:STORY-3120 | ai:claude
    pub efforts: &'static [&'static str],
    /// Codex takes effort as a `-c` config override; the rest take `--effort`.
    pub effort_as_config: bool,
}

impl ReviewVendor {
    /// The fixed argv plus `--model`/effort flags, inserted before any `--`.
    /// Callers pass only values validated by `launch::review_tuning`.
    pub(crate) fn argv_with(&self, model: Option<&str>, effort: Option<&str>) -> Vec<String> {
        let mut extra = Vec::new();
        if let Some(model) = model {
            extra.extend(["--model".to_owned(), model.to_owned()]);
        }
        match effort {
            Some(e) if self.effort_as_config => {
                extra.extend(["-c".to_owned(), format!("model_reasoning_effort={e}")])
            }
            Some(e) => extra.extend(["--effort".to_owned(), e.to_owned()]),
            None => {}
        }
        let mut argv: Vec<String> = self.argv.iter().map(|a| (*a).to_owned()).collect();
        let at = self
            .argv
            .iter()
            .position(|a| *a == "--")
            .unwrap_or(argv.len());
        argv.splice(at..at, extra);
        argv
    }
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
    efforts: &["low", "medium", "high", "xhigh", "max"],
    effort_as_config: false,
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
    efforts: &["low", "medium", "high", "xhigh"],
    effort_as_config: true,
};

/// The finder when neither `--agent` nor `[agents] vendor` names one.
/// trace:BUG-2775 | ai:claude
pub(crate) const DEFAULT_REVIEW_VENDOR: &ReviewVendor = &CLAUDE;

/// Every vendor with a hardened review contract. trace:STORY-3120 | ai:claude
pub(crate) const REVIEW_VENDORS: [&ReviewVendor; 2] = [&CLAUDE, &CODEX];

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
