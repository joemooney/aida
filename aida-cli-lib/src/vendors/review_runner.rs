// trace:STORY-3120 | ai:claude
//! Per-vendor review-runner contract for `launch::review_runner`: the program,
//! a fixed argv that never carries the prompt, and the only auth variables the
//! runner may inherit.

use aida_core::vendors::LaunchVendor;
use anyhow::{anyhow, Result};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReviewVendor {
    pub program: &'static str,
    pub argv: &'static [&'static str],
    pub auth: &'static [&'static str],
    /// Reasoning-effort levels this vendor accepts.
    // trace:STORY-3120 | ai:claude
    pub efforts: &'static [&'static str],
    /// Codex takes effort as a `-c` config override; the rest take `--effort`.
    pub effort_as_config: bool,
    /// The vendor's own print-mode time limit, set inside the runner deadline
    /// so the CLI ends its turn before the group kill.
    // trace:STORY-3202 | ai:claude
    pub print_timeout: Option<&'static str>,
    /// Lowercase stderr fragments meaning "not signed in for headless use".
    pub auth_failure: &'static [&'static str],
    /// A live, signed-in run has verified this vendor's print-mode tool
    /// confinement. An unverified vendor runs only when named explicitly: it is
    /// never the automatic challenger and refuses `--post-comment`.
    // trace:STORY-3202 | ai:claude
    pub confinement_verified: bool,
    /// Verified unable to read files beyond the prompt. Only such a vendor may
    /// publish a PR comment: a prompt-injected diff could otherwise make it
    /// quote a local secret.
    // trace:BUG-2783 | ai:claude
    pub no_read: bool,
}

/// A review runner that exited because it is not signed in for headless use.
// trace:STORY-3202 | ai:claude
#[derive(Debug)]
pub(crate) struct AuthRequired {
    pub vendor: &'static str,
    pub stderr: String,
}

impl std::fmt::Display for AuthRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is not signed in for non-interactive use; `aida doctor --lane-host` names \
             the fix (stderr: {})",
            self.vendor, self.stderr
        )
    }
}

impl std::error::Error for AuthRequired {}

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

    /// Whether `output` carries one of this vendor's not-signed-in messages.
    pub(crate) fn auth_failed(&self, output: &[u8]) -> bool {
        let lower = String::from_utf8_lossy(output).to_ascii_lowercase();
        self.auth_failure.iter().any(|m| lower.contains(m))
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
    print_timeout: None,
    auth_failure: &[],
    confinement_verified: true,
    // `--tools ""` leaves the session with no file-read tool at all.
    no_read: true,
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
    print_timeout: None,
    auth_failure: &[],
    // Tool use is confined; its whole-disk read scope is BUG-2783's.
    confinement_verified: true,
    // `--sandbox=read-only` still reads any file the user can.
    no_read: false,
};

/// Antigravity: print mode with the empty `--print=` value, so the prompt is
/// read from the piped stdin; plan mode (no edits), no permission bypass, no
/// slash commands. The runner's empty cwd is an untrusted workspace with no
/// project settings, hooks or MCP config to load. `--sandbox` plus plan mode
/// auto-denies shell, web and writes headless (seen live), but reads were seen
/// allowed anywhere, so `--settings` denies every tool rule agy names. That
/// deny is not yet verified live: agy stays `confinement_verified: false`.
// trace:STORY-3202 | ai:claude
pub(crate) const AGY: ReviewVendor = ReviewVendor {
    program: "agy",
    argv: &[
        "--output-format=text",
        "--mode=plan",
        "--sandbox",
        AGY_DENY_ALL_SETTINGS,
        "--disable-slash-commands",
        "--print=",
    ],
    auth: &["GEMINI_API_KEY"],
    efforts: &["low", "medium", "high", "xhigh", "max"],
    effort_as_config: false,
    print_timeout: Some("--print-timeout"),
    auth_failure: &[
        "authentication failed",
        "authentication required",
        "please sign in",
        "not logged into antigravity",
    ],
    confinement_verified: false,
    // Its read deny is unverified, and reads were seen allowed anywhere.
    no_read: false,
};

/// agy's session-only settings: deny every permission rule kind it names
/// (file reads at any path, writes, shell commands, MCP tools and URL access).
const AGY_DENY_ALL_SETTINGS: &str = concat!(
    r#"--settings={"permissions":{"deny":["read_file(*)","read_file(/)","#,
    r#""write_file(*)","write_file(/)","command(*)","mcp(*)","read_url(*)","#,
    r#""execute_url(*)","download_url(*)"]}}"#
);

/// `agy models` for the lane-host auth probe: needs sign-in, sends no prompt.
pub(crate) const AGY_AUTH_PROBE: ReviewVendor = ReviewVendor {
    argv: &["models"],
    print_timeout: None,
    ..AGY
};

/// The finder when neither `--agent` nor `[agents] vendor` names one.
// trace:BUG-2775 | ai:claude
pub(crate) const DEFAULT_REVIEW_VENDOR: &ReviewVendor = &CLAUDE;

/// Every vendor with a hardened review contract.
// trace:STORY-3120 | ai:claude
pub(crate) const REVIEW_VENDORS: [&ReviewVendor; 3] = [&CLAUDE, &CODEX, &AGY];

/// The review contract for `raw`. Unknown names are refused, never defaulted;
/// every launch vendor has a hardened review contract.
// trace:STORY-3202 | ai:claude
pub(crate) fn review_vendor(raw: &str) -> Result<&'static ReviewVendor> {
    match LaunchVendor::parse(raw).map_err(|e| anyhow!("{e}"))? {
        LaunchVendor::Claude => Ok(&CLAUDE),
        LaunchVendor::Codex => Ok(&CODEX),
        LaunchVendor::Agy => Ok(&AGY),
    }
}
