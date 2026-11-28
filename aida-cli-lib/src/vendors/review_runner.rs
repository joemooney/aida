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
    /// Fixed environment variables always set on this vendor's child process
    /// (wins over `parent_env`), used to inject deny-all tool/permission config.
    // trace:STORY-3120 | ai:antigravity
    pub fixed_env: &'static [(&'static str, &'static str)],
    /// Reasoning-effort levels this vendor accepts.
    // trace:STORY-3120 | ai:claude
    pub efforts: &'static [&'static str],
    /// Codex takes effort as a `-c` config override; the rest take `effort_flag`.
    pub effort_as_config: bool,
    /// Optional CLI flag for reasoning effort (`"--effort"` for Claude/Agy, `"--thinking"` for Pi, `None` for OpenCode).
    // trace:STORY-3120 | ai:antigravity
    pub effort_flag: Option<&'static str>,
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
    /// Verified unable to read the user's files beyond the prompt. NOT "safe to
    /// publish": a confined codex or agy still reads its own auth token, so
    /// the publish gate is `RunnerCall::may_post` (this AND `post_allowed`).
    // trace:BUG-2783 | ai:claude
    pub no_read: bool,
    /// Run inside bubblewrap with an empty HOME holding only these auth files:
    /// the vendor's own tools can then read nothing else of the user's.
    // trace:BUG-2783 | ai:claude
    pub confined_auth: Option<ConfinedAuth>,
    /// May publish a PR comment. A confined runner can still read its own auth
    /// token, so only claude (no tools at all) posts until output redaction.
    // trace:BUG-2783 | ai:claude
    pub post_allowed: bool,
}

/// Where a confined vendor's own auth and config live: `dir` under HOME, or
/// the directory `dir_env` names. Only `files` in it are bound, read-only, at
/// `$HOME/<dir>/<file>`; an absent file is skipped.
///
/// | vendor | dir (or env)                   | files                                 |
/// |--------|--------------------------------|---------------------------------------|
/// | codex  | `.codex` (`CODEX_HOME`)        | `auth.json`, `config.toml`            |
/// | agy    | `.gemini/antigravity-cli`      | `antigravity-oauth-token`, `settings.json` |
// trace:BUG-2783 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ConfinedAuth {
    pub dir: &'static str,
    pub dir_env: Option<&'static str>,
    pub files: &'static [&'static str],
}

/// A review runner that exited because it is not signed in for headless use.
// trace:STORY-3202 | ai:claude
#[derive(Debug)]
pub(crate) struct AuthRequired {
    pub vendor: &'static str,
    pub stderr: String,
    /// The run was bubblewrap-confined, so its auth file was read-only.
    // trace:BUG-2783 | ai:claude
    pub confined: bool,
}

/// Why a confined run's auth can fail where an unconfined one would not: the
/// vendor's own token refresh cannot rewrite its read-only auth bind.
// trace:BUG-2783 | ai:claude
pub(crate) const CONFINED_AUTH_HINT: &str = "inside the review confinement its auth file is \
     read-only, and a token refresh needs a writable auth file; run the vendor CLI once \
     outside AIDA to refresh";

impl std::fmt::Display for AuthRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is not signed in for non-interactive use; `aida doctor --lane-host` names \
             the fix (stderr: {})",
            self.vendor, self.stderr
        )?;
        if self.confined {
            write!(f, "; {}: {CONFINED_AUTH_HINT}", self.vendor)?;
        }
        Ok(())
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
            Some(e) => {
                if let Some(flag) = self.effort_flag {
                    extra.extend([flag.to_owned(), e.to_owned()]);
                }
            }
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
    fixed_env: &[],
    efforts: &["low", "medium", "high", "xhigh", "max"],
    effort_as_config: false,
    effort_flag: Some("--effort"),
    print_timeout: None,
    auth_failure: &[],
    confinement_verified: true,
    // `--tools ""` leaves the session with no file-read tool at all.
    no_read: true,
    confined_auth: None,
    post_allowed: true,
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
    fixed_env: &[],
    efforts: &["low", "medium", "high", "xhigh"],
    effort_as_config: true,
    effort_flag: None,
    print_timeout: None,
    // A rejected or unrefreshable token: inside bubblewrap its auth.json is a
    // read-only bind, so a refresh that must rewrite it fails. trace:BUG-2783 | ai:claude
    auth_failure: &[
        "could not be refreshed",
        "refresh token",
        "401 unauthorized",
        "not logged in",
    ],
    // Tool use is confined; its whole-disk read scope is BUG-2783's.
    confinement_verified: true,
    // `--sandbox=read-only` reads anything visible, so the runner sees only
    // the bubblewrap view: system dirs, TLS/DNS files and its own auth.
    no_read: true,
    confined_auth: Some(ConfinedAuth {
        dir: ".codex",
        dir_env: Some("CODEX_HOME"),
        files: &["auth.json", "config.toml"],
    }),
    post_allowed: false,
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
    fixed_env: &[],
    efforts: &["low", "medium", "high", "xhigh", "max"],
    effort_as_config: false,
    effort_flag: Some("--effort"),
    print_timeout: Some("--print-timeout"),
    auth_failure: &[
        "authentication failed",
        "authentication required",
        "please sign in",
        "not logged into antigravity",
    ],
    confinement_verified: false,
    // Reads were seen allowed anywhere, so it runs in the bubblewrap view.
    no_read: true,
    confined_auth: Some(ConfinedAuth {
        dir: ".gemini/antigravity-cli",
        dir_env: None,
        files: &["antigravity-oauth-token", "settings.json"],
    }),
    post_allowed: false,
};

/// OpenCode (`opencode run`): non-interactive run mode reading the prompt from stdin,
/// with `OPENCODE_CONFIG_CONTENT` disabling every tool and denying every permission
/// rule (v1 `permission` map, v2 `permissions` array, and `tools` booleans).
/// Tool confinement and read restriction are not verified live and it does not use
/// the BUG-2783 bwrap wrapper, so `confinement_verified`, `no_read`, and `post_allowed`
/// stay `false`.
// trace:STORY-3120 | ai:antigravity
pub(crate) const OPENCODE: ReviewVendor = ReviewVendor {
    program: "opencode",
    argv: &["run"],
    auth: &[
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GEMINI_API_KEY",
        "OPENCODE_API_KEY",
    ],
    fixed_env: &[("OPENCODE_CONFIG_CONTENT", OPENCODE_DENY_ALL_CONFIG)],
    efforts: &["low", "medium", "high", "xhigh"],
    effort_as_config: false,
    effort_flag: None,
    print_timeout: None,
    auth_failure: &["authentication failed", "unauthorized", "api key"],
    confinement_verified: false,
    no_read: false,
    confined_auth: None,
    post_allowed: false,
};

/// OpenCode runtime configuration override (`OPENCODE_CONFIG_CONTENT`): disables
/// every built-in tool (`tools`) and denies every permission in both OpenCode v1
/// (`permission`) and v2 (`permissions`) schemas so a hostile prompt cannot run
/// shell commands, edit files, read local files, or fetch URLs.
const OPENCODE_DENY_ALL_CONFIG: &str = concat!(
    r#"{"tools":{"bash":false,"shell":false,"edit":false,"write":false,"patch":false,"#,
    r#""read":false,"glob":false,"grep":false,"list":false,"webfetch":false,"#,
    r#""todowrite":false,"todoread":false,"task":false,"mcp":false},"#,
    r#""permission":{"*":"deny","bash":"deny","shell":"deny","edit":"deny","write":"deny","#,
    r#""patch":"deny","read":"deny","glob":"deny","grep":"deny","list":"deny","#,
    r#""webfetch":"deny","todowrite":"deny","todoread":"deny","task":"deny","mcp":"deny"},"#,
    r#""permissions":[{"action":"*","resource":"*","effect":"deny"},"#,
    r#"{"action":"shell","resource":"*","effect":"deny"},"#,
    r#"{"action":"bash","resource":"*","effect":"deny"},"#,
    r#"{"action":"edit","resource":"*","effect":"deny"},"#,
    r#"{"action":"write","resource":"*","effect":"deny"},"#,
    r#"{"action":"read","resource":"*","effect":"deny"},"#,
    r#"{"action":"webfetch","resource":"*","effect":"deny"}]}"#
);

/// Pi coding agent (`pi --print --no-tools --no-session --no-extensions --no-skills --no-prompt-templates`):
/// non-interactive print mode reading the prompt from stdin with all tools, extensions,
/// skills, and prompt templates disabled, taking reasoning effort via `--thinking <LEVEL>`.
/// `confinement_verified`, `no_read`, and `post_allowed` stay `false` unless run through the BUG-2783 bwrap wrapper.
// trace:STORY-3120 | ai:antigravity
pub(crate) const PI: ReviewVendor = ReviewVendor {
    program: "pi",
    argv: &[
        "--print",
        "--no-tools",
        "--no-session",
        "--no-extensions",
        "--no-skills",
        "--no-prompt-templates",
    ],
    auth: &[
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GEMINI_API_KEY",
        "PI_CODING_AGENT_DIR",
    ],
    fixed_env: &[],
    efforts: &["low", "medium", "high", "xhigh"],
    effort_as_config: false,
    effort_flag: Some("--thinking"),
    print_timeout: None,
    auth_failure: &["authentication failed", "unauthorized", "api key"],
    confinement_verified: false,
    no_read: false,
    confined_auth: None,
    post_allowed: false,
};

/// agy's session-only settings: deny every permission rule kind it names
/// (file reads at any path, writes, shell commands, MCP tools and URL access).
const AGY_DENY_ALL_SETTINGS: &str = concat!(
    r#"--settings={"permissions":{"deny":["read_file(*)","read_file(/)","#,
    r#""write_file(*)","write_file(/)","command(*)","mcp(*)","read_url(*)","#,
    r#""execute_url(*)","download_url(*)"]}}"#
);

/// `agy models` for the lane-host auth probe: needs sign-in, sends no prompt,
/// so it carries no untrusted input to confine.
pub(crate) const AGY_AUTH_PROBE: ReviewVendor = ReviewVendor {
    argv: &["models"],
    print_timeout: None,
    confined_auth: None,
    ..AGY
};

/// The finder when neither `--agent` nor `[agents] vendor` names one.
// trace:BUG-2775 | ai:claude
pub(crate) const DEFAULT_REVIEW_VENDOR: &ReviewVendor = &CLAUDE;

/// Every vendor with a hardened review contract.
// trace:STORY-3120 | ai:claude
pub(crate) const REVIEW_VENDORS: [&ReviewVendor; 5] = [&CLAUDE, &CODEX, &AGY, &OPENCODE, &PI];

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

/// Look up a review vendor by CLI name (`claude`, `codex`, `agy`/`antigravity`,
/// `opencode`, or `pi`).
// trace:STORY-3120 | ai:antigravity
pub(crate) fn review_vendor_ext(raw: &str) -> Result<&'static ReviewVendor> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("opencode") {
        return Ok(&OPENCODE);
    }
    if trimmed.eq_ignore_ascii_case("pi") {
        return Ok(&PI);
    }
    review_vendor(trimmed)
        .map_err(|_| anyhow!("`{trimmed}` is not a vendor AIDA can launch for code review; use claude, codex, antigravity, opencode or pi"))
}
