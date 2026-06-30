use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// The canonical `aida solo <action>` verbs. The legacy
/// `--watch`/`--off`/`--status` flags map onto these as silent aliases.
// trace:STORY-627 | ai:claude
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoloAction {
    /// Start the solo loop in the foreground (== legacy `--watch`).
    Run,
    /// Stop a running solo loop (== legacy `--off`).
    Stop,
    /// Print whether a solo loop / solo mode is active (== legacy `--status`).
    Status,
}

#[derive(Parser, Debug)]
#[clap(
    author,
    version,
    about = "AI-native requirements management — durable, agent-readable specs",
    after_help = "Bare `aida` / `aida help` leads with a small Getting-started set. \
                  Run `aida help --all` for the full command surface grouped by topic."
)]
pub struct Cli {
    /// Path to the requirements file (overrides auto-detection)
    #[clap(long)]
    pub file: Option<String>,

    /// Project name to use from central registry
    #[clap(long, short = 'p')]
    pub project: Option<String>,

    /// Connect to a remote AIDA server (e.g., "localhost:50051" or "grpc://host:port")
    /// Can also be set via AIDA_SERVER environment variable
    #[clap(long, short = 's')]
    pub server: Option<String>,

    /// Record this invocation with asciinema when running in a terminal
    #[clap(long)]
    pub asciinema: bool,

    /// Output path for --asciinema recordings
    #[clap(long, value_name = "PATH", requires = "asciinema")]
    pub cast_out: Option<PathBuf>,

    /// Title for --asciinema recordings
    #[clap(long, value_name = "TITLE", requires = "asciinema")]
    pub cast_title: Option<String>,

    #[clap(subcommand)]
    pub command: Command,
}

/// Hidden substrate machinery invoked by hooks / scaffolding, not by humans.
// trace:STORY-684
#[derive(Subcommand, Debug)]
pub enum InternalCommand {
    /// Enforce the advisor-no-code-write invariant at the commit boundary.
    /// Called by the scaffolded git pre-commit hook so the gate binds any
    /// vendor's commit (Codex, raw terminal, headless), not just Claude. Exits
    /// non-zero (aborting the commit) when an advisor session stages code with
    /// no sanctioned-coding context.
    // trace:STORY-684
    AdvisorCodeGate,

    /// Record a `--no-verify` bypass into the field-study rule-violation log.
    /// Called by the scaffolded git post-commit hook AFTER the commit lands,
    /// once the hook has established (via the pre-commit sentinel marker) that
    /// the pre-commit hook was skipped — i.e. the commit was made with
    /// `git commit --no-verify`. A no-op unless the field study is enabled
    /// (`AIDA_FIELD_STUDY=1`); never fails the already-completed commit.
    // trace:TASK-917
    RecordNoVerifyBypass,
}

#[derive(Subcommand, Debug)]
pub enum ServerCommand {
    /// Check server status
    Status,

    /// List requirements from server
    List {
        /// Filter by status
        #[clap(long)]
        status: Option<String>,

        /// Filter by feature
        #[clap(long)]
        feature: Option<String>,

        /// Limit results
        #[clap(long, default_value = "100")]
        limit: i32,
    },

    /// Get a requirement from server
    Get {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
    },

    /// Ping server to check connectivity
    Ping,
}

/// Commands for code-to-requirement traceability
#[derive(Subcommand, Debug)]
pub enum TraceCommand {
    /// Add a trace link from code to a requirement
    Add {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req: String,

        /// Path to the source file
        #[clap(long)]
        file: String,

        /// Symbol name (function, struct, module, etc.)
        #[clap(long)]
        symbol: Option<String>,

        /// Starting line number
        #[clap(long)]
        line_start: Option<u32>,

        /// Ending line number
        #[clap(long)]
        line_end: Option<u32>,

        /// Artifact type: source, test, config, doc
        #[clap(long, short = 't', default_value = "source")]
        r#type: String,

        /// Notes about this trace link
        #[clap(long)]
        notes: Option<String>,

        /// Git commit hash where this was implemented
        #[clap(long)]
        commit: Option<String>,
    },

    /// List trace links for a requirement or file
    List {
        /// Requirement ID (UUID or SPEC-ID) - lists all trace links for this requirement
        #[clap(long)]
        req: Option<String>,

        /// File path - lists all trace links for this file
        #[clap(long)]
        file: Option<String>,
    },

    /// Remove a trace link
    Remove {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req: String,

        /// Trace link ID to remove
        #[clap(long)]
        link_id: String,
    },

    /// Scan source files for AIDA trace annotations
    Scan {
        /// Path to scan (file or directory, defaults to current directory)
        path: Option<String>,

        /// File extensions to scan (comma-separated, e.g., "rs,py,ts")
        #[clap(long, default_value = "rs")]
        extensions: String,

        /// Add discovered trace links to requirements database
        #[clap(long)]
        update: bool,

        /// Show verbose output
        #[clap(long, short = 'v')]
        verbose: bool,
    },

    /// Sweep git commits for requirement references
    Sweep {
        /// Number of commits to scan (default: all)
        #[clap(long)]
        limit: Option<u32>,

        /// Branch to scan (default: current)
        #[clap(long)]
        branch: Option<String>,

        /// Only show commits, don't update database
        #[clap(long)]
        dry_run: bool,

        /// Show verbose output
        #[clap(long, short = 'v')]
        verbose: bool,
    },

    /// CI spec-id validity gate: walk a commit range, resolve every
    /// `(SPEC-ID)` subject trailer against the live requirement graph, and
    /// exit non-zero when any commit references a SPEC-ID that does not exist
    /// or is rejected (a dead/dangling provenance link). Mechanical/release
    /// commits with no trailer are exempt; plan commits are skipped.
    Gate {
        /// Git revision range to scan (e.g. `origin/main..HEAD`). Defaults to
        /// `<default-branch>..HEAD` (the commits this branch adds), falling
        /// back to `HEAD~20..HEAD` when no default branch resolves.
        #[clap(long)]
        range: Option<String>,

        /// Emit machine-readable JSON instead of human text.
        #[clap(long)]
        json: bool,
    },

    /// CI trace-COVERAGE check: walk a diff and report which changed source
    /// hunks carry the required code-to-spec provenance (an inline trace
    /// comment OR a live commit `(SPEC-ID)` trailer), per the deterministic
    /// coverage definition + exemptions (tests / generated / docs / config /
    /// vendored / pure-deletion / fmt-only / trivial). Distinct from `gate`,
    /// which validates the trailer spec-ids; this checks that the changed CODE
    /// is traced. Report-only by default (CI succeeds); `--block` fails CI on
    /// any uncovered coverable hunk.
    Coverage {
        /// Git revision range to scan (e.g. `origin/main..HEAD`). Defaults to
        /// `<default-branch>..HEAD` (the commits this branch adds), falling
        /// back to `HEAD~20..HEAD` when no default branch resolves.
        #[clap(long)]
        range: Option<String>,

        /// Emit machine-readable JSON instead of human text.
        #[clap(long)]
        json: bool,

        /// Fail (exit non-zero) when any coverable changed hunk is uncovered.
        /// Default is report-only: the report prints but CI succeeds.
        #[clap(long)]
        block: bool,
    },

    /// Trace-ROT detector: scan source for inline code-to-spec trace markers,
    /// resolve each referenced id against the live requirement graph, and flag
    /// dangling traces — markers whose target no longer exists (deleted,
    /// renumbered) or resolves to a rejected spec. Reports total / resolved /
    /// dangling + a rot rate. This makes a stale trace go "red" like a failing
    /// type — the counterpart to `gate` (validates commit trailers) and
    /// `coverage` (checks the code is traced). Report-only by default (exit 0);
    /// `--block` exits non-zero when any dangling trace exists, for CI gating.
    Check {
        /// Path to scan (file or directory). Defaults to the project root.
        path: Option<String>,

        /// Emit machine-readable JSON instead of human text.
        #[clap(long)]
        json: bool,

        /// Fail (exit non-zero) when any dangling trace exists. Default is
        /// report-only: the report prints but the command succeeds.
        #[clap(long)]
        block: bool,
    },
}

/// Commands for generating reports
#[derive(Subcommand, Debug)]
pub enum ReportCommand {
    /// Generate AI integration report
    AiIntegration {
        /// Output format: markdown or html
        #[clap(long, short = 'f', default_value = "markdown")]
        format: String,

        /// Output file path (defaults to stdout)
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,

        /// Project root directory for scaffolding status check
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Include scaffolding status in report
        #[clap(long)]
        include_scaffold: bool,
    },
}

/// Dogfood metrics over the recorded telemetry substrate.
///
/// A reporting layer — computes nothing the telemetry logs don't already
/// record.
// trace:STORY-477 | ai:claude
#[derive(Subcommand, Debug)]
pub enum MetricsCommand {
    /// Agent-lift signals: autonomous drain success rate, runs over distinct
    /// specs/builds, stale-base recoveries, and the autonomous-vs-human split.
    /// Reads `~/.aida/auto-complete.jsonl` + `~/.aida/usage.jsonl`.
    // trace:STORY-477 | ai:claude
    AgentLift {
        /// Report over the last N days/hours/minutes (e.g. `30d`, `12h`).
        // trace:STORY-477 | ai:claude
        #[clap(long, value_name = "WINDOW", default_value = "30d")]
        since: String,
        /// Emit Markdown — suitable for pasting into release notes or a case
        /// study. Default is the colorized terminal view.
        // trace:STORY-477 | ai:claude
        #[clap(long)]
        markdown: bool,
        /// Emit a JSON object with the computed signals for machine consumers.
        // trace:STORY-477 | ai:claude
        #[clap(long, conflicts_with = "markdown")]
        json: bool,
    },
}

/// Observe-only rule-adherence field study.
///
/// The git log is the planted sensor: every commit records its message + diff.
/// `scan` recomputes the stated-rule verdicts (commit-format, trace-presence)
/// over recent commits into a local-only log; `report` aggregates it. Opt-in —
/// nothing is recorded until `AIDA_FIELD_STUDY=1` or `[field_study] enabled =
/// true`.
// trace:SPIKE-67 | ai:claude
#[derive(Subcommand, Debug)]
pub enum FieldStudyCommand {
    /// Harvest rule-adherence verdicts from recent commits into the local
    /// field-study log. Idempotent — a commit already recorded is skipped.
    // trace:SPIKE-67 | ai:claude
    Scan {
        /// Git revision range to scan (e.g. `HEAD~200`, a tag, `--since`-style
        /// rev). Default: the most recent commits up to `--limit`.
        // trace:SPIKE-67 | ai:claude
        #[clap(long, value_name = "REV")]
        since: Option<String>,
        /// Cap how many commits are inspected. Default 200.
        // trace:SPIKE-67 | ai:claude
        #[clap(long, default_value = "200")]
        limit: usize,
    },
    /// Report adherence rates from the local field-study log, bucketed by task
    /// span — the "does the would-block rate rise with span?" lens.
    // trace:SPIKE-67 | ai:claude
    Report {
        /// Emit a JSON object for machine consumers.
        // trace:SPIKE-67 | ai:claude
        #[clap(long)]
        json: bool,
    },
    /// Report the live drain-observed stated-rule violations — the real-time
    /// gate-vs-rule signal: which stated rules a real drain broke (CI red on
    /// fmt/clippy/provenance, a punt, a reviewer flag) with no gate to stop
    /// them.
    // trace:SPIKE-67 | ai:claude
    Violations {
        /// Emit a JSON object for machine consumers.
        // trace:SPIKE-67 | ai:claude
        #[clap(long)]
        json: bool,
    },
}

/// Starter-memory-pack substrate-drift discovery.
///
/// The opt-in memory pack (`aida init --with-memories`) ships generic
/// AIDA-using discipline as Claude Code project memories. The pack grows
/// over time inside the `aida` binary, but a project that scaffolded it
/// months ago has no way to learn it's behind — `--refresh` only helps if
/// you already KNOW to run it. These commands close that discoverability gap.
// trace:STORY-410 | ai:claude
#[derive(Subcommand, Debug)]
pub enum MemoriesCommand {
    /// Compare the local memory pack to this binary's embedded master and
    /// report drift (missing, stale, edited, up-to-date). Reads only — never
    /// writes. The fix it recommends is `aida init --with-memories --refresh`.
    // trace:STORY-410 | ai:claude
    Check {
        /// List every item in each category. Default summarizes (max 5 per
        /// category).
        #[clap(long, short = 'v')]
        verbose: bool,

        /// Emit a machine-readable JSON report instead of the text summary.
        #[clap(long)]
        json: bool,
    },
}

/// Commands for scaffolding management
#[derive(Subcommand, Debug)]
pub enum ScaffoldCommand {
    /// Check scaffolding status against actual project files
    Status {
        /// Project root directory (defaults to current directory)
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Show detailed file comparisons
        #[clap(long, short = 'v')]
        verbose: bool,

        /// Generate HTML report with diffs
        #[clap(long)]
        report: bool,

        /// Output file for report (defaults to stdout)
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,
    },

    /// Preview scaffolding artifacts without applying
    Preview {
        /// Project root directory (defaults to current directory)
        #[clap(long)]
        project_root: Option<PathBuf>,
    },

    /// Apply scaffolding to project
    Apply {
        /// Project root directory (defaults to current directory)
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Overwrite existing files
        #[clap(long)]
        force: bool,

        /// Show what would be done without making changes
        #[clap(long)]
        dry_run: bool,

        /// Remove obsolete `aida-*` skills/commands/hooks that this AIDA
        /// version no longer ships (left over from an older version). Without
        /// this, they are only reported. Symlinks and non-`aida-` files are
        /// never touched.
        // trace:BUG-298 | ai:claude
        #[clap(long)]
        prune: bool,
    },

    /// Extract embedded templates to disk for customization
    Extract {
        /// Directory to extract templates to (defaults to ~/.config/aida/templates)
        #[clap(long)]
        output: Option<PathBuf>,

        /// Overwrite existing files
        #[clap(long)]
        force: bool,
    },

    /// Apply category-aware upgrades to scaffolded files. The right
    /// follow-up to `scaffold status` reporting drift — does the right
    /// thing per file category instead of `apply --force`'s blast-radius.
    ///
    /// Per category (see `docs/plans/2026-05-04-scaffold-categorization.md`):
    ///   - **template** (`.claude/skills/*`, `.claude/commands/*`,
    ///     `.claude/hooks/*`, `.claude/AIDA.md`, `.codex/skills/**`,
    ///     `.git/hooks/commit-msg`) — AIDA-owned; drifted files are
    ///     overwritten with the embedded template.
    ///   - **seed** (CLAUDE.md, AGENTS.md) — user-owned post-init;
    ///     drifted files are LEFT ALONE. Missing files are created.
    ///     AGENTS.md's delimited AIDA-AUTOGEN block IS upgraded.
    ///   - **managed-merge** (`.claude/settings.json`, `.mcp.json`) —
    ///     v1 leaves existing files alone (slot-merge deferred). Missing
    ///     files are created.
    ///
    /// Output groups by category and only mentions files that need
    /// attention or actually changed — no per-file noise for the
    /// happy-path matching files.
    // trace:FR-1-028 | ai:claude
    Upgrade {
        /// Project root directory (defaults to current directory)
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Show what would change without writing anything.
        #[clap(long)]
        dry_run: bool,

        /// Override the category strategy and overwrite every drifted
        /// file regardless of category. Equivalent to today's
        /// `apply --force` but with the cleaner per-category output.
        /// Use sparingly — overwrites your CLAUDE.md / AGENTS.md.
        #[clap(long)]
        force: bool,
    },

    /// Show a unified diff between embedded templates and on-disk files
    /// for any drifted scaffold artifact. Exits 1 if any drift is found,
    /// 0 if clean. Pairs with `aida scaffold status` — when that reports
    /// "N modified" this is how you actually see what changed.
    ///
    /// CLAUDE.md and AGENTS.md are diffed only on the AIDA-managed portion:
    /// CLAUDE.md is presence-only (drift = `@.claude/AIDA.md` import line
    /// missing); AGENTS.md compares just the content between
    /// `<!-- AIDA-AUTOGEN-BEGIN -->` / `<!-- AIDA-AUTOGEN-END -->` markers.
    /// Everything else gets a full-content diff.
    // trace:FR-1-027 | ai:claude
    Diff {
        /// Specific path (relative to project root) to diff. When omitted,
        /// every modified file is diffed in turn with file headers between.
        path: Option<PathBuf>,

        /// Project root directory (defaults to current directory)
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Force plain output even on a TTY (the colored crate normally
        /// auto-disables when piped). Equivalent to NO_COLOR=1.
        #[clap(long)]
        no_color: bool,

        /// Number of context lines around each hunk
        #[clap(long, short = 'U', default_value = "3")]
        context: usize,

        /// Print only the relative paths of drifted files (one per line),
        /// no diff body. Useful for piping into other tools.
        #[clap(long)]
        list: bool,
    },
}

/// Persona / hat commands. A role is a persistent named context that you
/// can resume across shells — captures the working directory, last
/// Inspect / resume past Claude Code sessions for this project, enriched
/// with the AIDA role and most-recent spec from each session's .jsonl.
/// Review-prompt generation and related review-workflow helpers. v1:
/// only `prompt`; future subcommands (e.g. `summary`, `checks`) can
/// land here without bloating the top-level `aida` help.
// trace:STORY-67 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum ReviewCommand {
    /// Generate a markdown review prompt from a set of linked
    /// requirements' acceptance criteria. Specs come either from an
    /// explicit `--specs` list (CSV) or from the commit-range of a
    /// PR/MR (parsed `(REQ-ID)` trailers in commit messages).
    ///
    /// For best results with --pr, install `gh` (https://cli.github.com)
    /// for GitHub or `glab` (https://gitlab.com/gitlab-org/cli) for
    /// GitLab. Without them, AIDA falls back to base=main + a local
    /// review branch named `pr-N` / `mr-N` — works when the PR was
    /// started via `aida session start --owns PR-N`, surprising
    /// otherwise.
    // trace:STORY-67, TASK-40 | ai:claude
    Prompt {
        /// Comma-separated list of spec IDs (e.g. "FR-N,STORY-M"). When
        /// given, --pr is ignored.
        #[clap(long, value_name = "SPEC-IDS")]
        specs: Option<String>,

        /// Pull spec IDs from the PR/MR's commit range. Forge auto-
        /// detected from origin URL; override with --forge github|gitlab.
        /// Resolves base/head via `gh pr view` / `glab mr view` when
        /// installed; otherwise falls back to base=main and a local
        /// review branch named `pr-N` / `mr-N`.
        // trace:TASK-40 | ai:claude
        #[clap(long, value_name = "N", conflicts_with = "specs")]
        pr: Option<u64>,

        /// Forge override for self-hosted GHE / self-hosted GitLab
        /// when --pr is used.
        #[clap(long, value_name = "FORGE")]
        forge: Option<String>,

        /// Write the prompt to PATH instead of stdout. Useful when
        /// piping into a freshly-launched Claude Code session
        /// (`aida review prompt --pr 2 --write .aida-review-prompt.md`).
        #[clap(long, value_name = "PATH")]
        write: Option<String>,
    },

    /// Assemble active review fragments into a root REVIEW.md file.
    // trace:SPIKE-35 | ai:antigravity
    Assemble {
        /// Output path for the assembled REVIEW.md file (default: REVIEW.md in project root).
        #[clap(long, short = 'o', value_name = "PATH")]
        output: Option<PathBuf>,
    },
}

/// Per-scope disposition / triage lease commands (the intake gate).
// trace:TASK-661 | ai:claude
#[derive(Subcommand, Debug)]
pub enum TriageCommand {
    /// Take the disposition lease for a scope before disposing drafts in it.
    /// Refused (naming the holder) if a live advisor already holds the scope.
    /// Idempotent for the same process re-acquiring. Default scope is the
    /// whole project.
    Acquire {
        /// Scope to dispose (e.g. a subsystem name). Omit for the whole
        /// project.
        #[clap(long)]
        scope: Option<String>,
        /// Override the owner id recorded on the lease (defaults to the
        /// shell user identity, same resolution as the queue).
        #[clap(long)]
        user: Option<String>,
    },
    /// Release the disposition lease for a scope you hold (no-op if you don't
    /// hold it).
    Release {
        /// Scope to release. Omit for the whole project.
        #[clap(long)]
        scope: Option<String>,
        /// Override the owner id (defaults to the shell user identity).
        #[clap(long)]
        user: Option<String>,
    },
    /// Show the live disposition leases for this project (dead-holder leases
    /// are reaped on read). `--json` for machine consumers.
    Status {
        /// Emit JSON instead of a table.
        #[clap(long)]
        json: bool,
    },
}

// trace:FR-1-043 | ai:claude
#[derive(Subcommand, Debug)]
pub enum SessionCommand {
    /// List recent Claude Code conversations for this project (cwd)
    /// with role + spec context extracted from each conversation's
    /// .jsonl. By default shows conversations with activity in the last
    /// 24 hours, up to 20 entries — `--all` bypasses the recency cutoff.
    ///
    /// This is the HISTORICAL view (past Claude Code conversations).
    /// For the live view of which scoped work leases are held right
    /// now, use `aida session leases`.
    // trace:BUG-98 | ai:claude
    // trace:BUG-522 | ai:claude — renamed from `session list` (the old
    // name collided with the leases view); `list` is kept as a
    // deprecated clap alias.
    ///
    /// The INITIAL TOPIC column is the conversation title Claude Code
    /// set at start — it is fixed and does NOT track current work, so
    /// for a long-running conversation it can read stale. Identify a
    /// conversation by its SPEC + AGE, not the topic.
    // trace:TASK-236
    #[clap(alias = "list")]
    Conversations {
        /// Show at most N conversations (default 20).
        #[clap(long, short = 'n', default_value = "20")]
        limit: usize,

        /// Plain output (no color), suitable for piping.
        #[clap(long)]
        no_color: bool,

        /// Bypass the default 24h recency cutoff and show every
        /// conversation the limit allows.
        // trace:STORY-59 | ai:claude
        #[clap(long)]
        all: bool,
    },

    /// Pick a session interactively and resume it via `claude --resume`.
    /// When `id` is given, resume that session directly without the
    /// picker. The picker shows the same enriched columns as `list`.
    Resume {
        /// Specific session id (or 8-char prefix) to resume. When
        /// omitted, opens an interactive picker.
        id: Option<String>,

        /// Show at most N sessions in the picker (default 20).
        #[clap(long, short = 'n', default_value = "20")]
        limit: usize,
    },

    /// Launch a new Claude Code session, recording the active role + a
    /// user-chosen title so `aida session list` can show them
    /// reliably (instead of greping the auto-generated subject). Execs
    /// `claude` once the launch metadata is recorded. By default this is
    /// a faithful launch — Claude's own native permission posture (it
    /// prompts). Turn on bypass for the whole fleet with `[agents] bypass
    /// = true` in `~/.aida/agents.toml` (project `.aida/agents.toml`
    /// overrides), or per-launch with `--permission-mode`.
    // trace:FR-1-044 trace:STORY-495 | ai:claude
    New {
        /// Title for the session (shown in `aida session list`). When
        /// omitted, you'll be prompted interactively.
        #[clap(long, short = 't')]
        title: Option<String>,

        /// Claude Code permission mode. When omitted, no `--permission-mode`
        /// is injected — Claude uses its native posture (the faithful
        /// default). Most common explicit values: `bypassPermissions`
        /// (no prompts), `acceptEdits` (auto-accept edits, prompt other
        /// tools), `auto` (research preview: auto-approves tool calls with
        /// background safety checks), `default` (prompt for everything),
        /// `plan`. The string is passed straight through to
        /// `claude --permission-mode`, so any value the installed Claude
        /// Code understands works.
        // trace:TASK-83 trace:STORY-495 | ai:claude
        #[clap(long)]
        permission_mode: Option<String>,

        /// Launch Claude in contained mode: strict Bash sandboxing, no
        /// unsandboxed fallback, destructive-command deny rules, and
        /// project-relative edit auto-allow only.
        #[clap(long)]
        sandbox: bool,

        /// Override the role recorded for this session (defaults to
        /// $AIDA_SESSION_ROLE).
        #[clap(long)]
        role: Option<String>,
    },

    /// Start a scoped session: create a sibling git worktree on a fresh
    /// branch, symlink AIDA state, and record a lease declaring this
    /// session owns a logical or physical scope (an EPIC, SPEC-ID, or
    /// path glob). v1 leases are advisory — `aida edit` / `aida-pickup`
    /// don't enforce them yet, but multiple sessions running concurrently
    /// can use the lease list to coordinate.
    // trace:EPIC-20 | ai:claude
    Start {
        /// Scope this session owns (alias: --spec <SCOPE>). Examples:
        ///   - `EPIC-N` / `epic-N` (resolved against the store)
        ///   - `FR-N` (any spec id)
        ///   - `src/scaffolding/**` (path glob — stored, not validated)
        ///   - `feature:auth` (free-form tag)
        #[clap(long, alias = "spec", value_name = "SCOPE")]
        owns: String,

        /// Branch name for the new worktree (default: derived from --owns).
        #[clap(long)]
        branch: Option<String>,

        /// Base branch to fork from (default: current branch).
        #[clap(long)]
        base: Option<String>,

        /// Check out the existing `--branch` instead of forking a new
        /// one — the fixup-on-an-existing-PR-branch flow (e.g.
        /// `aida session start --owns PR-N-fixup --branch <impl-branch>
        /// --reuse-branch`). Errors if the branch exists nowhere.
        /// Without this flag, an explicitly-named `--branch` that
        /// already exists is reused automatically (with a hint), and an
        /// auto-derived name always forks fresh. `--base` is ignored
        /// when reusing.
        // trace:TASK-245 | ai:claude
        #[clap(long)]
        reuse_branch: bool,

        /// Worktree directory (default: sibling of project root,
        /// `<repo-parent>/<repo-name>-<scope-slug>/`).
        #[clap(long, value_name = "PATH")]
        path: Option<String>,

        /// Forge override for `PR-N` / `MR-N` scopes when the origin
        /// URL doesn't auto-detect (self-hosted GHE/GitLab, multi-remote
        /// setups, etc.). Accepts `github` or `gitlab`.
        // trace:STORY-61 | ai:claude
        #[clap(long, value_name = "FORGE")]
        forge: Option<String>,

        /// Branch naming style when --branch isn't given.
        ///   `auto` (default): try slug, then slug-2..-10, then
        ///                     slug-YYYY-MM-DD, then slug-YYYY-MM-DD-2..-10.
        ///   `date`:           always slug-YYYY-MM-DD (with -N suffix on
        ///                     collision). Useful when every session
        ///                     should be traceable to its date.
        // trace:STORY-65 | ai:claude
        #[clap(long, value_name = "STYLE", default_value = "auto")]
        branch_style: String,

        /// After creating the worktree + lease, launch Claude Code inside
        /// it: chdirs into the worktree, records launch metadata in the
        /// same log as `aida session new`, then execs
        /// `claude --permission-mode <mode>`. Collapses the usual
        /// "start → cd → session new" three-step into one command.
        // trace:STORY-54 | ai:claude
        #[clap(long, short = 'l')]
        launch: bool,

        /// Title for the launched session (only used with --launch).
        /// Shown in `aida session list`. Prompted interactively when
        /// omitted; pass an empty string to skip the prompt.
        // trace:STORY-54 | ai:claude
        #[clap(long, short = 't')]
        title: Option<String>,

        /// Display name passed to Claude Code via `--name`. Shown in
        /// claude's prompt box, /resume picker, and terminal title.
        /// Without this, --launch derives a name from scope+branch+role:
        /// `review@PR-N` for reviewer sessions, `EPIC-N:batchM` for
        /// implementer epic-batch shapes, `<role>@<scope>:<suffix>`
        /// fallback. Truncated to 64 chars. Only honored with --launch.
        // trace:TASK-31 | ai:claude
        #[clap(long, short = 'n')]
        name: Option<String>,

        /// Claude Code permission mode for the launch. When omitted, no
        /// `--permission-mode` is injected — Claude uses its native posture
        /// (the faithful default; turn on bypass fleet-wide with
        /// `[agents] bypass = true`). Common explicit values:
        /// `bypassPermissions`, `acceptEdits`, `auto` (research-preview
        /// middle ground — see `aida queue work --help` for the tradeoff),
        /// `default`, `plan`. Pass-through to `claude --permission-mode`, so
        /// any installed-claude value works. Ignored without --launch.
        // trace:STORY-54, TASK-83 trace:STORY-495 | ai:claude
        #[clap(long)]
        permission_mode: Option<String>,

        /// With --launch, launch Claude in contained mode: strict Bash
        /// sandboxing, no unsandboxed fallback, destructive-command deny
        /// rules, and project-relative edit auto-allow only.
        #[clap(long)]
        sandbox: bool,

        /// Override the role recorded in this session's lease (and, when
        /// `--launch` is set, the persona the launched Claude inherits).
        /// Without `--role`, the role is derived from the scope:
        /// `--owns PR-N` / `--owns MR-N` → reviewer; everything else →
        /// implementer. `$AIDA_SESSION_ROLE` is a last-resort fallback
        /// only — when it disagrees with the scope-derived default the
        /// scope wins and a warning is printed.
        // trace:TASK-67 | ai:claude
        #[clap(long)]
        role: Option<String>,

        /// Claim a spec whose status is ambiguous: already In Progress
        /// with no local lease (another worktree or machine holds it),
        /// or NeedsAttention (punted by an autonomous agent and awaiting
        /// advisor triage). Without this flag those states refuse and
        /// ask you to triage first. Done / Completed / Rejected / Draft
        /// always refuse — `--force-claim` does NOT override them.
        // trace:BUG-379 | ai:claude
        #[clap(long)]
        force_claim: bool,

        /// Acquire the session's worktree from the warm-pool (reset a recycled
        /// tree instead of creating a fresh one — keeps the build cache warm).
        /// Overrides the `[worktree_pool] enabled` config for this run. Only
        /// affects the default new-branch flow (not --reuse-branch or PR
        /// review checkouts).
        // trace:STORY-714 | ai:claude
        #[clap(long)]
        pool: bool,

        /// Force a fresh `git worktree add` even when `[worktree_pool] enabled`
        /// is on — the opt-out counterpart to `--pool`.
        // trace:STORY-714 | ai:claude
        #[clap(long, conflicts_with = "pool")]
        no_pool: bool,
    },

    /// Register an existing Claude harness worktree lease from a SubagentStart hook.
    #[clap(hide = true)]
    HarnessWorktreeRegister {
        #[clap(long)]
        agent_id: String,
        #[clap(long)]
        cwd: String,
        #[clap(long)]
        agent_type: Option<String>,
        #[clap(long)]
        branch: Option<String>,
        #[clap(long)]
        scope: Option<String>,
    },

    /// Release a Claude harness worktree lease from a SubagentStop hook.
    #[clap(hide = true)]
    HarnessWorktreeRelease {
        #[clap(long)]
        agent_id: String,
    },

    /// End a scoped session: remove the worktree, delete the lease,
    /// leave the branch alone (merge/discard is up to the user). When
    /// `id` is omitted, ends the session whose lease names this cwd's
    /// worktree.
    ///
    /// As a side effect, when the just-ended session's branch has an
    /// open PR (detected via `gh`), files a Story-typed review item
    /// routed to the `reviewer` role with `implements` relations to
    /// every spec referenced in the PR's commit messages — so a
    /// forgotten `gh pr create` doesn't leave the reviewer unaware.
    // trace:STORY-66 | ai:claude
    End {
        /// Session id (8-char prefix accepted) to end. Omit to end the
        /// session matching the current cwd. Also accepts a SPEC-ID
        /// (e.g. `TASK-489` — treated as `--spec`) or a branch name
        /// (e.g. `task-489` — treated as `--branch`) when the lease
        /// covering cwd isn't the one you want to end.
        // trace:TASK-489 | ai:claude
        id: Option<String>,

        /// Resolve the lease by spec ID. Errors when zero or multiple
        /// leases own the spec; in the multi-match case lists them so
        /// you can disambiguate with the lease id.
        // trace:TASK-489 | ai:claude
        #[clap(long, conflicts_with = "branch")]
        spec: Option<String>,

        /// Resolve the lease by branch name. Errors when zero or
        /// multiple leases ride that branch.
        // trace:TASK-489 | ai:claude
        #[clap(long, conflicts_with = "spec")]
        branch: Option<String>,

        /// Skip the y/N confirmation.
        #[clap(long, short = 'y')]
        yes: bool,

        /// Force through safety checks. Two effects:
        ///   1. Force-terminate any live `claude` processes inside the
        ///      worktree (SIGTERM, 5s grace, then SIGKILL). Without this,
        ///      `session end` refuses to remove a worktree with live
        ///      claudes inside (prevents orphaned-claude-with-dangling-
        ///      cwd leak).
        ///   2. Discard uncommitted tracked/untracked-but-not-ignored
        ///      changes in the worktree. Without this, `session end`
        ///      refuses and prints the dirty file list. Gitignored
        ///      differences — `target/`, `.aida/cache.db`, etc. — never
        ///      require `--force` and are always discarded.
        // trace:BUG-61 | ai:claude
        // trace:BUG-67 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        force: bool,

        /// After removing the worktree, also delete the corresponding
        /// `~/.claude/projects/<encoded-cwd>/` directory so `claude
        /// --resume` doesn't try to revive sessions whose cwd no longer
        /// exists. Without this flag, the dir is left in place and a
        /// hint is printed pointing at `aida session prune --orphans`.
        // trace:TASK-70 | ai:claude
        #[clap(long = "purge-cc")]
        purge_cc: bool,

        /// Block until the PR's CI run reaches a terminal state (green
        /// or red) before releasing the lease — the SILENT variant:
        /// polls every 30s and prints only the poll ticks + final
        /// result. Use it to tail a long/overnight build without prompt
        /// noise. For live per-check progress instead, use `--watch-ci`.
        /// No-op when the session has no associated PR or `gh` isn't on
        /// PATH.
        // trace:TASK-111 | ai:claude
        #[clap(long, conflicts_with = "skip_ci")]
        wait_ci: bool,

        /// Block until CI reaches a terminal state — the LIVE variant:
        /// streams `gh run watch` so each check resolves on screen as it
        /// happens. Use it when you want interactive feedback; use the
        /// quieter `--wait-ci` to tail a build without the live display.
        /// Same end-session decision tree as `--wait-ci` once CI is
        /// terminal (green proceeds, red prompts).
        // trace:TASK-233 | ai:claude
        #[clap(long, conflicts_with = "skip_ci")]
        watch_ci: bool,

        /// Skip CI awareness entirely (release the lease without probing
        /// the PR's CI run). Use when CI is broken, when you're not the
        /// one who'll merge, or when the probe would be slow + you don't
        /// care.
        // trace:TASK-111
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        skip_ci: bool,

        /// Return this session's worktree to the warm-pool instead of
        /// removing it (now the DEFAULT for pooled worktrees): reset it to a
        /// clean detached base and mark it idle so the next acquire reuses it
        /// warm. Kept as an explicit opt-in for back-compat; a pooled tree is
        /// returned by default unless you pass `--remove`. No-op when the
        /// worktree isn't a registered pool tree.
        // trace:STORY-714 trace:TASK-985 | ai:claude
        #[clap(long = "return")]
        return_to_pool: bool,

        /// Delete the worktree instead of returning it to the warm-pool — the
        /// opt-out now that returning a pooled tree is the default. Forces the
        /// old destroy-and-recreate teardown (`git worktree remove`).
        // trace:TASK-985 | ai:claude
        #[clap(long, conflicts_with = "return_to_pool")]
        remove: bool,
    },

    /// List active session leases — the canonical "who holds what
    /// scoped work right now" view.
    ///
    /// For the historical record of past Claude Code conversations in
    /// this project, see `aida session conversations`.
    // trace:BUG-98 | ai:claude
    // trace:BUG-522 | ai:claude
    Leases {
        /// Probe live `claude` processes and warn about ones whose cwd is
        /// `(deleted)` — worktree was removed without ending claude. `-v`
        /// adds two columns: PID of the live claude (when present) and
        /// the Claude Code session id from
        /// `~/.claude/projects/<encoded>/<id>.jsonl`.
        // trace:STORY-69, TASK-56 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, short = 'v')]
        verbose: bool,
        /// Include stale leases in the listing. Default hides leases
        /// whose worktree no longer exists OR which have no live
        /// claude AND are >24h old. With --all, every lease renders
        /// with a state column (live ● / dormant ◐ / stale ⚠).
        // trace:TASK-55 | ai:claude
        #[clap(long)]
        all: bool,
    },

    /// Show details for one session lease (defaults to the lease covering
    /// cwd, or matched by ancestor PID). Accepts an 8-char id prefix.
    /// Output: scope, branch, worktree, inherited role, owner, hostname,
    /// lease file path, recent activity entries from the session log, and
    /// liveness (live claude in the worktree).
    // trace:STORY-68 | ai:claude
    Show {
        /// Session id (8-char prefix accepted). Omit to show the lease
        /// covering cwd, or — if cwd doesn't help — the lease whose
        /// creator_pid is in this shell's ancestor chain.
        id: Option<String>,

        /// Render the session's planned-cluster manifest as a status table
        /// (✓ Done / ◐ In progress / ○ Pending per item). Empty when
        /// /aida-pickup hasn't written a manifest for this session.
        // trace:STORY-98 | ai:claude
        #[clap(long)]
        plan: bool,
    },

    /// Manage the planned-cluster manifest for the active session
    /// (`.aida/sessions/<id>.manifest.toml`). Used by /aida-pickup to
    /// record which items the session intends to work, and by other
    /// commands to surface "planned by another session" cues.
    // trace:STORY-98 | ai:claude
    Manifest {
        #[command(subcommand)]
        cmd: SessionManifestCommand,
    },

    /// Delete Claude Code session metadata (`.jsonl` files under
    /// `~/.claude/projects/<encoded>/`) older than N days. Walks the
    /// current project + the parent project (when run inside a session
    /// worktree). Skips any project dir corresponding to an active
    /// `aida session` lease — a defensive guard so the user can run
    /// prune from anywhere without clobbering work in progress.
    ///
    /// Default behavior shows the candidates and asks for confirmation.
    /// Use `--dry-run` to preview without prompting, `--yes` to skip
    /// the prompt and delete. Each deletion is appended to
    /// `<project>/.aida/session-prune.log` so the action is auditable.
    // trace:STORY-60 | ai:claude
    Prune {
        /// Delete `.jsonl` files older than N days. Default 30.
        #[clap(long, default_value = "30")]
        days: u32,

        /// Show what would be deleted without touching anything. Use
        /// to preview before committing to a real prune.
        #[clap(long)]
        dry_run: bool,

        /// Skip the y/N confirmation. Without `--dry-run`, deletes
        /// immediately after printing the candidate list.
        #[clap(long, short = 'y')]
        yes: bool,

        /// Sweep `~/.claude/projects/*` for project dirs whose recorded
        /// cwd no longer exists on disk (orphans from removed worktrees).
        /// When set, each whole orphaned project dir is removed instead
        /// of file-by-file pruning. Composes with `--dry-run` and `--yes`.
        /// `--days` is ignored in this mode — orphan-ness is the trigger.
        // trace:TASK-70 | ai:claude
        #[clap(long)]
        orphans: bool,

        /// Sweep lingering session worktrees from `--escalate-blocks`
        /// punts whose spec has since been triaged out of Needs Attention.
        /// The autonomous-drain escalation path deliberately leaves the
        /// worktree alive in case the advisor's resume tier needs it; this
        /// flag finds the ones that never were resumed and removes the
        /// worktree + lease + manifest. Leases whose spec is still in
        /// Needs Attention are skipped (still awaiting triage). Composes
        /// with `--dry-run` and `--yes`. `--days` is ignored.
        // trace:TASK-358 | ai:claude
        #[clap(long, conflicts_with = "orphans")]
        escalations: bool,
    },

    /// Garbage-collect stale agent isolation worktrees under
    /// `.claude/worktrees/agent-*`. CONSERVATIVE by design: a worktree is
    /// removed ONLY when ALL of these hold — its branch is merged into the
    /// default branch (or it is detached/branchless), the worktree is CLEAN,
    /// it is NOT locked, and no live process / active session-lease references
    /// it. Anything else is PRESERVED and reported, never removed. Removal uses
    /// `git worktree remove` (NOT --force), so git's own safety checks add a
    /// second guard; a worktree git refuses to remove is skipped, never forced.
    ///
    /// Always runs the lossless `git worktree prune` first (clears bookkeeping
    /// for already-deleted worktree dirs). Never runs on the hot `aida status`
    /// read path — only on this explicit operator command and on
    /// `aida session end`.
    // trace:BUG-614 | ai:claude
    Gc {
        /// Show what would be removed and what is preserved without touching
        /// anything. Use to preview before committing to a real GC.
        #[clap(long)]
        dry_run: bool,

        /// Skip the y/N confirmation and remove the eligible worktrees
        /// immediately after printing the candidate list.
        #[clap(long, short = 'y')]
        yes: bool,
    },
}

// `session forget` (single-target .jsonl removal) and `session wakeup`
// (fallback-wakeup registry) were cut as orphaned zero-call verbs — no
// skill, harness, MCP tool, or internal command invoked them. `session
// prune` covers bulk cleanup; `session manifest` (MCP-wired) stays.
// trace:TASK-850 | ai:claude

/// Planned-cluster manifest subcommands. The manifest is a per-session
/// file at `.aida/sessions/<id>.manifest.toml` listing the SPEC-IDs the
/// session intends to work, written by /aida-pickup on cluster confirm.
// trace:STORY-98 | ai:claude
#[derive(Subcommand, Debug)]
pub enum SessionManifestCommand {
    /// Write (replace) the planned-cluster manifest for the active
    /// session. Pass items as a comma-separated SPEC-ID list in the order
    /// /aida-pickup intends to work them.
    ///
    /// Example:
    ///   aida session manifest write --items STORY-N,STORY-M,BUG-K
    ///
    // trace:STORY-98 | ai:claude
    // trace:TASK-487 | ai:claude
    Write {
        /// Comma-separated SPEC-IDs in planned order (position derives
        /// from order in the list, starting at 1).
        #[clap(long, value_name = "SPEC-IDS")]
        items: String,

        /// Plan source — typically "user prompt" (user-confirmed cluster)
        /// or "auto" (skill picked the head item). Free-form.
        #[clap(long, default_value = "user prompt")]
        source: String,

        /// Target session id (8-char prefix accepted). Defaults to the
        /// lease covering cwd / this shell's ancestor chain.
        #[clap(long)]
        session: Option<String>,
    },

    /// Mark `spec_id` as started in the active session's manifest
    /// (records started_at = now). Invoked by `aida edit --status
    /// in-progress` so the chip status flips automatically.
    // trace:STORY-98 | ai:claude
    MarkStarted {
        /// SPEC-ID to mark.
        spec_id: String,

        /// Target session id (8-char prefix accepted). Defaults to the
        /// lease covering cwd / this shell's ancestor chain.
        #[clap(long)]
        session: Option<String>,
    },

    /// Mark `spec_id` as completed in the active session's manifest
    /// (records completed_at = now). Invoked by `aida queue done` and
    /// `aida edit --status completed`.
    // trace:STORY-98 | ai:claude
    MarkCompleted {
        /// SPEC-ID to mark.
        spec_id: String,

        /// Target session id (8-char prefix accepted). Defaults to the
        /// lease covering cwd / this shell's ancestor chain.
        #[clap(long)]
        session: Option<String>,
    },
}

/// Pull-request side-effects intended to fire from /aida-pr at PR-create
/// time. Mirrors what `aida session end` would do as a backup, but moves
/// the trigger to the moment context is freshest (right after `gh pr
/// create` returns the URL).
// trace:STORY-90 | ai:claude
#[derive(Subcommand, Debug)]
pub enum PrCommand {
    /// File the reviewer story for the PR open on the current branch and
    /// queue it to the `reviewer` role. Idempotent: skips when a
    /// `Review PR-<n>:` story already exists for the PR. Detects the PR
    /// via `gh pr list --head <branch>` (so `gh` must be on PATH and
    /// authenticated).
    ///
    /// Intended trigger: `/aida-pr` runs this right after `gh pr create`
    /// succeeds — the agent already has the PR open, the commit range,
    /// and the spec list cached, but the auto-queue logic does its own
    /// gh detection so the call is self-contained.
    ///
    /// `aida session end` also fires this as a backup, so a forgotten
    /// /aida-pr (or a raw `gh pr create`) still ends up routed to the
    /// reviewer. The idempotency guard means both firing is fine.
    // trace:STORY-90 | ai:claude
    AutoQueueReview {
        /// Branch to look up the PR for. Defaults to the current
        /// branch via `git branch --show-current`.
        #[clap(long, value_name = "BRANCH")]
        branch: Option<String>,
    },

    /// Rebase a PR onto its base in a temporary worktree, then
    /// force-push-with-lease the rebased branch. Collapses the standard
    /// 6-command "rebase a PR before review" recipe into one call.
    ///
    /// Default mode aborts cleanly on conflicts (cleans up the temp
    /// worktree, prints the manual recipe, exits non-zero). `--check`
    /// reports stale-base + overlap + conflict-prediction without
    /// modifying anything. `--interactive` drops into the temp worktree
    /// on conflict so you can resolve + continue, then AIDA finishes the
    /// push. Cross-fork PRs are refused (force-push to a fork is the
    /// contributor's job).
    ///
    /// Force-push always uses `--force-with-lease`. The smoke check
    /// (default `cargo build --release` for Rust projects) is
    /// configurable via `.aida/config.toml` `[pr-rebase] smoke_check`.
    // trace:TASK-308 | ai:claude
    Rebase {
        /// PR number to rebase.
        #[clap(value_name = "N")]
        n: u64,

        /// Report-only: don't modify anything. Prints stale-base,
        /// overlapping files, and a best-effort conflict prediction.
        #[clap(long)]
        check: bool,

        /// On conflict, leave the temp worktree in place and prompt you
        /// to resolve + `git rebase --continue`, then finish the push.
        #[clap(long, conflicts_with = "check")]
        interactive: bool,

        /// Skip the post-rebase build smoke check.
        #[clap(long)]
        no_smoke: bool,

        /// Rebase onto this explicit base ref instead of the PR's
        /// declared base. Useful for PRs against non-main branches.
        #[clap(long, value_name = "REF")]
        base: Option<String>,
    },

    /// Ship a PR: create-if-needed → watch CI → squash-merge → pull →
    /// worktree-aware cleanup. The direct-publish counterpart to
    /// `aida queue work PR-N --auto-complete` (which drives the full
    /// reviewer pipeline) — use this for human-pre-approved work where
    /// no orchestrator review phase is needed (docs PRs, master-signed
    /// architecture work, recovery merges).
    ///
    /// With no `<N>`, the command targets the current branch's open PR
    /// — creating one via `gh pr create` (deriving title/body from the
    /// latest commit) if none exists. `aida pull` runs from the main
    /// worktree even when invoked from a sibling worktree, and the
    /// merge step detects branches checked out in sibling worktrees
    /// before deciding whether `--delete-branch` is safe — both
    /// papercuts surfaced by the 2026-05-22 stopgap-shell experience.
    ///
    /// Composes with the transient-retry layer for the `gh` calls
    /// (sub-second network blips no longer abort the flow).
    // trace:TASK-458 | ai:claude
    // trace:BUG-286 | ai:claude
    // trace:TASK-487 | ai:claude
    Ship {
        /// PR number to ship. When omitted, the command resolves the
        /// PR open on the current branch (or creates one if none
        /// exists).
        #[clap(value_name = "N")]
        n: Option<u64>,

        /// Skip the post-merge `aida pull`. Useful when shipping
        /// from inside a composition that pulls separately.
        #[clap(long)]
        no_pull: bool,

        /// Skip the `aida session end` worktree-cleanup step. Useful
        /// when you want to inspect the post-merge state before the
        /// worktree disappears.
        #[clap(long)]
        no_cleanup: bool,

        /// Print the resolved sequence without executing any of it.
        #[clap(long)]
        dry_run: bool,

        /// Delete the merged branch even when branches/PRs are stacked on
        /// it. Without this, ship keeps the branch alive when it detects
        /// stacked children, so deleting it can't auto-close their PRs;
        /// pass this to orphan the children deliberately.
        // trace:BUG-434 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        force_delete_branch: bool,

        /// Implementer's self-assessed actual complexity at ship time:
        /// `low` / `med` / `high`. Captured to
        /// `.aida/complexity-calibration/<SPEC>.yaml` alongside the
        /// punt count from `.aida/punts.jsonl` — feeds the three-way
        /// calibration view (`aida autonomy calibration mismatches`).
        /// Best-effort, not graded: estimate is a substrate-knowledge
        /// signal, never an approval criterion. Absent ⇒ ship slot
        /// records only the punt count.
        // trace:STORY-439 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_enum, value_name = "LEVEL")]
        complexity: Option<crate::complexity_calibration::ComplexityLevel>,

        /// Implementer's actual effort spent: 15m, 1h, 4h, 1d, or 1w.
        /// Captured to `.aida/effort-calibration/<SPEC>.yaml` as the
        /// ship/implementation touchpoint. `1d` is 8 work-hours; `1w`
        /// is 40 work-hours.
        // trace:STORY-451 | ai:codex
        #[clap(long, value_enum, value_name = "BUCKET")]
        effort: Option<crate::effort_calibration::EffortBucket>,

        /// Bypass the client-side trailer spec-ID check (Guard 1). Ship even
        /// when a commit's `(SPEC-ID)` trailer does not resolve to a live
        /// spec. Use only when you know the trailer is intentional (e.g. a
        /// cross-repo id the local store can't see).
        // trace:STORY-469 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        no_trailer_check: bool,
    },

    /// Deliberately HOLD the PR on the current session — push the branch but
    /// intentionally do not open the PR yet, pending a manual gate (a smoke
    /// test, an out-of-band review, an operator decision).
    ///
    /// Under an `--auto-complete` drain this is a clean finish outcome, NOT a
    /// failure: the orchestrator reads the hold signal this writes and reports
    /// a `Held` outcome (branch pushed, PR held) with the right recovery hint
    /// instead of mis-filing the missing PR as a phase-1 failure. Resolve the
    /// spec + branch from the active session lease; record `{spec, branch,
    /// reason}` for the orchestrator handshake.
    // trace:BUG-250 | ai:claude
    Hold {
        /// Why the PR is held — the gate you're running first. Surfaced in the
        /// drain epilogue.
        #[clap(long, value_name = "REASON")]
        reason: Option<String>,
    },
}

/// activity, optional purpose, and acts as a label in the statusline.
/// State lives at `<project>/.aida/roles/<name>.toml`.
// trace:EPIC-1-001 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RoleCommand {
    /// Enter (resume) an existing role. With no name on an interactive
    /// terminal, shows a picker of the project's roles; non-interactively
    /// it errors. Outputs shell code that must run in the calling shell:
    ///   raw binary: `eval "$(aida role enter advisor)"`
    ///   via the shell helper (`aida dev shell-init`): just `aida role enter advisor`
    /// The helper auto-evals it, so the bare form is correct there.
    Enter {
        /// Role name — picker shown when omitted on a TTY
        name: Option<String>,

        /// Restore the role's last working directory (cd in the eval output)
        #[clap(long)]
        cd: bool,
    },

    /// Add a new role, then enter it. Errors if the name already exists
    /// (use `aida role enter` to resume an existing one).
    Add {
        /// Role name — e.g. architect, reviewer, implementer
        name: String,

        /// Set the role's stated purpose
        #[clap(long)]
        purpose: Option<String>,

        /// Store this role globally (~/.aida/roles/) instead of per-project.
        /// Useful for personas you carry across projects (triage, code-review).
        #[clap(long)]
        global: bool,
    },

    /// List all roles for this project (and any global roles), sorted
    /// by last activity. Active role marked with `*`.
    List,

    /// Show details for one role (defaults to the active role if any).
    Show { name: Option<String> },

    /// Repair a corrupted role file: quarantine any unparseable
    /// activity-log entries, preserve the header and every well-formed
    /// entry, back the original up, and rewrite the file cleanly. A
    /// healthy file is left untouched. Defaults to the active role.
    // trace:TASK-956 | ai:claude — hidden from `role --help` (still runs).
    #[clap(hide = true)]
    Repair { name: Option<String> },

    /// Print the active role's name and exit, or exit 1 with empty
    /// stdout when no role is active. Pure read of `$AIDA_SESSION_ROLE`
    /// — no project-store load. Shell-friendly counterpart to
    /// `git branch --show-current`.
    // trace:TASK-42 | ai:claude
    Active,

    /// Print the active role's name on stdout (empty line when no role is
    /// active) and exit 0 either way. With `--check`, exit 1 instead when
    /// no role is active (still printing the name when one is). A pure read
    /// of `$AIDA_SESSION_ROLE` — no project-store load. Scripting-friendly
    /// surface for agents without direct env access.
    // trace:STORY-64 | ai:claude
    Current {
        /// Exit 1 (instead of 0) when no role is active.
        #[clap(long)]
        check: bool,
    },

    /// Deactivate the current role (state preserved). Outputs shell code:
    ///   `eval "$(aida role end)"`
    End,

    /// Delete a role's state file permanently. Confirms before deleting
    /// unless --yes is passed.
    Delete {
        name: String,

        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Install a starter set of global roles (implementer, advisor,
    /// reviewer) at ~/.aida/roles/ — the agent-wired role taxonomy.
    /// Idempotent — skips any that already exist; safe to re-run.
    /// (`architect` / `triage` are opt-in via `aida role add`.)
    Scaffold,

    /// Manage per-role scope filters. Scope filters are auto-applied to
    /// `aida list` and `aida queue list/next` while the role is active —
    /// e.g. set `--tags inbox --status draft` on the `triage` role to
    /// always see just the inbox-tagged drafts when wearing that hat.
    /// Override on a single command with explicit --tags/--status flags
    /// or --no-scope.
    // trace:TASK-1-021 | ai:claude
    #[clap(subcommand)]
    Scope(RoleScopeCommand),

    /// Manage the per-role Claude Code system-prompt addendum. The text
    /// is injected into Claude's context at session start via the
    /// aida-role-context.sh hook when this role is active.
    // trace:TASK-1-022 | ai:claude
    #[clap(subcommand)]
    Prompt(RolePromptCommand),
}

/// System-prompt addendum management. Text persists to the role's TOML
/// alongside scope filters; a SessionStart hook reads it and emits it
/// to the model as additionalContext.
// trace:TASK-1-022 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RolePromptCommand {
    /// Set the system-prompt addendum. Defaults to the active role; pass
    /// --name to target a different one. Pass content as the positional
    /// argument or via --content; --stdin reads from stdin (handy for
    /// multi-line text via `aida role prompt set --stdin < addendum.md`).
    Set {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,

        /// Addendum text (positional)
        content: Option<String>,

        /// Addendum text (alternative to positional, useful for shells
        /// that mangle special characters in arguments)
        #[clap(long, conflicts_with = "content")]
        content_flag: Option<String>,

        /// Read addendum from stdin
        #[clap(long, conflicts_with_all = ["content", "content_flag"])]
        stdin: bool,
    },

    /// Print the current system-prompt addendum (defaults to active role).
    Show {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,
    },

    /// Remove the system-prompt addendum (defaults to active role).
    Clear {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,
    },
}

/// Scope-filter management for a role. State persists to the role's TOML
/// file, so the filter follows the role into every shell that enters it.
// trace:TASK-1-021 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RoleScopeCommand {
    /// Set scope filters on a role. Defaults to the active role; pass --name
    /// to target a different one. At least one of --tags/--status required.
    Set {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,

        /// Comma-separated tags to AND into the default filter
        #[clap(long)]
        tags: Option<String>,

        /// Status to auto-apply (e.g. draft, approved, in-progress)
        #[clap(long)]
        status: Option<String>,
    },

    /// Show the scope filters for a role (defaults to active role).
    Show {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,
    },

    /// Clear scope filters. Without flags, clears all scope; with --tags
    /// or --status, clears just that field.
    Clear {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,

        /// Clear only the tag scope
        #[clap(long)]
        tags: bool,

        /// Clear only the status scope
        #[clap(long)]
        status: bool,
    },
}

/// Developer commands for working *on* AIDA itself (pyenv-style activation
// trace:STORY-527 | ai:claude
/// `aida burndown` — plan an autonomous backlog drain.
#[derive(Subcommand, Debug)]
pub enum BurndownCommand {
    /// Resolve which specs are ready to fan out vs parked, applying the
    /// pickability gate: bounded (not an epic), unblocked (no unsatisfied
    /// BlockedBy), decision-free (no pending question), not parking-tagged.
    /// Read-only — the run itself is the /aida-burndown skill in Claude Code,
    /// not a CLI subcommand. A "selector" (--status/--tag/--batch) narrows
    /// which specs the plan considers.
    Plan {
        /// Which specs to consider (the "selector"). Default: approved specs.
        #[clap(long, default_value = "approved")]
        status: String,
        /// Only specs carrying this tag.
        #[clap(long)]
        tag: Option<String>,
        /// Only specs in this batch (matches the `batch:<name>` tag).
        #[clap(long)]
        batch: Option<String>,
        /// Curation view: show approved + pickable specs that are NOT yet
        /// queued — the advisor's "what could I bless next" aid. Read-only;
        /// never auto-queues. (The default view's ready set requires queue
        /// membership — queueing a spec IS the advisor sign-off.)
        // trace:STORY-546 | ai:claude
        #[clap(long)]
        candidates: bool,
        /// Machine-readable JSON (`{ready, awaiting_signoff, parked}`).
        #[clap(long)]
        json: bool,
    },
    /// Explain why EVERY open spec is still open — not just the candidate set.
    /// Where `plan` shows ready-vs-parked for the specs a burndown would fan
    /// out, `explain` classifies all open specs (drafts, epics, deferred,
    /// held-for-review, blocked, in-flight, vision) into a bucket + one-line
    /// reason, derived purely from store signals. The post-burndown "why is
    /// each of these still here?" view.
    // trace:STORY-547 | ai:claude
    Explain {
        /// Machine-readable JSON (`[{spec, bucket, reason, needs_human}]`).
        #[clap(long)]
        json: bool,
    },
    /// Kick off and walk away: drain the advisor-blessed ready set in a headless
    /// Claude Code session. Launches `claude -p "/aida-burndown <selector>"`,
    /// which fans out worktree-isolated implementers in parallel, integrates
    /// their PRs, and loops until drained. Drains ONLY the queued + pickable set
    /// (advisor sign-off = queue membership) — never an unqueued spec.
    ///
    /// This is the autonomous, decision-FREE half of the drain. Its symmetric
    /// complement is `aida questions answer` (the interactive loop) — the
    /// HUMAN-decision drain that answers the parked, decision-REQUIRED specs and
    /// unparks them back into this ready set.
    // trace:STORY-555 | ai:claude
    // trace:STORY-545 | ai:claude
    Run {
        /// Which specs to consider (the "selector"). Default: approved.
        #[clap(long, default_value = "approved")]
        status: String,
        /// Only specs carrying this tag.
        #[clap(long)]
        tag: Option<String>,
        /// Only specs in this batch (matches the `batch:<name>` tag).
        #[clap(long)]
        batch: Option<String>,
        /// Cap the total number of specs drained this run (passed to the skill
        /// as `--max`). Default: drain until the blessed set is empty.
        #[clap(long)]
        max: Option<usize>,
        /// Parallel wave size — how many implementers fan out at once.
        #[clap(long)]
        concurrency: Option<usize>,
        /// Claude permission mode for the headless drain. Defaults to
        /// `bypassPermissions` so the unattended drain can push/merge/fan-out
        /// without stalling on prompts. Override with e.g. `acceptEdits`.
        #[clap(long)]
        permission_mode: Option<String>,
        /// Show the blessed ready set + the exact `claude -p` command that
        /// would run, then exit without launching.
        #[clap(long)]
        dry_run: bool,
        /// Stream live per-event progress instead of waiting silently. Launches
        /// the headless drain with `--output-format stream-json --verbose
        /// --include-partial-messages`, tees the JSONL to
        /// `.aida/burndown/<drain-id>.jsonl`, and renders a human-readable
        /// progress line per event to your terminal. AUTO-PROCEED is unchanged —
        /// this only adds visibility, never alters the drain's control flow. The
        /// teed JSONL is also the machine-readable substrate for future drain-
        /// status tooling.
        // trace:TASK-804 | ai:claude
        #[clap(long)]
        verbose: bool,
        // trace:STORY-647 | ai:claude
        /// Bypass the team RBAC guardrail (`[team.permissions] drain_start`).
        /// Starting an autonomous drain is an advisor-gated op by default; the
        /// gate is a guardrail, not security — the bypass is recorded in history.
        #[clap(long)]
        force: bool,
    },
    /// Is a drain running, and what is it doing? The read-side companion to
    /// `burndown run`. Reads the global drain lock — pid, start time, the
    /// launching command, host — and corroborates it against a PID-liveness
    /// probe (running vs a crashed/stale lock). Also lists the in-flight leased
    /// worktrees (the implementers fanned out under the drain) and points at the
    /// live event log (`.aida/burndown/<drain-id>.jsonl`) you can tail.
    /// Read-only; exits 0 whether or not a drain is running.
    // trace:TASK-806 | ai:claude (lock = BUG-538, event log = TASK-804)
    Status {
        /// Machine-readable JSON (`{drain:{running,pid,…}, in_flight:[…], log}`).
        #[clap(long)]
        json: bool,
    },
}

/// of an in-repo build, running dev servers, installing shell helpers).
// trace:EPIC-1-001 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DevCommand {
    /// Emit shell code that prepends the in-repo build dir to PATH.
    /// Use as: `eval "$(aida dev activate)"`. By default the freshest of
    /// `target/release/aida` vs `target/debug/aida` wins — pass `debug` /
    /// `release` (or `--debug` / `--release`) to PIN to a specific profile
    /// across subsequent re-activations. `--auto` clears the pin and
    /// returns to freshest-wins.
    Activate {
        /// Override the AIDA repo path (defaults to current directory if it
        /// looks like the aida repo, or $AIDA_DEV_REPO if set).
        #[clap(long)]
        repo: Option<String>,

        /// Profile shortcut: `debug` or `release` (positional). Equivalent
        /// to `--debug` or `--release`. Pins for subsequent re-activations.
        #[clap(value_parser = ["debug", "release", "auto"])]
        profile: Option<String>,

        /// Pin to the debug build (target/debug). Sticky — subsequent plain
        /// `aida dev activate` invocations honor the pin until you pass
        /// --auto or --release.
        // trace:FR-1-068 | ai:claude
        #[clap(long, conflicts_with_all = ["release", "auto"])]
        debug: bool,

        /// Pin to the release build (target/release). Sticky.
        // trace:FR-1-068 | ai:claude
        #[clap(long, conflicts_with_all = ["debug", "auto"])]
        release: bool,

        /// Clear any sticky profile pin and return to freshest-wins.
        #[clap(long, conflicts_with_all = ["debug", "release"])]
        auto: bool,
    },

    /// Emit shell code that undoes a previous `aida dev activate`.
    /// Use as: `eval "$(aida dev deactivate)"`.
    Deactivate,

    /// Run aida-server (REST/gRPC) and the React dev server (vite) in the
    /// foreground. Ctrl+C stops both. Defaults to serving the current
    /// project's store; React dev server only starts when run from the
    /// AIDA repo (since aida-web-react/ lives there).
    Serve {
        /// Override the REST/HTTP port for aida-server (default: 8080)
        #[clap(long)]
        rest_port: Option<u16>,

        /// Override the gRPC port for aida-server (default: 50051)
        #[clap(long)]
        grpc_port: Option<u16>,

        /// Override the vite dev server port (default: 5173)
        #[clap(long)]
        web_port: Option<u16>,

        /// Skip starting the React dev server even when aida-web-react/ exists
        #[clap(long)]
        no_web: bool,
    },

    /// Show current dev-activation state: is AIDA_DEV_ACTIVE set, what
    /// binary is being used, when was it built, what's the AIDA repo path.
    Status,

    /// Print shell helper functions (aida-on, aida-off) suitable for
    /// pasting into ~/.bashrc or ~/.zshrc. Pass --install to append them
    /// directly.
    ShellInit {
        /// Detect the user's shell and append the helpers to its rc file
        #[clap(long)]
        install: bool,
    },

    /// One-command release: bump version + tag + push (via scripts/release.sh),
    /// wait for the GitHub Actions workflow to publish binary tarballs, then
    /// upgrade your sibling installs to the new version. Default bump: patch.
    Release {
        /// Version bump kind: major, minor, patch, or an explicit version like "0.5.0"
        #[clap(default_value = "patch")]
        bump: String,
    },

    /// Alias for `aida dev release patch` — the most common case.
    Patch,
}

/// Commands for the SQLite read-cache that projects the git-canonical
/// store for fast list/filter/search queries.
// trace:EPIC-1-001 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    /// Force a full rebuild of the cache from the git store
    Rebuild,

    /// Show cache state (HEAD comparison, requirement count, last build time)
    Status,
}

/// Epic-scoped git-worktree management — the EPIC-55 workspace layer, mirroring
/// `git worktree`. First slice (STORY-716): create-or-enter a per-epic
/// workspace that is auto-scoped to that epic via `aida focus`.
///
/// STORY-714's warm-pool surface (`aida worktree pool status`) and tiered
/// removal (`aida worktree remove`) land as sibling variants under this enum
/// later — the surface is shaped to leave room for them.
// trace:STORY-716 | ai:claude
#[derive(Subcommand, Debug)]
pub enum WorktreeCommand {
    /// Create a git worktree for an epic off origin/main and auto-scope it to
    /// that epic. Default path `~/ai/aida-<epic-slug>` (e.g. `~/ai/aida-epic54`),
    /// default branch `<epic>-work` (e.g. `epic-54-work`). After creating the
    /// worktree it writes the `aida focus <epic>` marker INSIDE the new tree so
    /// reads there scope to the epic's subtree. Plain subcommand — prints the
    /// created path, does NOT cd (mirrors `git worktree add`). Idempotent: an
    /// existing worktree is reported + its focus re-affirmed, not re-created.
    Add {
        /// The epic to scope the worktree to (e.g. `EPIC-54`). Also the basis
        /// for the default path slug and branch name.
        epic: String,
        /// Override the worktree path (default `~/ai/aida-<epic-slug>`).
        #[clap(long, value_name = "PATH")]
        path: Option<String>,
        /// Override the branch name (default `<epic>-work`).
        #[clap(long, value_name = "BRANCH")]
        branch: Option<String>,
    },

    /// Create-if-missing, then cd into the epic's worktree. This emits shell
    /// (`cd '<path>'`) on stdout for the `aida()` wrapper to auto-eval, so a
    /// subprocess can move the parent shell. Run it BARE (e.g.
    /// `aida worktree enter <epic>`) — NOT wrapped in `eval "$(...)"`: the
    /// wrapper already auto-evals it, and double-eval would lose the cd (the
    /// no-double-eval convention). Without the wrapper installed, pipe the bare
    /// form through eval yourself: `eval "$(aida worktree enter <epic>)"`.
    Enter {
        /// The epic whose worktree to enter (created first if missing).
        epic: String,
        /// Override the worktree path (default `~/ai/aida-<epic-slug>`).
        #[clap(long, value_name = "PATH")]
        path: Option<String>,
        /// Override the branch name (default `<epic>-work`).
        #[clap(long, value_name = "BRANCH")]
        branch: Option<String>,
    },

    /// List AIDA-managed worktrees, each annotated with its `.aida/focus`.
    List {
        /// Machine-readable JSON (`[{path, branch, focus}]`).
        #[clap(long)]
        json: bool,
    },

    /// Manage the warm-pool of recycled worktrees — kept warm (reset-not-delete
    /// on hand-back) so build caches survive across fan-outs. Dissolves the
    /// cargo-cache poison and branch-stacking hazards of destroy-and-recreate.
    // trace:STORY-714 | ai:claude
    #[clap(subcommand)]
    Pool(WorktreePoolCommand),
}

/// Subcommands under `aida worktree pool`.
// trace:STORY-714 | ai:claude
#[derive(Subcommand, Debug)]
pub enum WorktreePoolCommand {
    /// Show every pooled worktree with its live state (available / in-use /
    /// leased / dirty / destroying) and current HEAD. Read-only.
    Status {
        /// Emit JSON instead of the human table.
        #[clap(long)]
        json: bool,
    },

    /// Acquire a worktree from the pool: reuse an idle warm tree (reset to a
    /// clean detached base) or create a new one up to the cap. Prints the path.
    Acquire {
        /// Stamp a durable reservation under this holder name (survives with no
        /// live process) — for a headless drain that parks work mid-flight.
        #[clap(long)]
        lease_holder: Option<String>,

        /// Emit JSON (`{"path": "..."}`) instead of the bare path.
        #[clap(long)]
        json: bool,
    },

    /// Return a worktree to the pool: reset it to a clean detached base and
    /// mark it idle. The directory PERSISTS (the warm cache is kept). Defaults
    /// to the pool tree containing the current directory.
    Return {
        /// Worktree path to return (default: the pool tree at the cwd).
        path: Option<String>,
    },

    /// Tear down pool worktrees. DRY-RUN BY DEFAULT — pass `--no-dry-run` to
    /// actually remove. Each tree is classified by risk; only `disposable`
    /// trees are removed unless you opt into a riskier class with one
    /// `--include-*` flag. This is the ONLY path that deletes a directory.
    Destroy {
        /// Specific worktree path(s) to destroy. Omit with `--all` for a bulk
        /// sweep (in which `--include-leased` is ignored as a safety stop).
        paths: Vec<String>,

        /// Target every pooled worktree.
        #[clap(long)]
        all: bool,

        /// Actually remove (the default is a preview).
        #[clap(long)]
        no_dry_run: bool,

        /// Allow removing dirty / unmerged / unverified trees (uncommitted or
        /// unlanded work is patch-salvaged first).
        #[clap(long)]
        include_unlanded: bool,

        /// Allow removing a tree with a live owner process.
        #[clap(long)]
        include_in_use: bool,

        /// Allow removing a durably-leased tree — honored only when the exact
        /// path is named, never in a `--all` sweep.
        #[clap(long)]
        include_leased: bool,

        /// Emit JSON instead of the human report.
        #[clap(long)]
        json: bool,
    },
}

/// Spec-quality tooling — checks you run ON a spec before work begins.
// trace:STORY-656 | ai:claude
#[derive(Subcommand, Debug)]
pub enum SpecCommand {
    /// Implementer-readiness pre-check: report whether a spec is ready to be
    /// picked up BEFORE it is queued, WITHOUT implementing anything. Always
    /// runs a deterministic 0-100 readiness score across weighted dimensions
    /// (description depth, an acceptance section, implementable type, priority,
    /// a linked parent, a not-too-vague heuristic), each with a pass/fail and a
    /// one-line reason. With `--ai`, also shells an AI pass (gated like
    /// `aida intent`, needs a TTY) that lists the questions an implementer
    /// would ask, the assumptions they'd make, and the ambiguities / missing
    /// acceptance it found.
    Dryrun {
        /// The SPEC-ID to pre-check (any story/task/bug/feature id).
        id: String,

        /// Also run the optional AI gap-report pass (shells `claude -p`; needs
        /// an interactive terminal). The deterministic pre-check runs either
        /// way — this only appends the AI report.
        #[clap(long)]
        ai: bool,

        /// Machine-readable JSON
        /// (`{spec, readiness_score, dimensions:[{name,pass,reason,weight}], ai_report?}`).
        #[clap(long)]
        json: bool,
    },

    /// Interview mode: resolve a spec's `dryrun` readiness gaps INTO the spec by
    /// asking clarifying questions. Runs the same deterministic readiness check
    /// `dryrun` does, turns each failing dimension into a question (missing
    /// acceptance -> "what does done look like?"; no parent -> "which spec is
    /// this a child of?"; no priority -> "high/medium/low?"), then folds the
    /// answers back into the spec as binding `## Acceptance` criteria and
    /// structured fields — never into comments.
    ///
    /// With a terminal it prompts for each gap in turn. Without one (the
    /// advisor/agent seat) it emits the structured question list and exits
    /// without blocking on stdin; feed answers back with `--answers <file>`.
    /// Propose-by-default: the spec is only written with `--apply`.
    // trace:STORY-657 | ai:claude
    Interview {
        /// The SPEC-ID to interview (any story/task/bug/feature id).
        id: String,

        /// Write the resolved answers into the spec (acceptance criteria,
        /// parent link, priority) and re-score. Without it, the command only
        /// proposes the edits / emits the question list — it never mutates the
        /// spec by surprise.
        #[clap(long)]
        apply: bool,

        /// Also derive questions from the optional AI gap report (shells
        /// `claude -p`, needs a TTY) — its ambiguities and implementer
        /// questions become extra acceptance-shaped questions.
        #[clap(long)]
        ai: bool,

        /// Answer the questions non-interactively from a JSON file (an array of
        /// `{"dimension":..,"answer":..}`, or `{"answers":[..]}`). Pair with
        /// `--apply` to fold them in. This is the agent/headless feedback path.
        #[clap(long, value_name = "FILE")]
        answers: Option<String>,

        /// Machine-readable JSON. Without answers, emits the question list
        /// (`{spec, readiness_score, questions:[{dimension,kind,prompt}]}`) for
        /// an agent to answer. With `--answers`/`--apply`, emits the applied
        /// edit summary.
        #[clap(long)]
        json: bool,
    },
}

/// Inspect / prune the durable processing-record audit trail.
// trace:STORY-582 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RecordCommand {
    /// List the processing records for one spec (or every spec carrying any,
    /// when no id is given).
    List {
        /// Spec id to inspect. Omit to walk every spec with a record.
        spec: Option<String>,
    },

    /// Trim processing records, keeping the spec. Records can be pruned later
    /// without losing the spec or its history. Propose-by-default — pass
    /// `--apply` to write.
    Prune {
        /// Restrict to one spec. Omit to sweep every spec.
        #[clap(long)]
        spec: Option<String>,

        /// Drop records older than this many days (e.g. `90`). When omitted,
        /// every record on the matched spec(s) is pruned.
        #[clap(long)]
        older_than: Option<u64>,

        /// Actually write the prune. Without it, the command only reports what
        /// it would remove (propose-by-default).
        #[clap(long)]
        apply: bool,
    },
}

/// Throwaway sandbox store for drain-testing and scenario play.
/// The sandbox is an ordinary git-canonical store living under a temp dir; it
/// is targeted via the `AIDA_STORE` env override, so it never touches the
/// project's real `aida-store` orphan branch.
// trace:SPIKE-48 | ai:claude
#[derive(Subcommand, Debug)]
pub enum SandboxCommand {
    /// Create a fresh throwaway store and print the `AIDA_STORE=...` export to
    /// activate it. Idempotent unless `--force` (refuses to clobber a populated
    /// existing sandbox without it).
    Create {
        /// Where to create the sandbox store. Default: a stable per-user temp
        /// dir (`$TMPDIR/aida-sandbox-<user>`), so re-running points at the
        /// same playground.
        #[clap(long)]
        path: Option<PathBuf>,

        /// Seed a few curated scenario specs (a lifecycle walk + a blocked-by
        /// chain) so there's something to play with immediately.
        #[clap(long)]
        seed: bool,

        /// Recreate even if the target already holds a populated sandbox store.
        #[clap(long)]
        force: bool,
    },

    /// Wipe the sandbox store's contents and re-initialize it empty (or seeded
    /// with `--seed`). The directory itself is reused.
    Reset {
        /// Sandbox store dir (default: the per-user temp sandbox).
        #[clap(long)]
        path: Option<PathBuf>,

        /// Re-seed curated scenario specs after the reset.
        #[clap(long)]
        seed: bool,
    },

    /// Delete the sandbox store directory entirely.
    Destroy {
        /// Sandbox store dir (default: the per-user temp sandbox).
        #[clap(long)]
        path: Option<PathBuf>,
    },

    /// Print the path of the (default or `--path`) sandbox store and whether it
    /// exists. With `--export`, print the `AIDA_STORE=...` line to eval.
    Path {
        /// Sandbox store dir (default: the per-user temp sandbox).
        #[clap(long)]
        path: Option<PathBuf>,

        /// Print a shell `export AIDA_STORE=...` line instead of the bare path.
        #[clap(long)]
        export: bool,
    },
}

/// Inter-agent mailbox — peer↔peer messaging between agents (distinct from
/// operator→agent briefs and top-down directives). Hybrid storage: a fast
/// local layer now, a git-canonical durable digest in a later slice.
// trace:STORY-493 | ai:claude
#[derive(Subcommand, Debug)]
pub enum MailboxCommand {
    /// Send a message to another agent, or `--broadcast` to all.
    Send {
        /// Recipient agent id (e.g. `codex`). Omit and pass `--broadcast` to reach everyone.
        #[clap(long)]
        to: Option<String>,

        /// Broadcast to every agent instead of a single recipient.
        #[clap(long, conflicts_with = "to")]
        broadcast: bool,

        /// The message body.
        body: String,

        /// Attach to an existing thread (default: start a new thread).
        #[clap(long)]
        thread: Option<String>,

        /// Id of the message this replies to.
        #[clap(long)]
        in_reply_to: Option<String>,

        /// Override the sender id (default: this shell's agent/user identity).
        #[clap(long)]
        from: Option<String>,

        /// Flag this as an urgent escalation ("stop"/heads-up) so it is
        /// surfaced out-of-band (statusline nag) instead of sitting unseen in
        /// a purely-chronological inbox. Lightweight: normal vs urgent only.
        #[clap(long)]
        urgent: bool,

        /// How the recipient should treat this message: `fyi` (informational,
        /// surface only), `request` (needs a response), or `handoff` (work
        /// transfer). Default: fyi. Orthogonal to --urgent (loudness vs kind).
        #[clap(long, value_name = "INTENT", default_value = "fyi")]
        intent: String,
    },

    /// Show an agent's inbox: messages addressed to it + broadcasts, oldest-first.
    /// Reading marks the inbox seen (clears unread); `--all` is the operator-wide
    /// read-only view across every agent.
    Inbox {
        /// Whose inbox (default: this shell's agent/user identity).
        agent: Option<String>,

        /// Operator-wide view: every message across all agents, oldest-first,
        /// with its recipient shown. Read-only — does not mark anything seen.
        #[clap(long, conflicts_with = "agent")]
        all: bool,

        /// Show the inbox WITHOUT marking it seen (does not advance the
        /// read-watermark). Lets a hook or a glance surface mail without
        /// consuming the unread flag — reading/acking stays an explicit act
        /// (a plain `aida mailbox inbox`).
        // trace:STORY-585
        #[clap(long, alias = "no-mark")]
        peek: bool,

        /// Show only UNREAD messages (newer than this inbox's read-watermark).
        // trace:STORY-585
        #[clap(long)]
        unread: bool,
    },

    // trace:STORY-585
    /// Ambient unread-mail notice for an agent's context: a capped, plain,
    /// role+user-scoped summary of unread messages, or nothing when the inbox
    /// is caught up. Never marks anything seen. The SessionStart / per-turn
    /// hook calls this; reading/acking stays the explicit `aida mailbox inbox`.
    Notice {
        /// Whose mail to summarize. Default: the union of this shell's
        /// agent/user identity and the session role (`AIDA_SESSION_ROLE`).
        agent: Option<String>,

        /// Max messages to list before collapsing the rest into "+N more"
        /// (default: 5). Keeps the per-turn context injection bounded.
        #[clap(long)]
        cap: Option<usize>,
    },

    /// Operator overview: agents with mail waiting + unread / urgent-unread
    /// counts, most-recent-activity first.
    List,

    /// Retract a sent message, leaving a withdrawn tombstone in mailbox views.
    // trace:STORY-583 | ai:codex
    Retract {
        /// Message id to retract.
        message_id: String,
    },

    /// Delete a sent message from mailbox views; records a marker so sync does not resurrect it.
    // trace:STORY-583 | ai:codex
    Delete {
        /// Message id to delete.
        message_id: String,
    },

    /// Show a full conversation thread, oldest-first.
    Thread {
        /// The thread id.
        thread_id: String,
    },

    /// Digest the local layer into the git-canonical store (durable, replayable,
    /// shareable across clones) and commit it on the orphan branch. Idempotent.
    Sync,
}

/// Guided origin bootstrap for a project with no git `origin`.
// trace:STORY-537 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RemoteCommand {
    /// Create (or attach) an `origin` for a project that has none. At a TTY,
    /// walks a menu: GitHub (via gh), a remembered GitLab host, another GitLab
    /// host (push-to-create over SSH), or attach-existing. Without a TTY (and
    /// no route flag) prints the manual recipe and exits cleanly. Pre-select a
    /// route with --github / --gitlab <host> / --attach <url> to stay
    /// scriptable.
    Create {
        /// Attach this existing repo URL as origin and push (skip the menu).
        #[clap(long)]
        attach: Option<String>,

        /// Create on GitHub via `gh repo create … --source . --remote origin
        /// --push` (skip the menu).
        #[clap(long, conflicts_with_all = ["attach", "gitlab"])]
        github: bool,

        /// Push-to-create on this GitLab host over SSH (skip the menu).
        #[clap(long, conflicts_with_all = ["attach", "github"], value_name = "HOST")]
        gitlab: Option<String>,

        /// Create the repo public instead of private (GitHub path).
        #[clap(long)]
        public: bool,
    },

    /// Wire an existing repo's URL as `origin` and push the current branch.
    /// The clean fallback when auto-create isn't possible (corporate GitLab):
    /// create the repo in the UI, then `aida remote attach <url>`.
    Attach {
        /// The existing repo's clone URL (SSH or HTTPS).
        url: String,
    },
}

/// Claude Code path-gated rules sync.
// trace:SPIKE-31 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RulesCommand {
    /// Reconcile `.claude/rules/aida-specs/` against the current spec
    /// graph: write a rule for every active spec with trace comments,
    /// remove rules whose spec is no longer active.
    Sync {
        /// Compute what would change without touching disk.
        #[clap(long)]
        dry_run: bool,

        // trace:SPIKE-35 | ai:claude
        /// Also emit a project-root `REVIEW.md` that
        /// aggregates every active spec's acceptance criteria as
        /// the highest-priority injection into Anthropic's managed
        /// Code Review pipeline. One file, committed.
        // trace:SPIKE-35 | ai:claude
        #[clap(long = "review-md")]
        review_md: bool,
    },
}

/// Project the graph into a layered docs tree.
// trace:FR-1-077 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DocsCommand {
    /// Render (or re-render) the docs tree under `<output>/aida/`.
    /// Default output is `docs/aida/`.
    Build {
        /// Output directory (default: `docs/aida/`)
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,

        /// Print what would be written, but don't write
        #[clap(long)]
        dry_run: bool,
    },

    /// Verify the docs tree on disk matches what the graph would render.
    /// Exits non-zero on drift. Suitable for a CI / pre-commit check.
    Check {
        /// Output directory (default: `docs/aida/`)
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,
    },

    /// Print AIDA's machinery + lifecycle glossary from the binary's
    /// embedded copy.
    ///
    /// Surfaces the canonical vocabulary (orchestrator, phase, drain, lease,
    /// role, scope, worktree, …) and the spec-lifecycle verbs (committed,
    /// pushed, merged, completed, released) without hunting for the file under
    /// docs/aida/discipline/ — and stays correct even if the project's
    /// scaffolded copy is missing or stale. Default prints both sections.
    Glossary {
        /// Print only the machinery glossary (orchestrator, drain, lease, …).
        #[clap(long)]
        machinery: bool,
        /// Print only the lifecycle vocabulary (the spec-state verbs).
        #[clap(long)]
        lifecycle: bool,
    },
}

/// Living-documentation entry surface.
///
/// `Doc` requirements are narrative captured *during* work — rationale,
/// scenarios, recipes, gotchas — linked back to the specs they explain via
/// `References` relationships. They flow through the same orphan-store /
/// cache pipeline as every other requirement; this subcommand exists to
/// make capture and lookup ergonomic so the docs land while the context
/// is still fresh.
// trace:EPIC-24, STORY-104 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DocCommand {
    /// Capture a documentation entry — narrative tied to one or more specs.
    Add {
        /// Title of the doc entry (e.g., "When to use `aida queue work --steal`").
        #[clap(long)]
        title: String,

        /// Specs this entry is about. Repeat or comma-separate. Each id is
        /// resolved against the store and linked as a `References`
        /// relationship — that's what `aida doc show <SPEC>` walks to find
        /// docs about a given spec.
        // trace:STORY-104 | ai:claude
        #[clap(long, value_delimiter = ',')]
        about: Vec<String>,

        /// Scenario tag — short label for the situation this doc covers
        /// (e.g., "muddle recovery", "first-time setup"). Stored in
        /// `custom_fields["scenario"]`; used as a filter axis by
        /// `aida doc list --scenario <name>`.
        #[clap(long)]
        scenario: Option<String>,

        /// Audience tags (repeat or comma-separate) — who this doc is for:
        /// `user`, `agent`, `developer`. Stored as a comma-joined string in
        /// `custom_fields["audience"]`.
        #[clap(long, value_delimiter = ',')]
        audience: Vec<String>,

        /// Description body. For long-form prose use --description-from-file
        /// or --description-stdin (mirrors `aida add`).
        #[clap(long, conflicts_with_all = ["description_from_file", "description_stdin"])]
        description: Option<String>,

        /// Read the description body from a file.
        #[clap(long, conflicts_with_all = ["description", "description_stdin"])]
        description_from_file: Option<PathBuf>,

        /// Read the description body from stdin.
        #[clap(long, conflicts_with_all = ["description", "description_from_file"])]
        description_stdin: bool,

        /// Tags for the entry (comma-separated).
        #[clap(long)]
        tags: Option<String>,
    },

    /// List documentation entries.
    List {
        /// Show only docs that reference <id> via `--about`. Resolves <id>
        /// against the store and walks the `References` edges back.
        #[clap(long, value_name = "ID")]
        about: Option<String>,

        /// Show only docs whose scenario tag matches exactly.
        #[clap(long)]
        scenario: Option<String>,

        /// Show only docs whose audience tag set contains <name>.
        #[clap(long)]
        audience: Option<String>,
    },

    /// Show docs. If <id> is a Doc spec id (e.g., DOC-3), print the entry's
    /// full detail. Otherwise treat <id> as a referenced spec and print
    /// every Doc that mentions it via `--about`.
    Show {
        /// Doc spec id, or any other spec id to walk `--about` references.
        id: String,
    },

    /// Release-time doc-coverage gate. List specs that reached Completed
    /// since the previous git tag but have no `aida doc` entry about them
    /// (no Doc that References them). Warn-only — exits 0 even when gaps
    /// exist — so it can be wired into the release flow without blocking.
    // trace:TASK-680 | ai:claude
    Coverage {
        /// Treat everything after this git ref/tag as "this release". When
        /// absent, the most recent `v*` tag is used; if there is no tag, the
        /// full history is scanned.
        #[clap(long, value_name = "REF")]
        since: Option<String>,

        /// Emit the gap list as JSON instead of a human warning block.
        #[clap(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum DbCommand {
    /// Print the path to the active store (the orphan-branch worktree, in
    /// distributed mode). The legacy `--name` registry lookup was dropped
    /// in Phase 1 of the kernel/module audit.
    // trace:FR-1-076 | ai:claude
    Path,

    /// (archived) Migrate between legacy YAML/SQLite/PostgreSQL backends.
    /// Kept for one-shot migrations off the legacy pre-git-canonical path.
    // trace:FR-1-076 | ai:claude
    // trace:EPIC-1-001 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(hide = true)]
    Migrate {
        /// Source format: "yaml", "sqlite", or "postgres"
        #[clap(long)]
        from: String,

        /// Target format: "yaml", "sqlite", or "postgres"
        #[clap(long)]
        to: String,

        /// Output path (for yaml/sqlite) or connection string (for postgres)
        #[clap(long, short = 'o')]
        output: Option<String>,

        /// Overwrite existing target file
        #[clap(long)]
        force: bool,
    },

    /// Show database statistics and info
    Info,

    /// Sync git-backed store with remote (push/pull)
    Sync {
        /// Pull from remote before pushing
        #[clap(long)]
        pull: bool,

        /// Push to remote after committing
        #[clap(long)]
        push: bool,

        /// Commit message for pending changes
        #[clap(long, short = 'm')]
        message: Option<String>,
    },

    /// (archived) Export legacy DB to a git-backed store directory.
    // trace:FR-1-076 | ai:claude
    #[clap(hide = true)]
    ExportGit {
        /// Output directory for the git-backed store
        #[clap(long, short = 'o')]
        output: String,
    },

    /// Assign agreed IDs (short IDs) to all objects that don't have one.
    /// Run this at merge-to-trunk to collapse a node-aware distributed id
    /// (e.g. `FR-N-NNN`) down to its short form (`FR-NNN`).
    // trace:TASK-487 | ai:claude
    MergeGate,

    /// Collapse legacy origin spec_ids onto their agreed_ids. For each
    /// requirement where spec_id ≠ agreed_id (typically a legacy
    /// zero-padded origin id with a later-assigned short agreed id), set
    /// spec_id := agreed_id and clear agreed_id. The on-disk YAML moves
    /// to the new sharded path; relationships are unaffected (they use
    /// UUIDs). Run with --dry-run first to preview.
    // trace:FR-1-071 | ai:claude
    // trace:TASK-487 | ai:claude
    RetireLegacyIds {
        /// Show what would change without writing anything.
        #[clap(long)]
        dry_run: bool,
    },

    /// Initialize a multi-repo workspace (multiple code repos sharing one store)
    WorkspaceInit {
        /// Workspace name
        #[clap(long)]
        name: Option<String>,

        /// Git remote URL for the shared store
        #[clap(long)]
        remote: Option<String>,
    },

    /// Show status of the git-backed store (changes, sync state, conflicts)
    Status,

    /// Manage pre-allocated agreed ID blocks for offline-safe trace comments
    // trace:FR-2-005 | ai:claude
    // trace:TASK-487 | ai:claude
    Block {
        #[clap(subcommand)]
        subcommand: BlockCommand,
    },

    /// Replay the Done → Completed auto-bump scan over a wider commit
    /// range than `aida pull` saw, recovering specs that got stranded
    /// at Done. The pull-time auto-bump only scans commits the current
    /// pull brings in; if a spec's YAML was unreadable at pull time, or
    /// the user pulled before flipping the spec to Done, the bump
    /// silently misses and the spec is stuck without manual intervention.
    // trace:TASK-226 | ai:claude
    // trace:STORY-86 | ai:claude
    // trace:BUG-96 | ai:claude
    // trace:TASK-487 | ai:claude
    ReconcileStatus {
        /// Bound the scan range. Accepts a commit SHA, a tag, or any
        /// `git rev-parse`-able ref. Without --since, walks the most
        /// recent 200 commits on the default branch (an over-broad
        /// window is safe — only Done specs flip, so idempotent on
        /// already-Completed specs).
        #[clap(long, value_name = "REF")]
        since: Option<String>,

        /// Limit the replay to a single spec. Faster than the full
        /// scan when you know which spec is stuck. The same default-
        /// branch + status-Done guards apply.
        #[clap(long, value_name = "SPEC-ID")]
        spec: Option<String>,

        /// Show what would flip without writing anything. Pairs with
        /// `--since` / `--spec` for previewing a targeted replay.
        #[clap(long)]
        dry_run: bool,
    },

    /// Audit the store for consistency problems. Currently supports
    /// `--collisions` (two requirements claiming the same short id,
    /// which the gate-time check prevents going forward but doesn't
    /// retroactively detect).
    // trace:TASK-80 | ai:claude
    // trace:BUG-82 | ai:claude
    // trace:TASK-487 | ai:claude
    Check {
        /// Report requirements whose preferred display id (agreed_id
        /// when assigned, else spec_id) collides with another
        /// requirement's spec_id or agreed_id. Pre-existing collisions
        /// from before the gate-time check landed persist in the store
        /// and won't auto-clear.
        #[clap(long)]
        collisions: bool,

        /// When used with `--collisions`, re-gate the later (higher
        /// position-encoded) claimant's agreed_id to the next free
        /// short id. The earlier claimant keeps the contested id.
        /// Without this flag, the command only reports; the operator
        /// decides which to keep.
        #[clap(long)]
        repair: bool,
    },
}

// Team RBAC: manage durable per-user roles in the shared roster
// (`registry/team.toml`). trace:STORY-646 | ai:claude
#[derive(Subcommand, Debug)]
pub enum TeamCommand {
    /// Set a user's durable team role in the roster (`registry/team.toml`,
    /// CAS push-wins). A rostered role is the user's effective role even with
    /// no `AIDA_SESSION_ROLE` set, so the advisor guardrail survives a
    /// forgotten env var.
    ///
    /// GUARDRAIL, NOT SECURITY: the store is a shared git branch, so anyone
    /// with push access can edit any spec directly with raw git regardless of
    /// their role. This roster prevents *accidents* (an implementer
    /// accidentally approving a spec) and records team structure + an audit
    /// trail — it is NOT an access-control boundary.
    // trace:STORY-646 | ai:claude
    SetRole {
        /// The user id (the person — matches `current_user_id`: `$AIDA_USER`
        /// / `$USER`). Not a node id.
        user: String,
        /// The role to grant (e.g. `advisor`, `implementer`). Validated
        /// against the known role set.
        #[clap(long)]
        role: String,
    },

    /// Show YOUR effective role: the roster role for your user id if present,
    /// else `AIDA_SESSION_ROLE`, else the default. Says where it came from.
    // trace:STORY-646 | ai:claude
    MyRole {
        /// Machine-readable JSON output.
        #[clap(long)]
        json: bool,
    },

    /// Remove a user's entry from the roster (`registry/team.toml`, CAS push).
    /// A friendly no-op if the user isn't present. Use this to clean stray /
    /// duplicate keys — e.g. an orphaned integer key left by an older roster.
    // trace:STORY-654 | ai:claude
    UnsetRole {
        /// The user id to remove (the person key — matches `current_user_id`,
        /// `$AIDA_USER` / `$USER`; may also be a stray integer key).
        user: String,
    },
}

/// Manage the shared person-alias registry (`registry/aliases.toml` on the
/// `aida-store` branch). One human routinely registers under several identity
/// strings across machines (`joe`, `joe.mooney`, `joe.mooney@gmail.com`); each
/// host mints its own ids, so the queue, the team roster, and the block list
/// otherwise show one person as several owners. Linking the strings collapses
/// them to one canonical person at every comparison/display boundary.
///
/// Composes with the case-fold (`Joe` vs `joe` already merge): the alias map is
/// the SECOND normalization layer, for the genuinely-different strings.
// trace:TASK-845 | ai:claude
#[derive(Subcommand, Debug)]
pub enum IdentityCommand {
    /// Link two identity strings as the same canonical person (bidirectional,
    /// idempotent). Writes `registry/aliases.toml` with a CAS push-wins loop.
    /// After linking, all the surfaces resolve both strings to one person.
    // trace:TASK-845 | ai:claude
    Link {
        /// The first identity string (e.g. `joe`).
        a: String,
        /// The second identity string (e.g. `joe.mooney@gmail.com`).
        b: String,
    },

    /// List the recorded person links: one block per canonical person with the
    /// aliases that resolve to them.
    // trace:TASK-845 | ai:claude
    List {
        /// Machine-readable JSON output.
        #[clap(long)]
        json: bool,
    },

    /// Show the canonical person an identity string resolves to, plus all the
    /// aliases that share it (after case-fold + alias-resolve).
    // trace:TASK-845 | ai:claude
    Show {
        /// The identity string to resolve (e.g. `Joe.Mooney@work.example`).
        id: String,
        /// Machine-readable JSON output.
        #[clap(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum NodeCommand {
    /// List all nodes registered in the shared registry. The current node
    /// (the one whose `.aida/node.toml` matches an entry) is marked with `*`.
    List,

    /// Show details for a single node. With no id, defaults to the current
    /// node (read from `.aida/node.toml`).
    Show {
        /// Node id to show (omit for current node). Accepted as a string.
        // trace:EPIC-9 | ai:claude
        // trace:TASK-487 | ai:claude
        id: Option<String>,
    },

    /// Acquire a node id for this clone. Performs a CAS push loop on the
    /// shared registry to claim the next sequential id, then writes the
    /// per-clone identity file at `.aida-store/.aida/node.toml`.
    /// Defaults pull `git config user.email` for the email stamp and the
    /// system hostname for the hostname stamp.
    // trace:EPIC-1-052 | ai:claude
    // trace:STORY-41 | ai:claude
    Acquire {
        /// Claim a specific node id (must be free in the registry).
        /// Accepted as a string — `"JM"` or `"1"` alike.
        // trace:EPIC-9 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        id: Option<String>,

        /// Override the hostname stamp (default: system hostname)
        #[clap(long)]
        hostname: Option<String>,

        /// Override the email stamp (default: `git config user.email`)
        #[clap(long)]
        email: Option<String>,

        /// Friendly name for this node, recorded alongside the node id in the
        /// shared roster (`aida team`). Defaults to `<host>-<user>-<seq>` (e.g.
        /// `imac-joe-1`). At a TTY with no flag you are prompted with the
        /// default pre-filled; non-interactive use takes the default silently.
        // trace:STORY-652 | ai:claude
        #[clap(long, value_name = "NAME")]
        node_name: Option<String>,

        /// Re-acquire even if `.aida/node.toml` already exists (default: refuse)
        #[clap(long)]
        force: bool,

        /// On collision, accept the suggested numeric-suffix fallback without
        /// prompting (e.g., `JM` taken → silently use `JM2`). Implies non-
        /// interactive mode.
        // trace:STORY-42 | ai:claude
        #[clap(long)]
        yes: bool,

        /// Re-claim an already-registered node id (the shared registry
        /// entry gets re-attributed to this clone). When the previous
        /// clone is reachable on this host, drops a HIJACKED.toml marker
        /// inside its `.aida-store/.aida/` so the user sees a warning the
        /// next time they run `aida` there. Mutually exclusive with
        /// `--id`.
        // trace:STORY-43 | ai:claude
        #[clap(long, value_name = "ID", conflicts_with = "id")]
        hijack: Option<String>,

        /// Backfill a node entry into the shared registry for some OTHER
        /// (typically legacy) clone, then push — WITHOUT touching this
        /// clone's own identity. The running clone keeps its `.aida/node.toml`
        /// untouched and no blocks are allocated. Requires explicit `--id`,
        /// `--hostname`, and `--email` (nothing is inferred from the local
        /// environment, since the entry is not about this clone). Mutually
        /// exclusive with `--hijack` and `--force`.
        // trace:FR-265 | ai:claude
        #[clap(long, conflicts_with_all = ["hijack", "force"])]
        remote_only: bool,
    },

    /// Backfill the owner ($USER string) on an existing node entry in the
    /// shared registry, then push (CAS). If the id is the CURRENT node, the
    /// local `.aida/node.toml` is updated too. Errors clearly if the id is
    /// absent. For legacy nodes that predate the owner/name identity fields.
    // trace:STORY-654 | ai:claude
    SetOwner {
        /// Node id whose owner to set.
        id: String,
        /// The owner's `current_user_id` string ($USER / $AIDA_USER).
        #[clap(long)]
        user: String,
    },

    /// Backfill the friendly name on an existing node entry in the shared
    /// registry, then push (CAS). If the id is the CURRENT node, the local
    /// `.aida/node.toml` is updated too. Errors clearly if the id is absent.
    // trace:STORY-654 | ai:claude
    SetName {
        /// Node id whose name to set.
        id: String,
        /// The friendly node name (e.g. `imac-joe-1`).
        #[clap(long)]
        name: String,
    },

    /// Remove a node entry from the shared registry. Does not invalidate
    /// any IDs already issued by that node. The node id is not reused.
    Release {
        /// Node id to release
        id: String,

        /// Skip the confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// List the team: every registered node/clone sharing this store, with
    /// each member's role. Identity introspection under the `node` namespace.
    /// Equivalent to the top-level `aida team` (kept visible because the
    /// multi-clone harness invokes it); with a subcommand, manage per-user
    /// roles (`set-role`, `my-role`).
    // trace:TASK-851 | ai:claude
    Team {
        /// Machine-readable JSON output (bare roster view only).
        #[clap(long)]
        json: bool,
        #[clap(subcommand)]
        cmd: Option<TeamCommand>,
    },

    /// Print the caller identity AIDA resolved — role, agent-type, agent-name,
    /// user-id, headless flag, ai-tool, and active session/scope. Read-only.
    /// Identity introspection under the `node` namespace; equivalent to the
    /// hidden top-level `aida whoami`.
    // trace:TASK-851 | ai:claude
    Whoami,
}

/// Subcommands under `aida presence` — the canonical surface for the operator
/// presence state (collapsing the old top-level `away`/`home`/`presence` verbs
/// into one). Bare `aida presence` shows status. The top-level `aida away` /
/// `aida home` remain as hidden aliases for muscle memory.
// trace:TASK-851 | ai:claude
#[derive(Subcommand, Debug)]
pub enum PresenceCommand {
    /// Mark yourself away from the keyboard (sets the machine-global presence
    /// state with a TTL). Same as the hidden top-level `aida away`.
    // trace:TASK-851 | ai:claude
    Away,
    /// Mark yourself back at the keyboard (clears any away state). Same as the
    /// hidden top-level `aida home`.
    // trace:TASK-851 | ai:claude
    Home,
    /// Show current effective presence: home/away, how long ago it was set,
    /// and TTL-remaining when away. Same as bare `aida presence`.
    // trace:TASK-851 | ai:claude
    Status,
}

#[derive(Subcommand, Debug)]
pub enum BlockCommand {
    /// Claim a new block of agreed IDs for this node (requires network push)
    Claim {
        /// Type prefix to claim a block for (e.g., FR, BUG, EPIC). Defaults to FR.
        #[clap(long, default_value = "FR")]
        r#type: String,

        /// Number of IDs to reserve in the block
        #[clap(long, default_value = "100")]
        size: u32,
    },

    /// List all claimed blocks across all nodes
    List,

    /// Show blocks for the current node and their remaining capacity
    Status,

    /// Cross-check nodes.toml against blocks.yaml. Reports any block-
    /// owning node id missing from the registry and (under blocks-only
    /// policy) any registered node with no claimed block. Exits non-zero
    /// on inconsistency.
    // trace:FR-281 | ai:claude
    Verify,
}

/// Operations on the orphan-store SHA pin (the `Aida-Store: <sha>`
/// trailer in every code commit).
// trace:EPIC-21 | ai:claude
#[derive(Subcommand, Debug)]
pub enum StoreCommand {
    /// Show the relationship between the current code commit and the
    /// orphan store: which store SHA the code was paired with at commit
    /// time, what the store HEAD is now, drift in commits between them.
    Status,

    /// Install the prepare-commit-msg hook (`aida-store-pair.sh`) into
    /// `.git/hooks/`. Idempotent — safe to re-run. New projects get
    /// this hook from `aida init` automatically; this command exists
    /// for retrofitting an existing project.
    InstallHook {
        /// Overwrite an existing prepare-commit-msg hook (default: refuse).
        #[clap(long)]
        force: bool,
    },

    /// Deep-repack the orphan store git repo to relieve the substrate tax of a
    /// long, never-compacted history. The bare command runs an aggressive
    /// `git gc` — safe + non-destructive: no history is rewritten, `aida
    /// history` is unaffected, no force-push needed. `gc` is an alias.
    // trace:STORY-733 | ai:claude
    #[clap(alias = "gc")]
    Compact {
        /// DESTRUCTIVE opt-in: rewrite the orphan-store history to a single
        /// snapshot commit. Rewrites the branch (breaks the full `aida
        /// history --events` timeline — preserved only via the backup ref) and
        /// needs a coordinated force-push every clone must re-sync. Never runs
        /// automatically. Without `--yes` it only PRINTS the plan.
        #[clap(long)]
        squash: bool,

        /// Required to actually perform a `--squash` rewrite. Without it,
        /// `--squash` prints exactly what it would do and exits without
        /// touching the store.
        #[clap(long)]
        yes: bool,
    },
}

/// Maintenance + migration ops on the AIDA store.
// trace:EPIC-19 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DoctorCommand {
    /// Focused multi-agent drift diagnostic for one category.
    // trace:STORY-462 | ai:codex
    Check {
        /// Category to check, e.g. stale-leases, orphan-branches,
        /// stale-remote-branches, OBE-briefs.
        category: String,

        /// Show older completed-without-commit findings individually.
        #[clap(long)]
        all: bool,

        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },

    /// Focused multi-agent drift heal for one category.
    // trace:STORY-462 | ai:codex
    Heal {
        /// Category to heal, e.g. stale-leases, orphan-branches,
        /// stale-remote-branches, OBE-briefs.
        category: String,

        /// Skip confirmation prompts.
        #[clap(long, short = 'y')]
        yes: bool,

        /// Permit riskier destructive fixes such as branch deletion.
        #[clap(long)]
        force: bool,

        /// Show older completed-without-commit findings individually.
        #[clap(long)]
        all: bool,

        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },

    /// Migrate this project from per-type counters (each type counts
    /// independently — every type starts back at 1) to a single global
    /// counter (one shared sequence — IDs increment across types).
    /// Allocates a new `*` block above the existing per-type blocks'
    /// max range_end, marks per-type blocks as exhausted, sets
    /// `[id_format] counter_scope = "global"`. Existing requirement
    /// spec_ids stay unchanged — only newly-added reqs use the global
    /// counter.
    // trace:EPIC-19, FR-271 | ai:claude
    // trace:TASK-487 | ai:claude
    MigrateCounterScope {
        /// Target scope. Today only `global` is supported. (Going from
        /// global back to per-type is harder — no migration path yet.)
        #[clap(long, value_parser = ["global"])]
        to: String,

        /// Print the planned changes without writing anything.
        #[clap(long)]
        dry_run: bool,

        /// Skip the y/N confirmation.
        #[clap(long, short = 'y')]
        yes: bool,

        /// Block size for the new shared block (default 1000, matching
        /// `aida init` Global default).
        #[clap(long, default_value = "1000")]
        size: u32,
    },

    /// Find and tombstone blocks whose `node_id` isn't in nodes.toml
    /// (orphaned from a clone that never registered, or from a
    /// pre-multi-node store). The block range stays reserved so other
    /// clones don't reallocate over it; `next` is bumped past `range_end`
    /// so the dispenser skips it.
    // trace:EPIC-19, EPIC-1-052 | ai:claude
    // trace:TASK-487 | ai:claude
    RepairStaleBlocks {
        /// Print what would change without writing.
        #[clap(long)]
        dry_run: bool,
        /// Skip confirmation.
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Detect duplicate spec_ids in the orphan store — multiple YAMLs
    /// claiming the same id (legacy leftovers, imports gone wrong).
    /// Reports only; v1 doesn't auto-renumber because that would orphan
    /// trace comments and commit refs.
    // trace:EPIC-19, BUG-31 | ai:claude
    // trace:TASK-487 | ai:claude
    ScrubCollisions,

    /// Walk every requirement's `relationships` array and verify each
    /// `target_id` resolves to an existing requirement UUID. Catches
    /// dangling references from deleted reqs, bad imports, or
    /// hand-edits. With --repair, strips dangling entries.
    // trace:EPIC-19 | ai:claude
    // trace:TASK-956 | ai:claude — hidden from `doctor --help` (still runs).
    #[clap(hide = true)]
    VerifyRelationships {
        /// Strip dangling references in-place. Without this flag, the
        /// command reports only.
        #[clap(long)]
        repair: bool,
        /// Skip the y/N confirmation when --repair would write.
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Walk source files under the project root for AIDA trace annotations
    /// and verify each spec_id resolves to an existing requirement.
    /// Catches dead trace comments left behind after a req got deleted, or
    /// simple typos. Default is read-only; pass `--strip-dangling` to
    /// remove markers pointing at unknown spec_ids (whole comment line
    /// deleted if the trace was its only content; otherwise just the
    /// trace fragment is excised).
    // trace:EPIC-19 | ai:claude
    ValidateTraceComments {
        /// Rewrite source files to remove dangling AIDA trace annotations.
        /// Lossy — comments around the marker are preserved, but the
        /// pointer itself is gone.
        // trace:EPIC-19 | ai:claude
        #[clap(long)]
        strip_dangling: bool,
        /// Print what would change without writing.
        #[clap(long)]
        dry_run: bool,
        /// Skip the y/N confirmation when --strip-dangling would write.
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Run every diagnostic in sequence and print a unified report.
    /// Composes `aida db block verify`, repair-stale-blocks --dry-run,
    /// scrub-collisions, verify-relationships, validate-trace-comments,
    /// plus a few smaller checks. Exits non-zero if any check found a
    /// problem.
    // trace:EPIC-19 | ai:claude
    // trace:TASK-956 | ai:claude — folded into the `doctor` surface: hidden
    // from `doctor --help` (the canonical drift verb is `doctor check`), but
    // the `fsck` name still runs its own full-suite integrity report. It is
    // NOT a true alias of `doctor check` — `check` is the per-category
    // multi-agent diagnostic and requires a category arg, whereas `fsck` runs
    // the whole legacy suite with no arg, so collapsing it would drop a
    // capability. Hiding preserves the name + behavior, off `--help`.
    #[clap(hide = true)]
    Fsck,

    /// Check that STORY / BUG descriptions contain a recognized
    /// acceptance heading (`## Acceptance`, `## Verify`, `## Tests`,
    /// `## Test cases`, `## Verification` — case-insensitive). Surfaced
    /// by the review-prompt generator: missing sections produce a
    /// placeholder downstream, this lint catches them at write-time.
    /// Exits non-zero if any STORY/BUG is missing a section.
    // trace:STORY-70, STORY-67 | ai:claude
    // trace:TASK-487 | ai:claude
    ConventionCheck {
        /// Quiet mode — print only the summary line, omit per-spec rows.
        #[clap(long, short = 'q')]
        quiet: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum FeatureCommand {
    /// Add a new feature with a prefix for IDs
    Add {
        /// Name of the feature (e.g., "Authentication")
        #[clap(long)]
        name: Option<String>,

        /// Prefix for requirement IDs (e.g., "AUTH")
        #[clap(long)]
        prefix: Option<String>,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },

    /// List all features
    List,

    /// Show details for a specific feature
    Show {
        /// The name or prefix of the feature to show
        name: String,
    },

    /// Edit an existing feature
    Edit {
        /// The name or prefix of the feature to edit
        name: String,

        /// New name for the feature
        #[clap(long)]
        new_name: Option<String>,

        /// New prefix for the feature
        #[clap(long)]
        new_prefix: Option<String>,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Show current ID configuration
    Show {
        /// Optional section to show, e.g. `store.sync`.
        // trace:STORY-284 | ai:codex
        section: Option<String>,
    },

    /// Set the ID format (single-level or two-level)
    Format {
        /// Format: "single" for PREFIX-NNN, "two" for FEATURE-TYPE-NNN
        format: String,
    },

    /// Set the numbering strategy
    Numbering {
        /// Strategy: "global", "per-prefix", or "per-feature-type"
        strategy: String,
    },

    /// Set the number of digits in IDs
    Digits {
        /// Number of digits (1-6)
        digits: u8,
    },

    /// Migrate existing SPEC-XXX IDs to new format
    Migrate {
        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Show or update user-level preferences at `~/.aida/preferences.toml`.
    /// Currently stores: preferred node id (used as default by `aida init`)
    /// and a fallback email (used when `git config user.email` is unset).
    // trace:STORY-44 | ai:claude
    User {
        /// Set the preferred node id (validated as `[A-Za-z0-9][A-Za-z0-9_-]*`,
        /// 1-32 chars). Pass empty string to clear.
        #[clap(long)]
        node_id: Option<String>,

        /// Set the fallback email. Pass empty string to clear.
        #[clap(long)]
        email: Option<String>,

        /// Print current preferences as TOML to stdout (machine-readable).
        #[clap(long)]
        toml: bool,
    },

    /// Show or toggle workflow hints printed inline at state-transition
    /// moments (queue drained → open PR, session end → start review).
    /// No-arg form prints the current setting; pass `true` / `false` to
    /// persist into `.aida/config.toml [hints] workflow_hints`. The env
    /// var `AIDA_HINTS=false` overrides the config for the current shell
    /// without writing to disk.
    // trace:STORY-106 | ai:claude
    Hints {
        /// `true` to enable, `false` to disable. Omit to show the
        /// current effective value (env-aware).
        enabled: Option<String>,
    },

    /// Inspect and customize the CLI glyph set — list the symbols, apply a
    /// canned theme, or override an individual glyph. Writes to
    /// `.aida/config.toml` (or `~/.aida/config.toml` with `--user`),
    /// preserving the rest of the file.
    // trace:STORY-633 | ai:claude
    #[clap(subcommand)]
    Glyph(GlyphCommand),

    /// Browse every configurable item in a navigable TUI: per row the knob's
    /// name, current value, built-in default, where it was set (scope), and a
    /// one-line explanation. The visual companion to `config show` — same
    /// resolved surface, one screen, arrow-key navigation. Read-only for now
    /// (edit knobs with the matching `config` subcommand or your config.toml).
    /// Needs a TTY; without one it points you at `config show`.
    // trace:STORY-661 | ai:claude
    Menu,
}

/// `aida config glyph ...` — CLI surface over the glyph registry, themes, and
/// the per-symbol override table.
// trace:STORY-633 | ai:claude (EPIC-45 phase 4)
#[derive(Subcommand, Debug)]
pub enum GlyphCommand {
    /// List every glyph: name, currently-resolved rendering (honoring the
    /// active profile + theme + overrides), and the unicode registry default.
    List,

    /// Inspect or apply glyph themes. With no NAME (or `theme list`), lists the
    /// embedded themes each with a one-line preview row. With a NAME, applies
    /// that theme — writes `[ui] theme = "<name>"` (a clean reference).
    Theme {
        /// Theme name to apply. Omit (or pass `list`) to list available themes.
        name: Option<String>,
        /// Materialize the theme's bundle into `[glyphs]` for hand-tweaking
        /// instead of writing the named reference.
        #[clap(long)]
        expand: bool,
        /// Write to `~/.aida/config.toml` instead of the project config.
        #[clap(long)]
        user: bool,
    },

    /// Override an individual glyph: writes `[glyphs] <name> = "<value>"`.
    Set {
        /// Glyph name (see `aida config glyph list`).
        name: String,
        /// Replacement string to render for this glyph.
        value: String,
        /// Write to `~/.aida/config.toml` instead of the project config.
        #[clap(long)]
        user: bool,
    },

    /// Drop a single per-symbol override.
    Unset {
        /// Glyph name whose override to remove.
        name: String,
        /// Operate on `~/.aida/config.toml` instead of the project config.
        #[clap(long)]
        user: bool,
    },

    /// Clear ALL per-symbol overrides (the whole `[glyphs]` table).
    Reset {
        /// Operate on `~/.aida/config.toml` instead of the project config.
        #[clap(long)]
        user: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum TypeCommand {
    /// List all requirement types
    List,

    /// Add a new requirement type
    Add {
        /// Name of the type (e.g., "Business")
        #[clap(long)]
        name: String,

        /// Prefix for the type (e.g., "BR")
        #[clap(long)]
        prefix: String,

        /// Description of the type
        #[clap(long)]
        description: Option<String>,
    },

    /// Remove a requirement type
    Remove {
        /// Name or prefix of the type to remove
        name: String,

        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },
}

/// Commands for managing relationship type definitions
#[derive(Subcommand, Debug)]
pub enum RelDefCommand {
    /// List all relationship definitions
    List,

    /// Show details for a specific relationship definition
    Show {
        /// Name of the relationship definition
        name: String,
    },

    /// Add a new relationship definition
    Add {
        /// Unique name for the relationship (lowercase, no spaces)
        #[clap(long)]
        name: String,

        /// Human-readable display name
        #[clap(long)]
        display_name: Option<String>,

        /// Description of what this relationship means
        #[clap(long)]
        description: Option<String>,

        /// Name of the inverse relationship (e.g., "child" for "parent")
        #[clap(long)]
        inverse: Option<String>,

        /// Whether this relationship is symmetric (A->B implies B->A)
        #[clap(long)]
        symmetric: bool,

        /// Cardinality: 1:1, 1:n, n:1, n:n (default: n:n)
        #[clap(long, default_value = "n:n")]
        cardinality: String,

        /// Allowed source requirement types (comma-separated, empty = all)
        #[clap(long)]
        source_types: Option<String>,

        /// Allowed target requirement types (comma-separated, empty = all)
        #[clap(long)]
        target_types: Option<String>,

        /// Color for visualization (hex format, e.g., #ff6b6b)
        #[clap(long)]
        color: Option<String>,
    },

    /// Edit an existing relationship definition
    Edit {
        /// Name of the relationship definition to edit
        name: String,

        /// New display name
        #[clap(long)]
        display_name: Option<String>,

        /// New description
        #[clap(long)]
        description: Option<String>,

        /// New allowed source types (comma-separated)
        #[clap(long)]
        source_types: Option<String>,

        /// New allowed target types (comma-separated)
        #[clap(long)]
        target_types: Option<String>,

        /// New color
        #[clap(long)]
        color: Option<String>,
    },

    /// Remove a relationship definition (only custom ones)
    Remove {
        /// Name of the relationship definition to remove
        name: String,

        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum RelationshipCommand {
    /// Add a relationship between requirements
    ///
    /// Both positional and flag forms work:
    ///   aida rel add <from> <to> --type child
    ///   aida rel add --from <from> --to <to> --type child
    // trace:TASK-487 | ai:claude
    Add {
        /// Source requirement ID (UUID or SPEC-ID). Positional or via --from.
        #[clap(value_name = "FROM")]
        from_pos: Option<String>,

        /// Target requirement ID (UUID or SPEC-ID). Positional or via --to.
        #[clap(value_name = "TO")]
        to_pos: Option<String>,

        /// Source requirement ID (UUID or SPEC-ID). Alias for the FROM positional.
        #[clap(long = "from", value_name = "FROM", hide = true)]
        from_flag: Option<String>,

        /// Target requirement ID (UUID or SPEC-ID). Alias for the TO positional.
        #[clap(long = "to", value_name = "TO", hide = true)]
        to_flag: Option<String>,

        /// Relationship type (parent, child, duplicate, verifies, verified-by, references, or custom)
        #[clap(long = "type", short = 't')]
        r#type: String,

        /// Create bidirectional relationship (adds inverse relationship automatically)
        #[clap(long, short = 'b')]
        bidirectional: bool,

        /// Override the guard that refuses `--type child` when the
        /// target (the parent) is in a terminal status
        /// (Completed/Rejected). Use when intentionally backfilling a
        /// forgotten child onto a closed epic.
        // trace:BUG-64 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        force_parent: bool,
    },

    /// Remove a relationship between requirements
    ///
    /// Both positional and flag forms work:
    ///   aida rel remove <from> <to> --type child
    ///   aida rel remove --from <from> --to <to> --type child
    // trace:TASK-487 | ai:claude
    Remove {
        /// Source requirement ID (UUID or SPEC-ID). Positional or via --from.
        #[clap(value_name = "FROM")]
        from_pos: Option<String>,

        /// Target requirement ID (UUID or SPEC-ID). Positional or via --to.
        #[clap(value_name = "TO")]
        to_pos: Option<String>,

        /// Source requirement ID (UUID or SPEC-ID). Alias for the FROM positional.
        #[clap(long = "from", value_name = "FROM", hide = true)]
        from_flag: Option<String>,

        /// Target requirement ID (UUID or SPEC-ID). Alias for the TO positional.
        #[clap(long = "to", value_name = "TO", hide = true)]
        to_flag: Option<String>,

        /// Relationship type
        #[clap(long = "type", short = 't')]
        r#type: String,

        /// Remove bidirectional relationship (removes inverse relationship too)
        #[clap(long, short = 'b')]
        bidirectional: bool,
    },

    /// List relationships — global by default, or filtered to a specific
    /// source/target requirement.
    ///
    /// Examples:
    ///   aida rel list                      # all edges in the graph
    ///   aida rel list <spec-id>            # outgoing edges from a spec
    ///   aida rel list --source <spec-id>   # same as positional form
    ///   aida rel list --spec <spec-id>     # alias of --source
    ///   aida rel list --id <spec-id>       # alias of --source
    ///   aida rel list --target <spec-id>   # incoming edges (what points AT it)
    ///   aida rel list --type child         # filter by edge type
    ///   aida rel list --dangling           # only edges with unresolved targets
    ///   aida rel list --type child --dangling   # composable
    // trace:TASK-65 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(visible_alias = "show")]
    List {
        /// Source requirement ID (UUID or SPEC-ID). Positional alias of
        /// --source. When omitted (and --target/--source unset), lists every
        /// edge in the graph.
        #[clap(value_name = "ID")]
        id: Option<String>,

        /// Source requirement — same as the positional. Explicit form for
        /// scripts that want to be unambiguous. `--spec` and `--id` are
        /// accepted as aliases so the dominant ID-flag convention (used by
        /// `db reconcile-status --spec`, etc.) Just Works here too.
        // trace:BUG-577 | ai:claude
        #[clap(long, visible_alias = "spec", visible_alias = "id")]
        source: Option<String>,

        /// Target requirement — inverts the query to "what edges point AT
        /// this requirement?". Useful for "who depends on this epic?".
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        target: Option<String>,

        /// Filter by relationship type (parent, child, duplicate, verifies,
        /// verified-by, references, or a custom name).
        #[clap(long = "type", short = 't')]
        r#type: Option<String>,

        /// Only show edges whose target UUID doesn't resolve (deleted-target
        /// tombstones from removed reqs). Pairs with `aida doctor
        /// verify-relationships --repair` to clean them up.
        // trace:BUG-53 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        dangling: bool,

        /// Include edges between terminal-status (Completed/Rejected)
        /// requirements. By default the global listing hides them so the
        /// view focuses on actionable work, matching `aida list`.
        #[clap(long)]
        all: bool,

        /// Cap the number of edges shown (0 = no cap). When unset, the
        /// unfiltered global listing auto-caps to avoid a firehose on large
        /// stores and prints a note; a source/target/type/dangling filter
        /// lifts the auto-cap. Pass an explicit `--limit` to override either way.
        // trace:TASK-778
        #[clap(long)]
        limit: Option<usize>,
    },
}

#[derive(Subcommand, Debug)]
pub enum CommentCommand {
    /// Add a comment to a requirement
    Add {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,

        /// Comment content. Prefer the positional `[CONTENT]`; this flag is a
        /// hidden backward-compat alias kept so existing `--content` scripts
        /// keep working.
        // trace:TASK-778 — de-duplicated from the positional [CONTENT];
        // hidden from --help so the two forms don't read as distinct args.
        #[clap(long, hide = true)]
        content: Option<String>,

        /// Comment content (positional argument)
        #[clap(name = "CONTENT")]
        content_positional: Option<String>,

        /// Author of the comment (defaults to AIDA_AUTHOR env var or system user)
        #[clap(long)]
        author: Option<String>,

        /// Parent comment ID (for replies)
        #[clap(long)]
        parent: Option<String>,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },

    /// List all comments for a requirement
    List {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
    },

    /// Edit a comment
    Edit {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req_id: String,

        /// Comment ID to edit
        #[clap(long)]
        comment_id: String,

        /// New content
        #[clap(long)]
        content: Option<String>,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },

    /// Delete a comment
    Delete {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req_id: String,

        /// Comment ID to delete
        #[clap(long)]
        comment_id: String,
    },
}

/// GitLab integration commands
#[derive(Subcommand, Debug)]
pub enum GitLabCommand {
    /// Configure GitLab connection
    Config {
        /// GitLab instance URL (e.g., https://gitlab.com or self-hosted)
        #[clap(long)]
        url: Option<String>,

        /// GitLab project ID (numeric)
        #[clap(long)]
        project: Option<u64>,

        /// Personal Access Token (will be stored securely)
        #[clap(long)]
        token: Option<String>,

        /// Show current configuration
        #[clap(long)]
        show: bool,
    },

    /// Test connection to GitLab
    Test,

    /// List issues from GitLab
    List {
        /// Filter by state (opened, closed, all)
        #[clap(long, default_value = "opened")]
        state: String,

        /// Filter by labels (comma-separated)
        #[clap(long)]
        labels: Option<String>,

        /// Search query
        #[clap(long)]
        search: Option<String>,

        /// Maximum number of issues to show
        #[clap(long, default_value = "20")]
        limit: u32,
    },

    /// Show a specific GitLab issue
    Show {
        /// Issue IID (e.g., 123 or GL-123)
        iid: String,
    },

    /// Show sync status for linked items
    Status {
        /// Requirement ID to check (optional, shows all if not specified)
        id: Option<String>,

        /// Only show diverged items
        #[clap(long)]
        diverged: bool,
    },

    /// Manage GitLab label mappings
    Labels {
        /// Validate that mapped labels exist in GitLab project
        #[clap(long)]
        validate: bool,

        /// Create missing labels in GitLab project
        #[clap(long)]
        create_missing: bool,

        /// Initialize label mappings with defaults
        #[clap(long)]
        init: bool,
    },

    /// Refresh sync state by checking GitLab for changes
    Refresh {
        /// Specific requirement ID to refresh (optional, refreshes all if not specified)
        id: Option<String>,

        /// Force refresh even if recently checked
        #[clap(long)]
        force: bool,
    },

    /// Control background polling for GitLab changes
    Poll {
        /// Action: status, start, stop
        #[clap(default_value = "status")]
        action: String,

        /// Poll interval in seconds (for start)
        #[clap(long, default_value = "300")]
        interval: u64,
    },
}

/// Curate Approved-but-not-queued work into the queue with risk + conflict
/// heuristics. The "backlog" is the Approved pile that nobody has yet
/// committed to working — `aida backlog list` filters it, `analyze` reports
/// pairwise file-overlap, `groom` moves a curated selection onto the queue
/// (optionally under a single `batch:NAME` tag for `aida queue work
/// --batch NAME`).
// trace:STORY-444 | ai:claude
#[derive(Subcommand, Debug)]
pub enum BacklogCommand {
    /// Show Approved items that are not currently in any user's queue.
    /// Risk chips (low / medium / high / unknown) are advisory only.
    List {
        /// Filter to a single risk level (low, medium, high, unknown).
        #[clap(long, value_name = "LEVEL")]
        risk: Option<String>,
        /// Filter by requirement type (e.g. task, story, bug, doc).
        #[clap(long, value_name = "TYPE")]
        r#type: Option<String>,
        /// Filter by priority (high, medium, low).
        #[clap(long, value_name = "PRIORITY")]
        priority: Option<String>,
        /// Require this exact tag (case-insensitive).
        #[clap(long, value_name = "TAG")]
        tag: Option<String>,
        /// Require any tag with this prefix (case-insensitive).
        #[clap(long, value_name = "PREFIX")]
        tag_prefix: Option<String>,
        /// Cap the number of rows shown.
        #[clap(long, default_value = "50")]
        limit: usize,
        /// Emit a stable JSON shape instead of the table.
        #[clap(long)]
        json: bool,
        /// User ID for queue-membership scan (defaults to AIDA_USER /
        /// system user — same resolution as `aida queue list`).
        #[clap(long)]
        user: Option<String>,
    },
    /// Report pairwise file-overlap between selected backlog candidates.
    /// File sets come from inline spec-id trace markers in source plus the
    /// `## Critical Files` section of any owning plan in `docs/plans/`.
    Analyze {
        /// Comma-separated list of spec IDs to analyze (≥ 2 required).
        #[clap(long, value_name = "CSV")]
        specs: Option<String>,
        /// Shorthand for `--specs A,B` — analyzes exactly one pair.
        #[clap(long, value_names = ["A", "B"], num_args = 2)]
        pair: Option<Vec<String>>,
        /// Emit a stable JSON shape instead of the table.
        #[clap(long)]
        json: bool,
    },
    /// Move selected backlog items onto the queue. Optionally tags every
    /// groomed item with `batch:NAME` so `aida queue work --batch NAME`
    /// can drain them as one cluster.
    Groom {
        /// Comma-separated list of spec IDs to groom into the queue.
        #[clap(long, value_name = "CSV")]
        specs: Option<String>,
        /// Read newline-separated spec IDs from stdin (mutually exclusive
        /// with --specs).
        #[clap(long, conflicts_with = "specs")]
        from_stdin: bool,
        /// Auto-select every decision-free backlog item using the same
        /// pickability gate the burndown uses (bounded/not-epic, unblocked,
        /// decision-free, not parking-tagged), then groom the survivors.
        /// Mutually exclusive with --specs / --from-stdin. DRY-RUN BY DEFAULT:
        /// prints the would-groom and would-park sets; pass --apply to write.
        ///
        /// Caveat: "decision-free" here means "no attached DecisionRequest",
        /// which is coarser than true design-latitude — a design-heavy spec
        /// with no formal question still passes. Run `aida questions` before
        /// `--pickable --apply`, and keep the dry-run review load-bearing.
        #[clap(
            long,
            visible_alias = "auto",
            conflicts_with = "specs",
            conflicts_with = "from_stdin"
        )]
        pickable: bool,
        /// With --pickable, exclude candidates riskier than this level from
        /// auto-selection (low / medium / high / unknown). Uses the same risk
        /// chip `aida backlog list` shows. `--risk high` allows all;
        /// `--risk low` admits only low-risk. Default: medium.
        #[clap(long, value_name = "MAX", default_value = "medium")]
        risk: String,
        /// With --pickable, actually write the survivors to the queue.
        /// Without it, --pickable is a dry run (queueing = advisor sign-off).
        #[clap(long, visible_alias = "yes")]
        apply: bool,
        /// Tag every groomed item with `batch:NAME` (composes with
        /// `aida queue work --batch NAME`).
        #[clap(long, value_name = "NAME")]
        batch: Option<String>,
        /// Print what would happen without writing.
        #[clap(long)]
        dry_run: bool,
        /// Optional note recorded on every produced queue entry.
        #[clap(long)]
        note: Option<String>,
        /// Override the queue user (defaults to AIDA_USER / system user).
        #[clap(long)]
        user: Option<String>,
    },
    /// Sum latest effort estimates for approved/planned/draft backlog items
    /// that are not currently queued.
    // trace:STORY-451 | ai:codex
    Load,
}

/// Fasttrack-lane introspection subcommands.
///
/// The bare `aida fasttrack <title>` filing form lives on the parent
/// `Command::Fasttrack` variant; this enum holds the read-only lane views that
/// hang off it.
// trace:TASK-905 | ai:claude — plain `//` keeps the marker out of `--help`.
#[derive(Subcommand, Debug)]
pub enum FasttrackCommand {
    /// Show each lane item's stage: requested to shipped.
    ///
    /// A derived projection over the `batch:fasttrack` / `batch:express` lane
    /// buckets — no new store. Each item's stage
    /// (requested / accepted / queued / running / blocked / punted / shipped /
    /// rejected) is read off its existing status, queue membership, active
    /// lease, punt ledger, and merged state. Cache-fast: it reuses the same
    /// summaries `aida list` reads, not the full-store `aida status` scan.
    Status {
        /// Emit the projection as JSON for machine consumers.
        #[clap(long)]
        json: bool,
    },
}

/// Personal work queue commands.
// trace:STORY-368 | ai:claude
// trace:TASK-487 | ai:claude
// why: clap-derived subcommand enum — boxing a variant's fields would break the
// derive's flat arg parsing; the size skew is inherent to CLI command modeling.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
pub enum QueueCommand {
    /// List items in your queue. When a role is active and --role is not
    /// passed explicitly, defaults to filtering on that role. Without
    /// --global / --local, merges the local (per-project) queue with the
    /// active role's global queue and tags global entries with the
    /// originating project (`[origin:<project>]`).
    // trace:FR-1-012 | ai:claude
    List {
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Include completed requirements
        #[clap(long)]
        include_completed: bool,
        /// Filter to items routed to a specific role (e.g., "implementer").
        /// Pass `--for any` (or `--role any`) to filter to UNROUTED items
        /// (entries with no `for_role`), matching the write-side semantic
        /// of `aida queue add --for any`. Use `--all` (alone or combined
        /// with `--for X`) to override the active-role default. `--for X`
        /// composes with `--all` and takes precedence — i.e.
        /// `--all --for reviewer` shows only reviewer-routed items.
        /// `--for <role>` is the canonical form, matching
        /// `aida queue add --for` and `aida queue next --for`; `--role`
        /// is kept as a hidden alias for back-compat.
        // trace:TASK-71 BUG-87
        #[clap(long = "for", visible_alias = "role")]
        role: Option<String>,
        /// Override the active-role default filter (show all roles).
        /// Combines with `--for X` — `--for X` always wins.
        // trace:BUG-87
        #[clap(long)]
        all: bool,
        /// Bypass the active role's scope_tags / scope_status filters.
        // trace:TASK-1-021 | ai:claude
        #[clap(long)]
        no_scope: bool,
        /// Show only the global, role-scoped queue at `~/.aida/queue/<role>.yaml`.
        /// Mutually exclusive with --local.
        // trace:FR-1-012 | ai:claude
        #[clap(long, conflicts_with = "local")]
        global: bool,
        /// Show only the local, per-project queue (skip the global merge).
        /// Mutually exclusive with --global.
        // trace:FR-1-012 | ai:claude
        #[clap(long)]
        local: bool,
        /// Pull from `origin/aida-store` before listing. Opt-in freshness
        /// for collaborators / multi-session workflows; fast path stays
        /// default. No-op when the local orphan branch is already
        /// current; warns and falls back to the local view when offline.
        // trace:STORY-78 | ai:claude
        #[clap(long)]
        sync: bool,
        /// Include Completed/Rejected entries in the listing. Default
        /// hides them with a footer count so the queue view stays
        /// focused on actionable work.
        // trace:TASK-46 | ai:claude
        #[clap(long)]
        include_terminal: bool,
        /// Filter to entries whose `for_scope` matches one of these
        /// scopes (comma list). Pass `none` to filter to entries with
        /// no `for_scope` set (the "uncategorized" pile).
        /// Mirrors the `--scope` shape on `queue add`. Distinct from
        /// `--no-scope`, which bypasses the active role's
        /// scope_tags/scope_status filters — different axis.
        // trace:TASK-52, STORY-57 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "CSV")]
        scope: Option<String>,
        /// Group entries by their parent EPIC for a visual cluster view.
        /// EPICs are sorted by item count desc; unscoped items appear
        /// under a final "Unscoped" group. Within each group, queue
        /// position order is preserved.
        // trace:TASK-33 | ai:claude
        #[clap(long)]
        tree: bool,
        /// Suppress the "Done — awaiting merge" in-flight section. By
        /// default `aida queue list` appends Done specs (work finished
        /// on a branch, not yet merged to main) so the natural "what am
        /// I waiting on" view stays complete. Pass this to get only the
        /// queued (Pending/Approved/Planned/InProgress) view.
        // trace:TASK-222 | ai:claude
        #[clap(long)]
        no_in_flight: bool,
        /// Show only the "Done — awaiting merge" in-flight section,
        /// suppressing the regular queue entries. Useful for "what am I
        /// waiting on a PR for" snapshots.
        // trace:TASK-222 | ai:claude
        #[clap(long, conflicts_with = "no_in_flight")]
        in_flight_only: bool,
        /// Filter to queued items tagged `batch:NAME`. Composes with
        /// `aida queue work --batch NAME` — the same tag drives both
        /// the audit view and the per-item drain.
        // trace:TASK-229 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "NAME")]
        batch: Option<String>,
        /// Filter to entries whose requirement carries this exact tag
        /// (case-insensitive). The general form of `--batch`.
        // trace:TASK-238 | ai:claude
        #[clap(long, value_name = "TAG")]
        tag: Option<String>,
        /// Filter to entries with any tag starting with this prefix —
        /// e.g. `--tag-prefix batch:` for all batched items.
        // trace:TASK-238 | ai:claude
        #[clap(long, value_name = "PREFIX")]
        tag_prefix: Option<String>,
        /// Group the queue by `batch:*` tag value — each batch under
        /// its own header, un-batched items under "No batch". Queue
        /// position order is preserved within each group.
        // trace:TASK-238 | ai:claude
        #[clap(long)]
        by_batch: bool,
        /// Emit the queue as JSON (cache-fast machine read) instead of the
        /// human table. Used by the TUI queue panel to avoid the full
        /// status scan.
        // trace:BUG-616 | ai:claude
        #[clap(long)]
        json: bool,
        /// Fleet-wide bird's-eye: aggregate every user's queue (not just your
        /// shell identity), read-only, grouped by user then role with the
        /// owning user shown per row. Opt-in for the coordinator seat;
        /// composes with `--all` to span all roles. Default scoping (your
        /// user + active role) is unchanged without this flag.
        // trace:STORY-672
        #[clap(long)]
        all_users: bool,
        /// Narrow the listing to items whose spec is in this epic's
        /// TRANSITIVE descendant tree — the epic itself plus all its
        /// children and grandchildren, computed in-process from the
        /// requirement graph (the same hierarchy closure `aida graph
        /// <ID> --tree` walks). Filters the single queue; it does NOT
        /// create a per-epic queue. ANDs with the role / scope filters
        /// (`--for`, `--all`, `--global`, `--local`). `--parent` is an
        /// alias.
        // trace:TASK-923 | ai:claude
        #[clap(long, visible_alias = "parent", value_name = "ID")]
        epic: Option<String>,
        /// Ignore the active focus for this listing — show the whole queue
        /// instead of the focused subtree. No-op when no focus is set or when
        /// `--epic` was passed explicitly.
        #[clap(long)]
        no_focus: bool,
    },
    /// Add a requirement to your queue
    Add {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// Add to top of queue
        #[clap(long)]
        top: bool,
        /// Add to bottom of queue (default)
        #[clap(long)]
        bottom: bool,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Note explaining why this was queued
        #[clap(long)]
        note: Option<String>,
        /// Route this item to a specific role queue (e.g., "implementer",
        /// "architect"). The other role wearer can then `aida queue list`
        /// while in that role to see incoming work. Routing `--for human`
        /// files the item into the human-attention set — it then surfaces in
        /// `aida list human` alongside specs that need a person by status/tag.
        // trace:TASK-747 | ai:claude
        #[clap(long)]
        r#for: Option<String>,
        /// Restrict routing to sessions whose lease scope matches this
        /// (e.g., an EPIC id). Default-populated to the active session's
        /// scope when adding from inside a session worktree, unless
        /// --no-scope is passed. Pairs with --for so two implementer
        /// sessions don't see each other's incoming work.
        // trace:STORY-57 | ai:claude
        #[clap(long)]
        scope: Option<String>,
        /// Restrict routing to one specific session, by 8+ char lease id
        /// prefix. Mutually exclusive with --no-scope (passing both is
        /// nonsense — --for-session implies a scope match too).
        // trace:STORY-57 | ai:claude
        #[clap(long = "for-session", conflicts_with = "no_scope")]
        for_session: Option<String>,
        /// Suppress the auto-default scope when adding from inside a
        /// session worktree. The entry stays scope-unrouted (visible to
        /// any session in --for's role).
        // trace:STORY-57 | ai:claude
        #[clap(long)]
        no_scope: bool,
        /// Add to the role's GLOBAL queue at `~/.aida/queue/<role>.yaml`
        /// instead of the local per-project queue. Requires --for or an
        /// active role context (AIDA_SESSION_ROLE).
        // trace:FR-1-012 | ai:claude
        #[clap(long)]
        global: bool,
        /// Bypass the guard that refuses queueing a Completed or
        /// Rejected requirement. Use for legitimate re-open scenarios;
        /// the default error message hints at this flag.
        // trace:TASK-45 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        force: bool,
    },
    /// Sum latest effort estimates for queued items.
    // trace:STORY-451 | ai:codex
    // trace:TASK-956 | ai:claude — hidden from `queue --help` (still runs).
    #[clap(hide = true)]
    Load {
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
    },
    /// Remove a requirement from your queue
    Remove {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Remove from the role's GLOBAL queue (requires --for or active role).
        // trace:FR-1-012 | ai:claude
        #[clap(long)]
        global: bool,
        /// When --global, the role whose global queue to remove from.
        /// Defaults to the active role (AIDA_SESSION_ROLE).
        #[clap(long)]
        r#for: Option<String>,
    },
    /// Move a queue item to a new position
    Move {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// Move to the front of the queue (slot 1). `--to-front` and
        /// `--to-top` are accepted aliases for the same action. When
        /// the target is already at slot 1, the command is a friendly
        /// no-op that prints `<id> is already at queue head`.
        // trace:TASK-280 | ai:claude
        // trace:TASK-491 | ai:claude
        #[clap(long, visible_aliases = ["to-front", "to-top"])]
        top: bool,
        /// Move to the back of the queue (last slot). `--to-back` is an
        /// accepted alias for the same action.
        // trace:TASK-280 | ai:claude
        #[clap(long, visible_alias = "to-back")]
        bottom: bool,
        /// Move to an absolute 1-indexed slot among this user's live
        /// (non-terminal) queue entries — `--to 1` is the front, `--to 3`
        /// the third slot. An N past the end clamps to the last slot rather
        /// than erroring. These match a bare `aida queue list`; a
        /// role/scope-filtered list shows a subset, so the visible slot may
        /// differ there — prefer `--to-front`/`--to-back` under a role filter.
        // trace:TASK-280 | ai:claude — trace:TASK-317 (soften "same as queue list")
        #[clap(long, value_name = "N", conflicts_with_all = ["top", "bottom", "before", "after"])]
        to: Option<usize>,
        /// Move before this requirement ID
        #[clap(long)]
        before: Option<String>,
        /// Move immediately after this requirement ID — symmetric to
        /// `--before`. Useful for "put X right after Y" reasoning, and
        /// for inserting a cluster of items after a single anchor
        /// without each move shifting the previous one further from the
        /// anchor.
        // trace:STORY-72 | ai:claude
        // trace:TASK-318 | ai:claude
        #[clap(long)]
        after: Option<String>,
        /// Allow moving an entry whose spec has terminal status
        /// (Completed/Rejected). The default refuses with an error so
        /// `queue move` doesn't silently no-op on a stale entry the
        /// queue list view hides. Has no effect when the entry isn't in
        /// the queue at all — that case still errors.
        // trace:BUG-249 | ai:claude
        #[clap(long)]
        force: bool,
    },
    /// Bulk-remove ALL queue entries for the user (DOES NOT touch the specs
    /// themselves — their status, content, and relationships stay intact;
    /// only the queue-membership records are removed). The next
    /// `aida queue list` shows an empty queue; affected specs survive in
    /// `aida list`. The destructive verb in the queue family — there is no
    /// undo short of re-adding each entry with `aida queue add`.
    ///
    /// Sibling verbs (use one of these instead when the scope is narrower):
    ///   • `aida queue remove <id>`     — remove ONE specific entry
    ///   • `aida queue prune --orphaned` — remove entries whose backing
    ///                                     spec was deleted (the
    ///                                     "??? (deleted)" ghosts)
    ///   • `aida queue done <id>`       — atomic "mark complete + remove"
    ///
    // trace:TASK-1-109 | ai:claude — plain `//` (not `///`) so the
    // trace marker doesn't leak into user-facing --help output per
    // TASK-268. The user-facing prose above is intentionally SPEC-ID-
    // free.
    Clear {
        /// User ID whose queue to clear (defaults to AIDA_USER or the
        /// shell's $USER). Does not affect other users' queues.
        #[clap(long)]
        user: Option<String>,
        /// Only remove queue entries whose backing spec has status
        /// `Completed`. Useful for tidying up after a batch ship: keeps
        /// the in-flight Approved / InProgress / Done entries, removes
        /// the ones whose work is already shipped. Orphan entries
        /// (backing spec deleted) are left in place — clean those up
        /// with `aida queue prune --orphaned`.
        #[clap(long)]
        completed: bool,
    },
    /// Prune queue entries matching a predicate. Today the only predicate is
    /// `--orphaned` (queue entries pointing at deleted/missing specs); future
    /// predicates may be added. Use `--dry-run` to preview before applying.
    // trace:TASK-537 | ai:claude — plain `//` so SPEC-ID doesn't leak into
    // user-facing --help output per TASK-268 convention.
    Prune {
        /// Remove queue entries whose backing spec no longer exists in the
        /// store (the "??? (deleted)" ghosts in `aida queue list`). Auto-
        /// queued reviewer entries from `aida pr` / `session end` can become
        /// orphans when the spec they cover is later deleted or rejected.
        #[clap(long)]
        orphaned: bool,
        /// Remove auto-queued reviewer entries whose PR has already merged. A
        /// review row ("Review PR-N: …") lingers when its PR merges outside the
        /// reviewer's `aida queue done` flow (e.g. a hand-merge); the backing
        /// spec is often still non-terminal so `--orphaned` misses it. Checks
        /// each review row's PR state with `gh`. Combine with `--orphaned` to
        /// sweep both.
        #[clap(long)]
        merged: bool,
        /// Preview the entries that would be removed; don't actually remove
        /// them. Pair with `--orphaned` / `--merged` for safe inspection.
        #[clap(long)]
        dry_run: bool,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Restrict prune to entries routed to this role. Default: all roles
        /// (with the active-role default applied). Useful when only the
        /// reviewer queue has orphan entries.
        #[clap(long = "for", visible_alias = "for-role")]
        r#for: Option<String>,
    },
    /// Peek at the top item in your queue without removing it. When a role
    /// is active and --role is not passed, defaults to filtering on it.
    /// Use this between work items to see what's next. Considers local +
    /// global queues by default (local wins on tiebreaks).
    // trace:FR-1-012 | ai:claude
    Next {
        /// Filter to items routed to a specific role. Pass `--for any` to
        /// peek the top UNROUTED item (matches `aida queue add --for any`
        /// write-side semantic). `--for <role>` is the canonical form,
        /// matching `aida queue add --for` and `aida queue list --for`;
        /// `--role` is kept as an alias. Composes with `--all`; `--for X`
        /// always takes precedence.
        // trace:TASK-71 BUG-87 | ai:claude
        #[clap(long = "for", visible_alias = "role")]
        role: Option<String>,
        /// Override the active-role default filter (show top from any
        /// role). Combines with `--for X` — `--for X` always wins.
        // trace:BUG-87
        #[clap(long)]
        all: bool,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Bypass the active role's scope_tags / scope_status filters.
        // trace:TASK-1-021 | ai:claude
        #[clap(long)]
        no_scope: bool,
        /// Look only in the global queue (skip local).
        #[clap(long, conflicts_with = "local")]
        global: bool,
        /// Look only in the local queue (skip global).
        #[clap(long)]
        local: bool,
    },
    /// Walk the queue and advance each item to its next step — drain the
    /// autonomous, dispatch the human-required (review / --zen / decision)
    /// interactively. Processes to a resolution, never hides work.
    // trace:STORY-566 | ai:claude
    Advance {
        /// Advance just this queued spec; omit to walk the whole queue.
        id: Option<String>,
        /// Non-interactive: take only the unambiguous autonomous step per
        /// item, skip anything needing a human.
        #[clap(long, short = 'y')]
        yes: bool,
        /// Override the queue user (defaults to AIDA_USER / system user).
        #[clap(long)]
        user: Option<String>,
    },
    /// Mark a requirement as completed AND remove it from your queue in one
    /// atomic step. Convenience for the implementer's done-then-pickup-next
    /// loop. Equivalent to: `aida edit <id> --status completed && aida queue remove <id>`.
    Done {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Skip confirmation
        #[clap(long, short = 'y')]
        yes: bool,
        /// Bypass the commits-without-PR pre-check. Use only when the spec
        /// was implemented on a different branch that is already merged,
        /// so the local branch has no PR by design. Logs the bypass.
        // trace:BUG-269 TASK-423 | ai:claude
        #[clap(long)]
        force: bool,
        /// Bypass the commits-without-PR pre-check (same effect as `--force`
        /// for this gate). Named for intent — `--force` is a general escape
        /// hatch, `--skip-pr-check` is specifically the pre-check gate. Logs
        /// the bypass.
        // trace:BUG-285 TASK-447 | ai:claude
        #[clap(long)]
        skip_pr_check: bool,
        /// Record a user-facing CLI surface change for the operator digest.
        /// Repeatable. e.g. `--interface-cli "aida foo — new command"`. The
        /// deterministic source for `aida digest --audience operator`; absent ⇒
        /// this spec never appears there. Skips the interactive capture prompt
        /// when any --interface-* flag is given.
        // trace:STORY-542 | ai:claude
        #[clap(long = "interface-cli", value_name = "LINE")]
        interface_cli: Vec<String>,
        /// Record a user-facing MCP surface change (new tool, gating, schema).
        /// Repeatable. See `--interface-cli`.
        // trace:STORY-542 | ai:claude
        #[clap(long = "interface-mcp", value_name = "LINE")]
        interface_mcp: Vec<String>,
        /// Record a user-facing TUI surface change (keybinding, pane, overlay).
        /// Repeatable. See `--interface-cli`.
        // trace:STORY-542 | ai:claude
        #[clap(long = "interface-tui", value_name = "LINE")]
        interface_tui: Vec<String>,
        /// Record some other user-facing interface change (not cli/mcp/tui).
        /// Repeatable. See `--interface-cli`.
        // trace:STORY-542 | ai:claude
        #[clap(long = "interface-other", value_name = "LINE")]
        interface_other: Vec<String>,
        /// Explicitly mark this spec as having NO user-facing interface change
        /// (clippy/refactor/test/internal). Skips the capture prompt and
        /// records nothing — the spec stays out of the operator digest.
        // trace:STORY-542 | ai:claude
        #[clap(long)]
        no_interface_change: bool,
    },
    /// Pick up a queued item (or scope-cluster of queued items) and launch
    /// claude in a fresh session worktree, with the role + skill routed
    /// from the item's metadata. Collapses the 5-7 manual steps (pull,
    /// session start, cd, role enter, claude /aida-pickup) into one
    /// command. With no `id`, picks the head of the active role's queue.
    /// With `id` matching a queued entry, that single item is the pickup
    /// target. With `id` resolving to an EPIC/STORY whose children are
    /// queued, drains the cluster — pre-populates the session manifest
    /// and routes the right skill.
    ///
    /// When `id` resolves to a known spec that ISN'T queued (and has no
    /// queued children), the error is status-aware: the recovery hint
    /// depends on the spec's current status — Done → `aida queue
    /// rework`; Planned → promote to Approved first; Completed/Rejected →
    /// re-open with `--force`.
    // trace:STORY-42, TASK-217 | ai:claude
    // trace:TASK-487 | ai:claude
    Work {
        /// Queued requirement ID (UUID, SPEC-ID, or agreed-id) for item
        /// pickup, OR an EPIC/STORY id with queued children for cluster
        /// pickup, OR `batch:NAME` for batch pickup (the positional
        /// equivalent of `--batch NAME` — accepts the literal tag printed
        /// by `aida queue list`), OR the `next` keyword: `next` picks the
        /// queue head (explicit form of the no-arg pickup), and `nextN`
        /// (e.g. `next3`) paired with `--auto-complete` drains the next N
        /// items from the head. Omit to pick up the head of the active
        /// role's queue.
        // trace:TASK-293 | ai:claude — plain `//` so the marker stays out
        // of `--help` output (TASK-268 user-facing-text convention).
        id: Option<String>,
        /// Count for the `next` keyword's spaced form — `aida queue work
        /// next 3` is equivalent to the compact `aida queue work next3`.
        /// Only meaningful directly after the `next` keyword.
        // trace:TASK-293 | ai:claude — plain `//` keeps the marker out of `--help`.
        count: Option<String>,
        /// Permission mode for the launched claude. Resolution order:
        ///   1. this flag
        ///   2. `AIDA_PERMISSION_MODE` env var
        ///   3. `.aida/config.toml [behavior] permission_mode`
        ///   4. AIDA-managed worktree default → `bypassPermissions`
        ///      (the worktree is git-sandboxed; the prompt flood was
        ///      eating autonomous overnight runs and Claude Code keeps
        ///      the rm -rf circuit breakers on)
        ///   5. fallback → `acceptEdits`
        ///      Common values: `auto` (research preview: auto-approves with
        ///      background safety checks; pairs with the pre-allow list),
        ///      `bypassPermissions` (legacy `--dangerously-skip-permissions`),
        ///      `plan` (read-only), `default` (prompt on everything). Passed
        ///      through unvalidated to `claude --permission-mode`.
        // trace:STORY-42, TASK-82, TASK-83, TASK-84 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "MODE")]
        permission_mode: Option<String>,
        /// Launch Claude in contained mode: strict Bash sandboxing, no
        /// unsandboxed fallback, destructive-command deny rules, and
        /// project-relative edit auto-allow only.
        #[clap(long)]
        sandbox: bool,
        /// Run the full setup (worktree, lease, manifest) but skip the
        /// final `claude` exec. Useful for scripting / debugging the
        /// resolver.
        #[clap(long)]
        no_launch: bool,
        /// Plan-only mode: launch a PLANNING session for the spec instead of
        /// an implementing one. Runs `/aida-plan <SPEC>` (not `/aida-pickup`)
        /// and defaults the permission mode to `plan` (read-only) so the
        /// session writes a `docs/plans/` file without touching code. After
        /// the plan lands, promote the spec with `aida plan promote <SPEC>`
        /// (Approved -> Planned). Lets you plan work ahead while another spec
        /// is being implemented (the parallel-pipelining workflow). Headless
        /// auto-complete plan-phase is separate; this flag is interactive
        /// only, so it conflicts with --auto-complete for now.
        // trace:STORY-265 | ai:claude
        #[clap(long, conflicts_with = "auto_complete")]
        plan_only: bool,
        /// Plan-then-ship: run a PLAN phase before the implementer phase, then
        /// the normal autonomous drain. The plan phase produces a `docs/plans/`
        /// file (the same planning session as `--plan-only`) and promotes the
        /// spec Approved -> Planned; the drain then implements, CIs, reviews,
        /// merges, pulls, and builds as usual. Opt-in — without this flag the
        /// drain starts straight at the implementer phase (default unchanged).
        /// Only meaningful with an autonomous drain (`--auto-complete` /
        /// `--drain`). Composes with `--no-human` (the plan phase runs headless
        /// too when the implementer does).
        // trace:STORY-265 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, requires = "autonomous")]
        with_plan: bool,
        /// Override the inferred role. Without this, the role is derived
        /// from the queue items' `for_role` (single-item: that item's
        /// role; cluster: majority with a warning about minority items).
        /// Falls back to active shell role when items have no `for_role`.
        #[clap(long, value_name = "NAME")]
        role: Option<String>,
        /// Skip the pre-pickup `aida db sync --pull`. By default queue
        /// work pulls the orphan-store branch first so the queue view is
        /// fresh; pass this when offline or when the user has just
        /// pulled in another shell.
        #[clap(long)]
        no_pull: bool,
        /// Cluster-mode filter: when `id` resolves to a parent scope,
        /// only drain queued children whose requirement type matches.
        /// E.g. `--type bug` to grab just the bug cluster under a
        /// given epic. Case-insensitive. Ignored in single-item / head
        /// pickup modes.
        #[clap(long = "type", value_name = "TYPE")]
        type_filter: Option<String>,
        /// Override the auto-derived branch name. Mirrors
        /// `aida session start --branch`.
        #[clap(long)]
        branch: Option<String>,
        /// Override the auto-derived worktree path. Mirrors
        /// `aida session start --path`.
        #[clap(long)]
        path: Option<String>,
        /// Stack this session's branch atop the most recent un-merged
        /// in-flight implementer branch instead of forking from
        /// `origin/main`. Lets the user start spec Y while spec X's PR is
        /// still in CI/review — when X merges, `aida pull` auto-rebases Y
        /// onto the new main (slice 3). The resolver picks the freshest
        /// implementer lease whose branch is not yet merged; the lease
        /// covering cwd is skipped so a session can't stack on itself.
        /// Mutually exclusive with `--base`.
        // trace:STORY-248 | ai:claude
        #[clap(long, conflicts_with = "base")]
        stack: bool,
        /// Explicit base branch for this session — fork from `BRANCH`
        /// instead of `origin/main`. The branch must exist locally or on
        /// origin. Refuses if the branch's PR has already merged
        /// (suggests `aida pull` + a fresh `--base main`); pass
        /// `--force-base` to override.
        // trace:STORY-248 | ai:claude
        #[clap(long, value_name = "BRANCH", conflicts_with = "stack")]
        base: Option<String>,
        /// Bypass the `--base BRANCH` safety check that refuses an
        /// already-merged base. Use when you know the rebase will still
        /// apply cleanly (e.g. base merged but branch tip hasn't been
        /// reused). Only meaningful with `--base`.
        // trace:STORY-248 | ai:claude
        #[clap(long = "force-base")]
        force_base: bool,
        /// Override the In-Progress guard when the target scope is already
        /// held by another active lease. Without --steal, `aida queue work`
        /// refuses and names the holding lease. With --steal, the holding
        /// session is ended first (`aida session end <id>` — clean exit;
        /// uncommitted work still blocks, exit those manually or use
        /// `aida session end --force`) and the new session takes over.
        // trace:TASK-81 | ai:claude
        #[clap(long)]
        steal: bool,
        /// Claim a spec whose status is ambiguous: already In Progress with
        /// no local lease, or Needs Attention after an autonomous punt. This
        /// mirrors `aida session start --force-claim`; terminal statuses and
        /// Draft still refuse.
        // trace:TASK-559 | ai:codex
        #[clap(long)]
        force_claim: bool,
        /// Override the focus-scope drift guard: start work on a
        /// spec outside the active `aida focus` subtree even when `[focus]
        /// out_of_scope = "block"` would refuse, and silence the `warn` nudge.
        /// Always overrides regardless of the configured policy.
        // trace:STORY-717 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        force: bool,
        /// Tag-driven batch pickup: instead of resolving from `id`, pick
        /// the head queued item tagged `batch:<NAME>`. Repeat the
        /// invocation to drain the batch one session per item (each
        /// session exits independently; the next pickup picks the new
        /// head). Mutually exclusive with positional `id` and
        /// `--type`. `--dry-run --batch NAME` lists members in pickup
        /// order without acting. Composes with `--auto-complete`:
        /// `--batch NAME --auto-complete` drains the whole batch
        /// autonomously — one full lifecycle per member — instead of one
        /// session per re-invocation.
        // trace:TASK-229, TASK-285 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "NAME", conflicts_with_all = ["id", "type_filter"])]
        batch: Option<String>,
        /// Ordered multi-batch auto-complete drain. Comma-separated names are
        /// drained left-to-right, exhausting one `batch:NAME` before the next.
        /// Equivalent comma-separated values are also accepted by `--batch`
        /// when paired with `--auto-complete`.
        // trace:TASK-310 | ai:codex
        #[clap(
            long,
            value_name = "A,B,C",
            requires = "auto_complete",
            conflicts_with_all = ["id", "batch", "type_filter", "dry_run"]
        )]
        batches: Option<String>,
        /// Coupled-sequential drain: with `--batch NAME --auto-complete`, drive
        /// the batch members on ONE shared branch in ONE worktree — each member
        /// is implemented + CI'd and committed in place (no per-member
        /// merge-to-main, no reset between members), then ONE cluster PR is
        /// opened linking every member. For TIGHTLY-COUPLED work that ships
        /// together, where later increments build on earlier commits. A member
        /// failure HALTS the drain, keeping prior members' commits on the
        /// branch. Contrast the default batch drain, which merges each member to
        /// main as its own PR. Requires `--batch` or `--batches`.
        // trace:TASK-1003, SPIKE-70 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, requires = "autonomous")]
        single_branch: bool,
        /// Coupled-sequential drain: with `--batch NAME --auto-complete`, drive
        /// the batch members ONE AT A TIME, in pickup order — each member forks
        /// off the freshly-pulled main, runs its full lifecycle, and merges as
        /// its OWN PR before the next member starts. For coupled-but-
        /// independently-shippable work that must land in order (each increment
        /// stays a reviewable PR to main). A member failure SHELVES that member
        /// and the drain continues with the rest — contrast `--single-branch`,
        /// which accumulates every member on one branch and HALTS on a failure.
        /// This names + guards the existing batch drain, which is already
        /// one-member-at-a-time; concurrency is pinned to 1. Requires `--batch`
        /// or `--batches`.
        // trace:TASK-1005, SPIKE-70 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, requires = "autonomous", conflicts_with = "single_branch")]
        sequential: bool,
        /// Preview without acting. For a single spec: print the resolved
        /// plan — session id, worktree path, branch, role, skill, and the
        /// lease it WOULD set up — then exit, creating no worktree, no
        /// lease, and launching no session. With `--batch`: print the
        /// matching queue entries in pickup order. Lets you safely see what
        /// a pickup will do before committing to it.
        // trace:TASK-229 | ai:claude
        // trace:TASK-1053 | ai:claude
        #[clap(long)]
        dry_run: bool,
        /// Resume a prior `claude` conversation for this scope instead of
        /// cold-launching. Bare `--resume` continues the most recent
        /// recorded session for the scope; `--resume <session-id>` (or a
        /// unique prefix) continues a specific one. The conversation's
        /// JSONL persists after `aida session end` removes the worktree,
        /// so a session can be resumed days later. Mutually exclusive
        /// with `--fresh`.
        // trace:TASK-112 | ai:claude
        #[clap(
            long,
            value_name = "SESSION-ID",
            num_args = 0..=1,
            default_missing_value = "",
            conflicts_with = "fresh"
        )]
        resume: Option<String>,
        /// Force a cold launch even when prior `claude` sessions exist
        /// for this scope — suppresses the default resume prompt.
        // trace:TASK-112 | ai:claude
        #[clap(long)]
        fresh: bool,
        /// List the recorded `claude` sessions for this scope (most
        /// recent first) and exit without launching a session.
        // trace:TASK-112 | ai:claude
        #[clap(long)]
        list_sessions: bool,
        /// Caller-minted Claude conversation id (a UUID) to launch with,
        /// instead of the auto-generated one. The AIDA TUI passes this so
        /// it can track + later resume the conversation deterministically.
        /// Implies a fresh launch — mutually exclusive with `--resume`.
        // trace:STORY-132, EPIC-26 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "UUID", conflicts_with = "resume")]
        session_id: Option<String>,
        /// Which vendor CLI hosts the interactive session: `claude` (default)
        /// or `codex`. The AIDA TUI passes `--vendor codex` to host a Codex
        /// tab. Codex has no caller-minted session id, so `--vendor codex`
        /// hosts a fresh interactive session and ignores `--session-id` /
        /// `--resume`. Only affects the interactive (non-headless) launch.
        // trace:TASK-895 | ai:claude
        #[clap(long, value_name = "VENDOR", default_value = "claude")]
        vendor: String,
        /// Drive the full implementer → CI → reviewer → merge → pull →
        /// build lifecycle for one SPEC in a single command, instead of
        /// running the 5+ steps by hand. The orchestrator spawns each
        /// Claude session, waits for it, and advances; the human only
        /// interacts inside the sessions themselves. Bare `--auto-complete`
        /// runs all six phases; the value picks a variant that stops early:
        ///   through-ci    — stop after CI is green (PR routed to reviewer)
        ///   through-merge — stop after the merge (skip pull + build)
        ///   skip-build    — phases 1-5 (skip the final build verify)
        /// Exit code is 0 on success, else the 1-based index of the phase
        /// that failed (1 implementer, 2 CI, 3 review, 4 merge, 5 pull,
        /// 6 build). Takes a SPEC id; with no id it picks the head of the
        /// active role's queue, the same way the no-arg `aida queue work`
        /// does. Or pass `--batch NAME` to drain a whole batch:
        /// `--batch NAME --auto-complete` runs one full lifecycle per batch
        /// member until the batch is empty, `--max` is reached, or a phase
        /// fails. The `nextN` keyword (e.g. `aida queue work next3
        /// --auto-complete`) is the un-pre-tagged equivalent — it drains the
        /// next N items from the queue head in order.
        // trace:STORY-246, TASK-285, TASK-293 | ai:claude
        // trace:TASK-487 | ai:claude
        // TASK-560: `resume` is intentionally NOT in this clap conflict list.
        // The pair is still rejected, but by a manual check in the handler that
        // explains WHY they conflict and names both recovery paths — clap's
        // auto-generated "cannot be used with" message left the operator
        // stuck. trace:TASK-560 | ai:claude
        #[clap(
            long,
            value_name = "MODE",
            num_args = 0..=1,
            default_missing_value = "full",
            group = "autonomous",
            conflicts_with_all = [
                "no_launch", "fresh", "list_sessions",
                "dry_run", "type_filter",
            ]
        )]
        auto_complete: Option<String>,
        /// Drain the queue: try to ship every drivable queued item in order,
        /// fully autonomously, skipping items that can't be driven (blocked,
        /// human-only, already in flight). A discoverable shorthand for
        /// `--auto-complete --no-human=both --max <queue-size>`. Composes with
        /// `--batch` (drain that batch instead of the whole queue) and
        /// `--max-failures` (cap the shelving budget). Explicit `--auto-complete`,
        /// `--no-human`, or `--max` flags override the shorthand's defaults.
        // trace:TASK-578 | ai:claude — plain `//` keeps the marker out of `--help`.
        // `group = "autonomous"` lets `--drain` satisfy the `requires` on the
        // auto-complete-only flags (`--max-failures`, `--json`, …) the same way
        // an explicit `--auto-complete` does. trace:TASK-578 | ai:claude
        #[clap(long, group = "autonomous")]
        drain: bool,
        /// With `--auto-complete`: emit one JSON line per phase transition
        /// on stdout (machine-readable progress for TUI / scripting) instead
        /// of the human-readable progress lines.
        // trace:STORY-246 | ai:claude
        // TASK-578: `requires = "autonomous"` so `--drain` (a group member)
        // satisfies it the same way `--auto-complete` does.
        #[clap(long, requires = "autonomous")]
        json: bool,
        /// With `--batch NAME --auto-complete`: stop the batch drain after
        /// N members ship, even when the batch has more queued. Without it
        /// the drain runs until the batch is empty for the role. Only
        /// meaningful for a batch drain.
        // trace:TASK-285 | ai:claude
        // trace:TASK-578 — `requires = "autonomous"` so `--drain` satisfies it.
        #[clap(long, value_name = "N", requires = "autonomous")]
        max: Option<usize>,
        /// With `--auto-complete`: after this many phase failures shelve in
        /// a single batch, stop the drain entirely rather than continue
        /// past the wreck (the environment is probably broken — gh
        /// rate-limited, CI infra down, etc.). Default is 5. Set to 0 to
        /// fall back to the historical "first failure stops" behaviour.
        /// Per-batch — a `--batches A,B,C` chain gets an independent budget
        /// for each.
        // trace:EPIC-28 | ai:claude
        // trace:TASK-578 — `requires = "autonomous"` so `--drain --max-failures`
        // composes (the spec's named composition case).
        #[clap(long, value_name = "N", requires = "autonomous")]
        max_failures: Option<usize>,
        /// With `--auto-complete`: hard budget cap on cumulative reported tokens
        /// (input + output + cache) across the whole drain. Once the headless
        /// phases' accumulated usage crosses this, the drain stops cleanly at
        /// the next spec boundary rather than burning more quota on a wedged
        /// chunk. A backstop for unattended drains; composes with
        /// `--max-failures` and the goal condition (whichever fires first).
        // trace:TASK-966 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_name = "N", requires = "autonomous")]
        max_tokens: Option<u64>,
        /// With `--auto-complete`: stop the drain before starting the next spec
        /// once this many specs have been acted on (shipped / punted /
        /// escalated / shelved). A hard iteration cap for unattended drains.
        // trace:TASK-966 | ai:claude
        #[clap(long, value_name = "N", requires = "autonomous")]
        max_iterations: Option<u64>,
        /// With `--auto-complete`: wall-clock budget for the whole drain. A bare
        /// number is minutes; suffixes / compounds work too (`90s`, `45m`,
        /// `2h`, `1h30m`). The drain stops between specs once the deadline
        /// passes — in-flight work is never interrupted mid-phase.
        // trace:TASK-966 | ai:claude
        #[clap(long, value_name = "DUR", requires = "autonomous")]
        max_runtime: Option<String>,
        /// With `--auto-complete`: minutes a *headless* phase may make no
        /// commit / file-change before the watchdog kills + shelves it (a
        /// degenerate echo/sleep spin). Default 10; `0` disables. Overrides
        /// `[drain] no_progress_minutes`.
        // trace:BUG-420 | ai:claude
        // trace:TASK-578 — `requires = "autonomous"` so `--drain` satisfies it.
        #[clap(long, value_name = "MIN", requires = "autonomous")]
        no_progress_minutes: Option<u64>,
        /// With `--auto-complete`: hard wall-clock ceiling (minutes) per
        /// *headless* phase — a backstop in case progress-detection misses.
        /// Default 45; `0` disables. Overrides `[drain] phase_ceiling_minutes`.
        // trace:BUG-420 | ai:claude
        // trace:TASK-578 — `requires = "autonomous"` so `--drain` satisfies it.
        #[clap(long, value_name = "MIN", requires = "autonomous")]
        phase_ceiling_minutes: Option<u64>,
        /// Resume a crashed `--auto-complete` drain from `.aida/drain-state.json`
        /// instead of starting fresh. Probes git/PR/spec reality to re-enter at
        /// the first incomplete phase (never re-merging a merged PR), and
        /// refuses if the original orchestrator process is still alive
        /// (double-drive guard). Pair with `--dry-run` to preview the plan
        /// without re-entering. Distinct from `--resume` (which continues a
        /// Claude *session*).
        // trace:STORY-492 | ai:claude
        #[clap(long, requires = "auto_complete")]
        resume_drain: bool,
        /// With `--resume-drain`: only resume if the recorded drain matches this
        /// id (its run UUID or start-timestamp prefix) — a guard against
        /// resuming a stale state file.
        // trace:STORY-492 | ai:claude
        #[clap(long, value_name = "ID", requires = "resume_drain")]
        drain_id: Option<String>,
        /// With `--resume-drain` or `--from-pr`: print the reconciled re-entry
        /// plan (which phase the drive would re-enter at) WITHOUT re-entering.
        /// The safe way to preview a resume / PR-only drive. (The plain
        /// `--dry-run` cannot be combined with `--auto-complete`, hence a
        /// dedicated flag.)
        // trace:STORY-492 | ai:claude
        // trace:TASK-405 | ai:claude — now also previews a `--from-pr` drive.
        #[clap(long, requires = "auto_complete")]
        resume_dry_run: bool,
        /// PR-only invocation: implementation already shipped OUTSIDE the
        /// orchestrator (a PR is already open for the spec), so SKIP the
        /// implementer phase and drive the remaining phases
        /// (reviewer → CI → merge → pull → build). Probes the PR's real state
        /// to pick the entry phase: CI not green → reviewer (CI-wait is
        /// implementer-coupled and cannot be re-run by a fresh process);
        /// reviewer done → merge; etc. Refuses cleanly if no open PR exists,
        /// the PR is already merged, or the spec is already Completed.
        /// Composes with `--auto-complete` and its `through-ci` / `through-merge`
        /// / `skip-build` variants. Distinct from `--resume-drain` (which
        /// recovers a CRASHED orchestrator's own drain from its state file);
        /// `--from-pr` engages a FRESH orchestrator on a PR that progressed
        /// outside it.
        // trace:TASK-405 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, requires = "auto_complete", conflicts_with = "resume_drain")]
        from_pr: bool,
        /// Run `--auto-complete` phases headless (`claude -p`) so the drain
        /// needs no Ctrl+D.
        ///
        /// reviewer-only (default, also bare `--no-human`): phase 3
        /// (reviewer) runs headless; phase 1 (implementer) stays interactive
        /// and the drain PAUSES there for you.
        ///
        /// both: full headless drain, implementer included. On a design-fork
        /// it cannot safely resolve the implementer punts the spec to Needs
        /// Attention rather than guessing — triage punts with
        /// `aida findings list`.
        ///
        /// Headless sessions force `--permission-mode bypassPermissions` and
        /// log JSON output under `.aida/headless-logs/`. Aliases:
        /// `--unattended`, `--headless`.
        // trace:STORY-263, TASK-306, STORY-276 | ai:claude — plain `//` so the
        // marker stays out of `--help` (TASK-268 user-facing-text convention).
        #[clap(
            long,
            value_name = "MODE",
            num_args = 0..=1,
            default_missing_value = "reviewer-only",
            aliases = ["unattended", "headless"]
        )]
        no_human: Option<String>,
        /// Under `--no-human=both`: leave a spec parked in Needs Attention
        /// when the headless advisor escalates its design-fork to a human,
        /// and advance the drain (the default). A paused spec beats a
        /// guessed one — the conservative choice. Mutually exclusive with
        /// `--escalate-defaults`; only meaningful with `--no-human=both`.
        // trace:STORY-306 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, conflicts_with = "escalate_defaults")]
        escalate_blocks: bool,
        /// Under `--no-human=both`: when the headless advisor escalates a
        /// design-fork, resume the implementer to ship the defensible
        /// default rather than parking the spec, and file a needs-human
        /// finding for post-hoc review. For mechanical batches where
        /// throughput beats per-spec correctness. Only meaningful with
        /// `--no-human=both`.
        // trace:STORY-306 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        escalate_defaults: bool,
        /// "Advisor on standby" autonomy mode: auto-resolve mechanical
        /// confirmation prompts (open PR? grab next item? end session?)
        /// without pausing, while still pausing for genuine design-fork
        /// questions. Use it when you are at the keyboard but don't want to
        /// click-yes through every mechanical step. Sets `AIDA_ZEN=1` in the
        /// launched session (exporting `AIDA_ZEN=1` yourself is equivalent).
        /// When combined with `--no-human`, `--no-human` wins (it is the
        /// stronger mode) and a warning is printed.
        // trace:STORY-287 | ai:claude — plain `//` so the marker stays out
        // of `--help` output (TASK-268 user-facing-text convention).
        #[clap(long)]
        zen: bool,
        /// With `--zen`: always pause at the grab-next/stop checkpoint after
        /// the PR opens, even on a clean finish — for when you want to drive
        /// grab-next by hand. By default a clean `--zen` finish (no human ever
        /// needed) auto-runs `aida session end` and exits. Equivalent to
        /// `[zen] auto_exit = false` in `.aida/config.toml`, per-invocation.
        /// No effect without `--zen`.
        // trace:STORY-564 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        pause_always: bool,
        /// Suppress the end-of-command summary a standalone
        /// `aida queue work <PR-N> --role reviewer` prints (verdict, cost,
        /// artifact paths). For scripted consumers that read the verdict
        /// file `.aida/review-verdicts/PR-N.json` directly. No effect on
        /// non-reviewer or orchestrator-driven runs.
        // trace:BUG-226 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        quiet: bool,
        /// Suppress the high-signal headless tee under `--no-human`. By
        /// default the parent prints `│ [headless] ...` lines for the
        /// headless Claude's `system.init`, `assistant.text`, `tool_use`,
        /// and `result` events so the operator can follow progress without
        /// a second terminal. With this flag (or `AIDA_TEE_HEADLESS=0`),
        /// the chatter is silenced but failures (`is_error`,
        /// `permission_denials`) still stream — they must NEVER hide.
        // trace:TASK-307 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        no_tee_headless: bool,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Calibration mode override — turn the cold-boot vs fork-from-live
        /// advisor calibration ledger ON for *this* drain regardless of
        /// `[advisor] calibration_mode`. With it on, every punt produces
        /// both a cold-boot verdict (which drives the drain) and a
        /// fork-from-live verdict (shadow only) when a live advisor is
        /// registered. Recorded to `.aida/punts/<punt-id>/calibration.yaml`.
        /// Only meaningful with `--no-human=both`.
        // trace:STORY-347 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, conflicts_with = "no_calibrate")]
        calibrate: bool,
        /// Calibration mode override — turn calibration OFF for *this*
        /// drain regardless of `[advisor] calibration_mode`. Pair with
        /// `--calibrate` to flip the toggle per-drain.
        // trace:STORY-347 | ai:claude
        #[clap(long)]
        no_calibrate: bool,
        /// Opt out of the reviewer pre-flight stale-base refusal. When
        /// the orchestrator (phase 3) or a direct `aida queue work
        /// <PR-N> --for reviewer` detects that the PR's base is behind
        /// origin AND a file the PR touches has also moved on the base
        /// since the PR forked, the reviewer is refused so reviews
        /// don't run against stale code. Pass this flag to override
        /// the refusal and review against stale anyway. The "stale +
        /// no file overlap" case prints a warning either way.
        // trace:STORY-281 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        allow_stale_base: bool,
        /// Opt out of the reviewer pre-flight intermediate-only refusal.
        /// By default the reviewer refuses a PR whose diff changes ONLY
        /// intermediate/generated files (build outputs, gitignored
        /// paths, lockfiles with no source change) because such a fix is
        /// not reproducible — it's overwritten on the next build. Pass
        /// this flag when the PR is a deliberate regeneration of
        /// checked-in build output.
        // trace:TASK-480 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        allow_intermediate_only: bool,
        /// Opt out of phase-3 auto-rebase recovery. By default a fully
        /// headless auto-complete drain that hits the reviewer stale-base
        /// pre-flight attempts one clean `aida pr rebase` before refusing.
        // trace:STORY-429 | ai:claude
        #[clap(long)]
        no_auto_rebase: bool,
        /// Implementer's pickup-time complexity estimate:
        /// `low` / `med` / `high`. Captured to
        /// `.aida/complexity-calibration/<SPEC>.yaml` AND stamped onto
        /// the spec as a `complexity:<level>` tag so existing tag
        /// tooling (e.g. `aida queue list --tag-prefix complexity:`)
        /// works on the new dimension. Best-effort, not graded — feeds
        /// the three-way calibration view, never an approval gate.
        /// Absent ⇒ no pickup slot is captured.
        // trace:STORY-439 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_enum, value_name = "LEVEL")]
        complexity: Option<crate::complexity_calibration::ComplexityLevel>,
        /// Implementer's pickup-time assistance estimate:
        /// `none` / `advisor` / `human` — how much help the spec is
        /// expected to need. Captured alongside `--complexity` and
        /// stamped as `estimated-assistance:<level>`. The actual
        /// intervention count comes from the punt ledger
        /// (`.aida/punts.jsonl`); this flag captures only the
        /// prediction.
        // trace:STORY-439 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long = "assist-est", value_enum, value_name = "LEVEL")]
        assist_est: Option<crate::complexity_calibration::AssistanceLevel>,
        /// Pickup-time effort estimate: 15m, 1h, 4h, 1d, or 1w.
        /// Captured as the `plan` touchpoint for queued work pickup
        /// (post-plan/implementation-brief estimate). `1d` is 8
        /// work-hours; `1w` is 40 work-hours.
        // trace:STORY-451 | ai:codex
        #[clap(long, value_enum, value_name = "BUCKET")]
        effort: Option<crate::effort_calibration::EffortBucket>,
        /// Refuse auto-queuing Approved-but-not-queued specs. Under --strict,
        /// aida queue work refuses with the status-aware recovery hint.
        // trace:TASK-547 | ai:antigravity
        #[clap(long)]
        strict: bool,
    },
    /// Show what an active session has shipped so far alongside what
    /// remains. Bucketed view (Shipped / In flight / Working now /
    /// Remaining) with a net summary line. Default resolves to the
    /// most-recent active session; `--session ID` targets a specific
    /// lease; `--batch NAME` reads members of a `batch:NAME` tag instead
    /// of a session manifest.
    ///
    /// Buckets:
    ///   Shipped     — status Completed/Rejected (merged to default branch)
    ///   In flight   — status Done (work finished on a branch; PR open)
    ///   Working now — status InProgress
    ///   Remaining   — Approved/Planned/Draft (still queued)
    ///
    // trace:TASK-232 | ai:claude
    Progress {
        /// Specific session id (8+ char prefix) to read the manifest of.
        /// Default: the lease covering cwd, else the most-recent active
        /// lease.
        // trace:TASK-232 | ai:claude
        #[clap(long, value_name = "SESSION_ID")]
        session: Option<String>,
        /// Resolve items from a `batch:NAME` tag instead of a session
        /// manifest. Composes with the batch-drain convention: items
        /// tagged `batch:NAME` (set via `aida add --tags` or
        /// `aida edit --tags`) are members of the batch.
        /// Mutually exclusive with `--session`.
        // trace:TASK-232, TASK-229 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "NAME", conflicts_with = "session")]
        batch: Option<String>,
        /// Filter items to those modified after this timestamp
        /// (RFC3339 or `<N>{d,h,m}` ago, e.g. `2d`, `12h`). Useful for
        /// "what changed since yesterday" snapshots when no manifest
        /// or batch tag applies.
        // trace:TASK-232 | ai:claude
        #[clap(long, value_name = "TS")]
        since: Option<String>,
        /// Show every member by id under each bucket, not just the
        /// first few. Default truncates Remaining at 8 items with a
        /// `…and N more` tail.
        // trace:TASK-232 | ai:claude
        #[clap(long)]
        verbose: bool,
    },
    /// Flip a spec's status, route it to a role's queue, and (optionally)
    /// launch a session — encapsulates the recurring implementer →
    /// reviewer → fixup recovery sequence into a single verb. Replaces
    /// the manual three-step:
    ///
    ///   aida edit SPEC --status in-progress
    ///   aida queue add SPEC --for ROLE
    ///   aida queue work SPEC
    ///
    /// Smart status transitions (overridable with `--status`):
    ///   Approved   → no flip (just queue)
    ///   Planned    → InProgress
    ///   InProgress → no flip, refuse re-queue without --force
    ///   Done       → InProgress (typical PR-review-found-issues case)
    ///   Completed  → InProgress, requires --force (terminal-status guard)
    ///   Rejected   → Approved, requires --force
    ///
    // trace:TASK-218 | ai:claude
    Rework {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// Also launch a session for the spec (chains `aida queue work`).
        /// Without this, rework is metadata-only: status flip + queue add.
        #[clap(long)]
        work: bool,
        /// Override the routing role. Default: active role
        /// (`AIDA_SESSION_ROLE`), matching `aida queue add` semantics.
        #[clap(long)]
        r#for: Option<String>,
        /// Override the smart target status. Pass any status string
        /// (`in-progress`, `approved`, `planned`, …); the smart table
        /// above is bypassed.
        #[clap(long, value_name = "STATE")]
        status: Option<String>,
        /// Capture a comment on the spec at rework time (added via the
        /// same path as `aida comment add`). Useful for the audit trail
        /// of why a Done or Completed spec is being re-opened.
        #[clap(long)]
        reason: Option<String>,
        /// Chain `aida queue work --resume` to resume a prior claude
        /// session for this spec instead of cold-launching. Implies
        /// `--work`.
        #[clap(long)]
        resume: bool,
        /// Bypass the terminal-status guard (Completed/Rejected) and
        /// the "already in progress" guard. Mirrors `queue add --force`.
        // trace:TASK-45 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        force: bool,
        /// When chaining `--work`, end any session holding the target
        /// scope first (passes through to `aida queue work --steal`).
        // trace:TASK-81 | ai:claude
        #[clap(long)]
        steal: bool,
        /// Permission mode (only used with `--work`; passed to
        /// `aida queue work --permission-mode`).
        #[clap(long, value_name = "MODE")]
        permission_mode: Option<String>,
        /// Skip the pre-pickup `aida db sync --pull` (only used with
        /// `--work`; passed to `aida queue work --no-pull`).
        #[clap(long)]
        no_pull: bool,
        /// User ID (defaults to AIDA_USER or system user).
        #[clap(long)]
        user: Option<String>,
    },
    /// Recover a spec from a failed phase-1 implementer session — an
    /// interactive wizard over the existing recovery primitives.
    ///
    /// After a phase-1 failure (an Anthropic 529, a commit-and-exit without a
    /// PR, partial work, an external crash), recovery is a mechanical sequence
    /// of git/gh/aida commands whose exact shape depends on the spec's state:
    /// does a lease still hold it? is there an open/merged PR? are there commits
    /// ahead of `origin/main` that were never pushed? is the worktree dirty?
    /// This command inspects that state, recommends a recovery path, and steps
    /// through it interactively — instead of you remembering the dance each time.
    ///
    /// It is a FRONT-END over existing primitives, not new mechanism: it reuses
    /// the same lease probes as `aida session leases`, the same PR/branch probes
    /// the orchestrator uses, drives phases 3-6 via the PR-only orchestrator
    /// path when a PR is already open, and falls back to `aida pull` / session
    /// cleanup / re-queue for the other cases. Recommended actions:
    ///   • open PR (pushed)        → drive phases reviewer → merge → pull → build
    ///   • commits, not pushed     → push + open PR + drive phases
    ///   • commits + dirty worktree→ commit WIP, then push + PR + drive
    ///   • no commits + dirty      → commit WIP and park for resumption
    ///   • no commits + clean      → end the lease and re-queue
    ///   • PR merged / spec done   → pull / nothing to do (already shipped)
    Recover {
        /// Spec to recover (UUID or SPEC-ID).
        id: String,
        /// Print the inspection result + recommended recovery plan WITHOUT
        /// executing anything. The safe way to see the state read before acting.
        #[clap(long)]
        dry_run: bool,
        /// Skip all confirmation prompts and run the recommended path
        /// non-interactively (for scripted / headless use). NOT the default.
        #[clap(long, visible_alias = "yes")]
        auto: bool,
        /// User ID (defaults to AIDA_USER or system user).
        #[clap(long)]
        user: Option<String>,
    },
    /// Integrate finished work: serialize the back-end merge phases over every
    /// spec that's Done with an open PR.
    ///
    /// This is the consumer half of a producer/consumer split. Implementers
    /// (the producers, working in parallel on isolated scopes) finish their
    /// work, flip a spec to Done, and leave an open PR — they never merge. This
    /// command (the single, serial consumer) watches for that pair — Done + open
    /// PR — and drives the remaining phases (reviewer → CI → merge → pull →
    /// build) on each in turn, ONE AT A TIME. Merging into the default branch is
    /// a shared, serial resource, so there is exactly one merge authority.
    ///
    /// The handoff is the substrate itself: the loop polls for the Done +
    /// open-PR state, no message bus needed. Each ready spec is driven through
    /// the same PR-only orchestrator path as `aida queue work <id>
    /// --auto-complete --from-pr` (implementation shipped outside this loop, so
    /// the implementer phase is skipped). A spec whose PR is already merged, or
    /// has no open PR, or whose PR probe is inconclusive (gh missing / auth /
    /// network) is reported and skipped — never re-driven, never guessed.
    Integrate {
        /// Inspect the ready-for-integration set and print what WOULD be driven,
        /// without merging anything. The safe way to see the loop's decision.
        #[clap(long)]
        dry_run: bool,
        /// Run a single pass over the ready set and exit (the default). Kept as
        /// an explicit flag for symmetry with --watch.
        #[clap(long)]
        once: bool,
        /// Keep watching: after each pass, BLOCK until a drain event lands (a PR
        /// is ready / CI reaches a verdict / a PR merges) and rescan, integrating
        /// newly-ready specs as producers ship them. Event-driven — it wakes on
        /// real activity, not a blind timer; the idle backstop only bounds the
        /// quiet-period rescan. Without this the command makes a single pass.
        #[clap(long)]
        watch: bool,
        /// Idle backstop (seconds) for --watch: the longest the loop waits with no
        /// drain event before a full rescan. Superseded by --idle-minutes when
        /// that is given. Default 60.
        #[clap(long, value_name = "SECS", default_value_t = 60)]
        interval: u64,
        /// Cap the number of specs integrated this run (across all passes). 0 =
        /// no cap. A guardrail for a first cautious run.
        #[clap(long, value_name = "N", default_value_t = 0)]
        max: usize,
        // trace:STORY-335 | ai:claude
        /// Rebase each member's PR branch onto current main before merging it.
        /// A deferred batch cuts every branch from the same stale main, so
        /// without this they merge un-rebased. Opt-in: composes `pr rebase`
        /// (force-push-with-lease); a rebase conflict skips that member and
        /// continues. Preview with --dry-run first.
        #[clap(long)]
        rebase: bool,
        // trace:STORY-335 trace:TASK-691 | ai:claude
        /// Accumulation strategy for the batch. `per-item` (default) = a branch
        /// + PR per item, rebased and merged in order. `one-branch` and
        ///   `stacked` are accepted but not built yet (they error with a pointer).
        ///   When omitted, falls back to `[integrate] strategy` in
        ///   .aida/config.toml, then `per-item`.
        #[clap(long, value_enum)]
        strategy: Option<crate::integrate::IntegrateStrategy>,
        // trace:TASK-1036 | ai:claude
        /// Scope the scan to a focus epic/spec and its transitive descendants —
        /// only PRs under that subtree are integrated. Overrides the per-worktree
        /// `aida focus` marker / AIDA_FOCUS for this run. Omit to use the active
        /// focus (if any), else scan the whole project.
        #[clap(long, value_name = "ID")]
        focus: Option<String>,
        // trace:TASK-1036 | ai:claude
        /// Idle backstop for `--watch`, in minutes: the longest the loop waits
        /// with no drain event before a full rescan. The loop is event-driven —
        /// it wakes immediately on a ready PR / CI verdict / merge — so this only
        /// bounds the quiet-period rescan. Defaults to `--interval` seconds.
        #[clap(long, value_name = "N")]
        idle_minutes: Option<u64>,
        // trace:STORY-647 | ai:claude
        /// Bypass the team RBAC guardrail (`[team.permissions] integrate`). The
        /// gate is a guardrail, not security — the bypass is recorded in history.
        #[clap(long)]
        force: bool,
        /// User ID (defaults to AIDA_USER or system user).
        #[clap(long)]
        user: Option<String>,
    },
}

/// `aida questions` — the async decision inbox. The advisor distills a fork
/// it can't resolve into a structured DecisionRequest on the spec; the human
/// batch-answers it here, OUTSIDE any agent (plain CLI, no LLM session).
/// Slice 1 records the answer (pure data op); the loop-resume auto-applier
/// that applies the chosen resolution token is deferred.
// trace:STORY-522 | ai:claude
// trace:TASK-780 | ai:claude
#[derive(Subcommand, Debug)]
pub enum QuestionsCommand {
    /// Run by: anyone, when: you want to see what's pending (read-only).
    ///
    /// List the decision inbox — every spec with a recorded DecisionRequest,
    /// pending ones first. The pure read; never prompts (scripting/scanning).
    List,

    /// Run by: advisor / automation, when: detecting which specs need a human
    /// decision before they're pickable (the detection pass).
    ///
    /// Sweep the scoped backlog for specs that are likely to need a human
    /// decision before implementation, then attach DecisionRequests for the
    /// flagged candidates that do not already have an open request.
    Sweep {
        /// Scope to sweep. Defaults to `backlog` (approved/planned/in-progress
        /// near-term work). Supported: backlog, approved, planned,
        /// in-progress, all. Low-priority and archived specs are always
        /// excluded.
        scope: Option<String>,
        // trace:TASK-700 | ai:claude
        /// Attach the DecisionRequests. Without this the sweep only PREVIEWS
        /// which specs would get one (it mutates specs, so writing is opt-in).
        #[clap(long)]
        apply: bool,
    },

    /// Run by: the advisor, when: it hits a fork it can't resolve and needs to
    /// pose the question to the human (not a human-run command).
    ///
    /// Pose a structured DecisionRequest on a spec. The advisor distills a
    /// fork into a self-contained question + enumerated choices, each mapping
    /// to a deterministic resolution token. Refuses if the spec already has a
    /// pending request unless --force. Requires at least two choices.
    Ask {
        /// The spec (UUID or SPEC-ID) the question is about.
        spec: String,

        /// The self-contained question (no spec re-read needed to answer).
        #[clap(long, short = 'q', value_name = "TEXT")]
        question: String,

        /// A choice, as `label|consequence|resolution`. Repeatable; supply at
        /// least two. `resolution` is a deterministic action token (e.g.
        /// `status:rejected`, `tag:+deferred:post-stability`, `noop`) — never
        /// free-form prose.
        #[clap(long, short = 'c', value_name = "LABEL|CONSEQUENCE|RESOLUTION")]
        choice: Vec<String>,

        /// 1-based index of the recommended default choice (stored 0-based).
        #[clap(long, value_name = "N")]
        recommend: Option<usize>,

        /// Why the recommended default is recommended.
        #[clap(long, value_name = "TEXT")]
        rationale: Option<String>,

        /// Overwrite an existing pending DecisionRequest on this spec.
        #[clap(long)]
        force: bool,
    },

    /// Run by: human + agent together (interactive), when: an under-specified
    /// spec needs acceptance criteria authored before it's pickable.
    ///
    /// Fire up an INTERACTIVE advisor that interrogates you to author
    /// acceptance criteria for under-specified specs — the human-side
    /// complement to `sweep` (sweep detects, clarify resolves). Launches
    /// `claude "/aida-clarify <specs>"` (interactive, not headless) seeded
    /// with the flagged specs and walks you through each until pickable.
    ///
    /// With no specs it defaults to the swept set (the same detection `sweep`
    /// uses), honouring the same exclusions: visions/folders/meta/principles/
    /// terms, already-built/held specs, and specs with an active lease.
    Clarify {
        /// The spec(s) to clarify. Omit to clarify the swept set.
        specs: Vec<String>,

        /// Show which specs would be clarified + the exact `claude` command,
        /// then exit without launching.
        #[clap(long)]
        dry_run: bool,
    },

    /// Run by: the human, when: draining pending decisions (no LLM session — a
    /// plain operator data op).
    ///
    /// Answer pending decisions — the HUMAN-decision drain, the symmetric
    /// complement of `aida burndown run` (which drains the decision-FREE ready
    /// set with autonomous agents). Answering APPLIES the chosen resolution:
    /// it binds a design decision into the spec's `## Acceptance`, clears a
    /// disposition gate, or rejects — then auto-queues the now decision-free
    /// spec onto the burndown ready set (advisor-gated; honest about any
    /// remaining blocker). No agent, no LLM session — a pure operator data op.
    ///
    /// `answer <spec> <choice>` records one answer non-interactively (choice
    /// is a 1-based number, or the word `default`/`recommended`).
    /// `answer` with no args enters the interactive loop over all pending
    /// (TTY) — the human-decision drain. `answer --all-defaults` confirms
    /// every recommended default at once.
    // trace:STORY-555 | ai:claude
    Answer {
        /// The spec (UUID or SPEC-ID) to answer. Omit for the interactive loop.
        spec: Option<String>,

        /// The chosen option: a 1-based number, or `default`/`recommended`.
        choice: Option<String>,

        /// Confirm the recommended default for every pending request that has
        /// one, in a single batch.
        #[clap(long)]
        all_defaults: bool,

        /// Attach a free-text counter-proposal recorded ALONGSIDE the chosen
        /// option (e.g. "name it list-claude-sessions"). A pure data op — no
        /// LLM at answer-time; the implementer reads choice + note later. The
        /// non-interactive form of the interactive prompt's "type something"
        /// escape; pair with <spec> <choice>.
        // trace:TASK-791 | ai:claude
        #[clap(long, value_name = "TEXT")]
        note: Option<String>,
    },
}

/// `aida findings` — triage findings filed by headless drain phases (the
/// reviewer's `from-review:` tag, the implementer's `from-implementer:`
/// tag) and the advisor seat's `from-advisor:` observations.
// trace:STORY-278, STORY-285 | ai:claude
// trace:TASK-487 | ai:claude
// trace:STORY-467 | ai:claude
#[derive(Subcommand, Debug)]
pub enum FindingsCommand {
    /// File an advisor observation as a finding awaiting triage. Capture a
    /// pattern you've spotted from a live session before the context decays;
    /// promote, dismiss, or recur it later. The note becomes the finding's
    /// description; the title is the first line of the note unless --title
    /// is given.
    // trace:STORY-467 | ai:claude
    Add {
        /// The observation body. Required — without a note the finding has
        /// nothing to triage. Use `-` to read from stdin.
        #[clap(long, value_name = "TEXT")]
        note: String,

        /// What kind of finding this is. Free-form so future kinds (e.g.
        /// `principle-candidate`, `architectural-concern`) ship without
        /// a code change; `observation` is the default and the only
        /// advisor kind today.
        #[clap(long, value_name = "KIND", default_value = "observation")]
        kind: String,

        /// One-line title. Defaults to the first line of `--note`,
        /// truncated to 80 characters.
        #[clap(long, value_name = "TEXT")]
        title: Option<String>,

        /// Severity — `major`, `minor`, or `cosmetic`. Absent → unknown
        /// (sorted last in the triage view).
        #[clap(long, value_name = "LEVEL")]
        severity: Option<String>,

        /// Comma-separated SPEC-IDs the observation is about. The first
        /// becomes the origin (`from-advisor:<SPEC>`); the rest get
        /// `linked:<SPEC>` tags. Omit to file as `from-advisor:general`.
        #[clap(long, value_name = "IDS", value_delimiter = ',')]
        linked_specs: Vec<String>,

        /// Extra tags, comma-separated. Added verbatim — use the same
        /// `aida:<subcommand>` colon-namespaced convention as elsewhere.
        #[clap(long, value_name = "TAGS")]
        tags: Option<String>,
    },

    /// Increment a finding's recurrence counter. Use when you spot the same
    /// pattern again — the counter survives in the `recurrence:N` tag and
    /// the optional `--note` appends to the audit trail. Recurrence ≥ 3 is
    /// the promote-it signal (see `docs/aida/discipline/observation-discipline.md`).
    // trace:STORY-467 | ai:claude
    Recur {
        /// The finding's ID (UUID or SPEC-ID).
        id: String,

        /// What you saw this time. Appended as a timestamped audit comment.
        #[clap(long, value_name = "TEXT")]
        note: Option<String>,
    },

    /// List draft findings awaiting triage, grouped by source then origin and
    /// severity-sorted.
    List {
        /// Narrow to review findings raised against one PR.
        #[clap(long, value_name = "N")]
        pr: Option<u32>,

        /// Narrow to findings from one source: `review`, `implementer`, or
        /// `advisor`.
        #[clap(long, value_enum)]
        source: Option<crate::findings::FindingSource>,

        /// Narrow to findings carrying `kind:<value>` — implementer kinds
        /// (`deviation`, `design-choice`, `bug-spotted`,
        /// `followup-suggestion`) or advisor kinds (`observation`).
        #[clap(long, value_name = "KIND")]
        kind: Option<String>,

        /// Print just the pending-finding count (for session-start surfacing).
        #[clap(long)]
        count: bool,
    },

    /// Dismiss a finding — sets status Rejected and records an audit comment.
    Dismiss {
        /// The finding's ID (UUID or SPEC-ID).
        id: String,

        /// Rationale for the dismissal. When provided, the audit comment
        /// includes the text verbatim (e.g. *"Dismissed by joe 2026-05-20:
        /// duplicates an earlier filing"*) instead of the bare "Dismissed"
        /// marker — so the *why* lands in one command instead of two.
        // trace:TASK-404 | ai:claude
        // trace:TASK-420 | ai:claude
        #[clap(long, value_name = "TEXT")]
        reason: Option<String>,
    },

    /// Promote a finding — sets status Approved and adds it to a work queue.
    Promote {
        /// The finding's ID (UUID or SPEC-ID).
        id: String,

        /// Route the promoted finding to this role's work queue. Defaults
        /// to `implementer` — a promoted finding is implementation work.
        #[clap(long = "for", value_name = "ROLE")]
        r#for: Option<String>,

        /// Rationale for the promotion ("more important than its priority
        /// suggests", "blocks another open PR", etc.). When provided, the
        /// rationale is appended as an audit comment so the *why* survives
        /// alongside the queue note.
        // trace:TASK-404 | ai:claude
        // trace:TASK-420 | ai:claude
        #[clap(long, value_name = "TEXT")]
        reason: Option<String>,

        /// When the finding's origin-ID fix has already merged to the default
        /// branch, bump the promoted spec straight to Completed instead of
        /// queueing it as fresh work. Without this flag, an already-merged
        /// origin-ID only prints a warning and the finding still queues.
        // trace:TASK-579 | ai:claude
        #[clap(long = "auto-complete")]
        auto_complete: bool,

        /// Promote (and queue) even when the origin-ID fix already merged —
        /// for reopening or extending the finding. Suppresses the
        /// already-merged warning / auto-complete entirely.
        // trace:TASK-579 | ai:claude
        #[clap(long)]
        force: bool,
    },

    /// Calibration review surface — list cold-boot vs fork-from-live
    /// advisor verdicts side-by-side. Default view shows disagreements
    /// (the rows that name a substrate gap); `--all` widens to agreements
    /// too, and `--stats` swaps the table for the rolling metric.
    /// Annotate one row with the `annotate` sub-action.
    // trace:STORY-347 | ai:claude
    Calibration {
        #[clap(subcommand)]
        action: Option<CalibrationAction>,

        /// Restrict to records within the window. Form: `<N>{d,h,w,m}` —
        /// e.g. `7d`, `12h`, `2w`, `30m`.
        #[clap(long, value_name = "WINDOW")]
        since: Option<String>,

        /// Show only records where the two advisors agreed.
        #[clap(long, conflicts_with_all = ["disagreement", "all"])]
        agreement: bool,

        /// Show only records where the two advisors disagreed. The default
        /// (no flag) is equivalent — disagreements are the triage signal.
        #[clap(long, conflicts_with_all = ["agreement", "all"])]
        disagreement: bool,

        /// Show every record (agreements + disagreements + no-fork rows).
        #[clap(long, conflicts_with_all = ["agreement", "disagreement"])]
        all: bool,

        /// Print the rolling-metric summary (agreement rate over the last N
        /// records, 4-week trend, annotation-category histogram) instead
        /// of the per-record table.
        #[clap(long)]
        stats: bool,

        /// Number of records the `--stats` agreement rate considers.
        /// Default 50.
        #[clap(long, value_name = "N", default_value_t = 50)]
        last: usize,

        /// Emit a machine-readable JSON view instead of the human table.
        #[clap(long)]
        json: bool,
    },
}

// `aida findings calibration <ACTION>` — sub-action of the calibration
// review surface. The base verb (`aida findings calibration` with no
// action) lists records; the `annotate` action attaches a one-line note
// to a specific punt-id. trace:STORY-347 | ai:claude
#[derive(Subcommand, Debug)]
pub enum CalibrationAction {
    /// Annotate a calibration record. The note is a one-line triage hint;
    /// the prefix categories `gap → wrote memory <name>`, `inherently
    /// in-flight, accept`, and `cold-boot was actually correct` feed the
    /// `--stats` histogram, but any text is accepted.
    Annotate {
        /// The punt-id (the `<SPEC>-<unix-seconds>` directory name under
        /// `.aida/punts/`).
        punt_id: String,
        /// The annotation text. Trimmed; stored verbatim.
        note: String,
    },
}

// trace:TASK-394 | ai:claude
/// Persist the one-time `--no-human` scope acknowledgement as a
/// file marker so an unattended loop doesn't re-prompt per iteration. The
/// pre-flight gate checks the marker in addition to AIDA_NO_HUMAN_ACKNOWLEDGED.
#[derive(Subcommand, Debug, Clone)]
pub enum NoHumanCommand {
    /// Persist the acknowledgement so future `--no-human` drains skip the
    /// scope prompt. Machine-wide (`~/.aida/no-human-acknowledged`) by default;
    /// `--project` scopes it to this repo (`.aida/no-human-acknowledged`).
    Acknowledge {
        /// Scope the acknowledgement to this project instead of the machine.
        #[clap(long)]
        project: bool,
    },
    /// Remove the persistent acknowledgement (the scope prompt returns).
    Revoke {
        /// Revoke the project-scoped marker instead of the machine-wide one.
        #[clap(long)]
        project: bool,
    },
    /// Show whether `--no-human` is currently acknowledged and via which channel.
    Status,
}

/// Three-way complexity-calibration views (pickup vs ship vs reviewer).
/// The parent `aida autonomy` namespace is shared with an eventual `report`
/// subcommand; this adds only the calibration surface so the two land cleanly
/// side-by-side.
// trace:STORY-439 trace:TASK-340 | ai:claude
#[derive(Subcommand, Debug, Clone)]
pub enum AutonomyCommand {
    /// Calibration views over `.aida/complexity-calibration/`.
    // trace:STORY-439 | ai:claude
    #[clap(subcommand)]
    Calibration(CalibrationSubcommand),
    /// Human-intervention maturity report: how many times drains had to stop
    /// and ask a human, rolled up per day so the trend is readable. The
    /// count trending toward zero as the autonomy machinery matures is the
    /// honest maturity signal.
    // trace:TASK-340 | ai:claude
    Report {
        /// Cap the number of dated rows printed (newest first). Default 30.
        #[clap(long, value_name = "N", default_value_t = 30)]
        last: usize,
        /// Emit a machine-readable JSON object instead of the human table.
        #[clap(long)]
        json: bool,
    },
}

/// Subcommands under `aida autonomy calibration`.
// trace:STORY-439 | ai:claude — plain `//` keeps the marker out of `--help`.
#[derive(Subcommand, Debug, Clone)]
pub enum CalibrationSubcommand {
    /// Surface specs where pickup-predicted complexity diverged most
    /// from reviewer-assessed implementation complexity. The
    /// substrate-gap signal — a class of work the agents systematically
    /// misjudge. Records ranked by biggest gap first; ties broken by
    /// recency.
    // trace:STORY-439 | ai:claude
    Mismatches {
        /// Restrict to records within the window. Form: `<N>{d,h,w,m}`
        /// — e.g. `7d`, `12h`, `2w`, `30m`. Filters by the newest
        /// timestamp across the three slots.
        #[clap(long, value_name = "WINDOW")]
        since: Option<String>,
        /// Cap the rows printed. Default 50.
        #[clap(long, value_name = "N", default_value_t = 50)]
        last: usize,
        /// Emit a machine-readable JSON array instead of the human table.
        #[clap(long)]
        json: bool,
    },
}

/// Quantitative effort/load views over `.aida/effort-calibration/`.
// trace:STORY-451 | ai:codex
#[derive(Subcommand, Debug, Clone)]
pub enum LoadCommand {
    /// Sum latest effort estimates for queued items.
    Queue,
    /// Sum latest effort estimates for approved/planned/draft backlog items
    /// that are not currently queued.
    Backlog,
    /// Queue + backlog + in-flight effort summary.
    Report,
    /// Estimate-vs-actual effort deltas. `1d` is 8 work-hours; `1w`
    /// is 5 work-days / 40 work-hours.
    Calibration {
        /// Restrict to records within the window. Form: `<N>{d,h,w,m}`
        /// — e.g. `7d`, `12h`, `2w`, `30m`.
        #[clap(long, value_name = "WINDOW")]
        since: Option<String>,
        /// Group deltas by requirement type.
        #[clap(long)]
        by_type: bool,
        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
}

/// Browse and analyze the local design-fork punt ledger.
// trace:STORY-325 | ai:claude
#[derive(Subcommand, Debug, Clone)]
pub enum PuntsCommand {
    /// Browse recent punt records from the ledger
    List {
        /// Filter by spec (display ID or UUID)
        #[clap(long)]
        spec: Option<String>,

        /// Filter by category (e.g. design-fork, ambiguous-spec, missing-context)
        #[clap(long, short = 'c')]
        category: Option<String>,

        /// Filter by resolution path (e.g. punted, advisor-resolved, escalated-to-human)
        #[clap(long, short = 'r')]
        resolution: Option<String>,
    },

    /// Print rolling metrics and stats for recent punts
    Analyze,

    /// Extract a pattern from a punt into a new memory pack entry
    Promote {
        /// The spec ID of the punt record to promote
        id: String,

        /// The name of the memory file (e.g., 'discipline/operator_curation.md')
        memory_name: String,
    },
}

/// Orchestrator-run introspection. The `--auto-complete` orchestrator passes a
/// corroboration token to its phase children; these subcommands let a child —
/// or a skill running inside one — verify whether it is a *genuine* orchestrator
/// child rather than guessing from a bare `AIDA_AUTO_COMPLETE` env var.
// trace:BUG-233 | ai:claude
#[derive(Subcommand, Debug)]
pub enum OrchestratorCommand {
    /// Print the corroborated orchestrator context of the current process:
    /// `orchestrated` when `AIDA_AUTO_COMPLETE` is set AND its
    /// `AIDA_AUTO_COMPLETE_TOKEN` names a live orchestrator run, else
    /// `interactive`. Skills branch their orchestrator-aware behavior off this
    /// word instead of trusting the bare env var.
    Status {
        /// Emit `{"context","corroborated","reason"}` JSON instead of the
        /// bare status word.
        #[clap(long)]
        json: bool,
    },
}

/// Zen-mode introspection. `AIDA_ZEN=1` enables zen mode but is
/// inherited by every child process — a leaked / stale value is unverifiable
/// on its own. These subcommands let a session — or a skill running inside one
/// — verify whether zen mode is *genuinely* in effect rather than trusting the
/// bare env var.
///
/// Every variant is `hide = true`: these are skill / orchestrator introspection
/// hooks, not daily-driver verbs, so they stay OFF the `aida zen --help` top
/// line (which leads with `aida zen <SPEC>` / `aida zen "<thought>"`). They
/// still parse + run.
// trace:STORY-725 | ai:claude — plain `//` keeps the marker out of `--help`.
// trace:BUG-237 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum ZenCommand {
    /// Print the corroborated zen context of the current process: `zen` when
    /// `AIDA_ZEN=1` is set AND corroborated (a live `--auto-complete --zen`
    /// orchestrator run, or the session's own `--zen` lease token), else
    /// `interactive`. Skills branch their zen behavior off this word instead
    /// of reading the bare `$AIDA_ZEN` env var, so a leaked `AIDA_ZEN=1` never
    /// silently auto-resolves a confirmation prompt.
    #[clap(hide = true)]
    Status {
        /// Emit `{"context","corroborated","reason"}` JSON instead of the
        /// bare status word.
        #[clap(long)]
        json: bool,
    },
    /// The standalone-`--zen` finish decision. Prints `auto-exit` when this
    /// zen session reached a clean finish — no human ever in the loop, no open
    /// punt, no `--pause-always` — so the `/aida-pr` checkpoint runs
    /// `aida session end` itself and exits; prints `pause` otherwise so it
    /// renders the grab-next/stop table. The gate the skill consults instead
    /// of always pausing.
    // trace:STORY-564 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(hide = true)]
    Finish {
        /// Emit `{"decision","reason","corroborated"}` JSON instead of the
        /// bare decision word.
        #[clap(long)]
        json: bool,
    },
    /// Mark the current `--zen` session as having needed a human — call it the
    /// moment the session pauses on a design-fork for the standby advisor or
    /// raises a punt. Its presence makes `aida zen finish` pause at the
    /// grab-next/stop checkpoint rather than auto-exiting.
    // trace:STORY-564 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(hide = true)]
    NeedsHuman {
        /// One line on why a human was needed (for later triage). Only the
        /// marker's presence drives the gate; the reason is recorded alongside.
        #[clap(long, value_name = "TEXT")]
        reason: String,
    },
}

/// Live-advisor registration. The `--no-human=both` orchestrator reads
/// `~/.aida/advisor.toml` to decide whether to **fork** the live advisor
/// session (full in-flight context) instead of **cold-booting** a fresh
/// headless advisor (substrate-only). Register once per advisor session;
/// `unregister` clears the file.
// trace:STORY-360 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum AdvisorCommand {
    /// Record the current Claude session as the live advisor — writes
    /// `~/.aida/advisor.toml`. With no `--uuid`, reads `CLAUDE_CODE_SESSION_ID`
    /// from the environment (Claude Code sets it on every session). Pass
    /// `--uuid <id>` to register a specific session id.
    Register {
        /// Claude session UUID. Defaults to `$CLAUDE_CODE_SESSION_ID`.
        #[clap(long, value_name = "UUID")]
        uuid: Option<String>,

        /// Override the cwd-derived project slug. Rare — the default is
        /// derived from the current directory the same way Claude Code
        /// encodes it under `~/.claude/projects/<slug>/`.
        #[clap(long, value_name = "SLUG")]
        project_slug: Option<String>,
    },

    /// Clear the registration. Idempotent — succeeds even when nothing is
    /// registered.
    Unregister,

    /// Presence-gated fork-from-live watch loop: while `away`, periodically fork
    /// the live advisor session and run a garden + mailbox-triage + escalate
    /// pass headless. Opt-in by invocation; exits on `aida home` (or away-TTL).
    /// The forked advisor only does safe/bounded work and escalates the rest.
    // trace:STORY-586 | ai:claude
    Watch {
        /// Preview each tick's decision (and the fork cost) without forking.
        #[clap(long)]
        dry_run: bool,

        /// Run a single tick and exit (cron-friendly).
        #[clap(long)]
        once: bool,

        /// Conservative mode: the forked advisor still gardens but only
        /// surfaces/escalates mailbox items (never acts on a request).
        #[clap(long)]
        triage_only: bool,

        /// Seconds between presence re-checks (default 60).
        #[clap(long, default_value_t = 60)]
        poll_interval: u64,

        /// Seconds between forks while away (default 1200 = 20m).
        #[clap(long, default_value_t = 1200)]
        fork_interval: u64,
    },

    /// The advisor's read-only situational dashboard — one screen of counts,
    /// each row pointing at the canonical command to act on it: live-advisor
    /// readiness, intake drafts, pending decisions, findings, backlog,
    /// burndown readiness, queue depth, and live sessions. Pure aggregation of
    /// existing surfaces; writes nothing.
    Status {
        /// Emit the dashboard as JSON instead of the human summary.
        #[clap(long)]
        json: bool,

        /// Show only the narrow live-advisor registration block (the
        /// pre-dashboard output) — back-compat for muscle memory and scripts.
        #[clap(long)]
        registration: bool,
    },

    /// Recurring maintenance/research tasks that land in the queue on a
    /// cadence. No daemon: due schedules are evaluated and fired on every
    /// `aida pull`. Manage them with the subcommands below.
    // trace:STORY-262 | ai:claude
    #[clap(subcommand)]
    Schedule(ScheduleCommand),

    /// Generate a checked-in advisor handoff brief for a sibling project.
    /// Writes a dated, structured Markdown brief at
    /// `<to>/docs/<date>-advisor-handoff.md` with five sections — parent
    /// identity (auto), vision, decided things, substrate slice (scoped by
    /// `--focus`), and latitude. The brief is a template: parent identity
    /// and the focus topic are filled in automatically; the strategic
    /// sections are placeholders the operator authors before committing it
    /// to the child project.
    // trace:STORY-363 | ai:claude
    Handoff {
        /// Path to the sibling/child project the brief is for. The brief is
        /// written under `<to>/docs/`; the directory is created if absent.
        #[clap(long, value_name = "PROJECT")]
        to: std::path::PathBuf,

        /// The focus topic for this handoff — names the slice of advisor
        /// context the child project inherits (e.g. "git-canonical store",
        /// "orchestrator drain"). Used in the brief title and the substrate
        /// section heading.
        #[clap(long, value_name = "TOPIC")]
        focus: String,

        /// Overwrite an existing brief for the same date instead of
        /// refusing. By default a same-day brief is preserved.
        #[clap(long)]
        force: bool,
    },
}

/// No-daemon scheduled-task management. A schedule is a recurring task
/// template with a cadence; when due (cadence elapsed since the last fire)
/// `aida pull` files a fresh TASK into the target role's queue, tagged
/// `scheduled:<name>`. Storage is local at `.aida/schedules.toml`.
// trace:STORY-262 | ai:claude
#[derive(Subcommand, Debug)]
pub enum ScheduleCommand {
    /// Register a recurring task. The schedule fires immediately on the next
    /// `aida pull` (never-fired schedules are due now), then on every cadence
    /// boundary thereafter.
    Add {
        /// Unique short name (the registration key). Becomes the
        /// `scheduled:<name>` tag on every filed TASK.
        name: String,

        /// Cadence: a Go-duration-like token (`90d`, `14d`, `30d`, `12h`,
        /// `1w`). Units: m=minutes, h=hours, d=days, w=weeks.
        #[clap(long)]
        every: String,

        /// TASK title filed when the schedule fires.
        #[clap(long)]
        template: String,

        /// TASK description (optional).
        #[clap(long)]
        description: Option<String>,

        /// Extra comma-separated tags applied to the filed TASK (in addition
        /// to `scheduled:<name>` and `batch:scheduled`).
        #[clap(long)]
        tags: Option<String>,

        /// Role the filed TASK is routed to. Defaults to `advisor`.
        #[clap(long = "for", default_value = "advisor")]
        for_role: String,
    },

    /// List registered schedules with cadence, last-fired, next-due, status.
    List {
        /// Emit the schedule list as JSON instead of the human table.
        #[clap(long)]
        json: bool,
    },

    /// Enable a disabled schedule so it resumes firing on cadence.
    Enable {
        /// Schedule name.
        name: String,
    },

    /// Disable a schedule. It's preserved but never fired until re-enabled.
    Disable {
        /// Schedule name.
        name: String,
    },

    /// Remove a schedule entirely.
    Remove {
        /// Schedule name.
        name: String,
    },

    /// Manually fire due schedules now (the same logic `aida pull` runs). With
    /// a name, force-fire that one schedule regardless of cadence.
    Run {
        /// Optional schedule name. Without it, fire all currently-due
        /// schedules. With it, force-fire that schedule even if not yet due.
        name: Option<String>,
    },
}

/// Worker-directive introspection. The `aida-worker` shell function reads
/// pending directives from `.aida/worker.cmd` (a FIFO — one directive
/// per line); this subcommand lists them so a user can see "what will the
/// worker do next?" without `cat`ing a runtime file. Pairs with the
/// `aida-worker` function emitted by `aida dev shell-init`.
// trace:TASK-294 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum WorkerCommand {
    /// List pending directives from `.aida/worker.cmd` in FIFO order — the
    /// next directive the `aida-worker` shell function will act on is at the
    /// top. Prints `No pending directives.` (exit 0) when the file is empty
    /// or absent.
    Directives {
        /// Emit the directive list as JSON instead of the human summary.
        #[clap(long)]
        json: bool,
    },
}

// Headless-log introspection. A `--no-human` drain writes one JSONL file per
// child session under `.aida/headless-logs/`; this subcommand wraps the right
// block-type filtering so the user gets a clean text stream instead of the
// pages of `null` a naive `jq` filter produces.
// trace:TASK-398 | ai:claude
#[derive(Subcommand, Debug)]
pub enum HeadlessCommand {
    /// Stream a headless-drain JSONL log as clean text — one block of
    /// assistant prose per message, blank line between messages. No daemon,
    /// no parsing into cache.db. With no argument, picks the most-recently-
    /// modified log under `.aida/headless-logs/`; pass a SPEC-ID to pick
    /// the newest log for that spec, or a lease/session id prefix
    /// (e.g. `019e4400`) to disambiguate between concurrent drains.
    // trace:TASK-487 | ai:claude
    Tail {
        /// SPEC-ID (e.g. `TASK-N`) or lease/session id prefix. Optional —
        /// when omitted, the most-recently-modified log is followed.
        target: Option<String>,

        /// List every log file under `.aida/headless-logs/` (kind, spec,
        /// mtime, size, filename) and exit. Useful for picking a lease id
        /// when multiple drains have touched the same spec.
        #[clap(long, conflicts_with_all = ["target", "with_tools", "tools_only", "include_user", "no_follow", "since"])]
        list: bool,

        /// Interleave tool invocations as `[ToolName] <preview>` lines,
        /// alongside the assistant text. Off by default — most users only
        /// want the narration.
        #[clap(long)]
        with_tools: bool,

        /// Print only tool invocations, no assistant text. Useful for
        /// watching what commands the implementer is running.
        #[clap(long, conflicts_with = "list")]
        tools_only: bool,

        /// Also surface `type=="user"` messages (tool_result payloads). Off
        /// by default — headless mode rarely has interactive user content,
        /// and tool_results add a lot of noise.
        #[clap(long)]
        include_user: bool,

        /// Print existing content and exit instead of staying attached with
        /// `tail -f`-style polling. Useful for piping into a pager.
        #[clap(long, short = 'n')]
        no_follow: bool,

        /// Skip entries older than the given duration (e.g. `10m`, `2h`,
        /// `1d`). Bare integers are interpreted as seconds. Compared
        /// against `timestamp` fields when the event carries one; events
        /// without a timestamp (assistant messages, system events) pass
        /// through unfiltered.
        #[clap(long, value_name = "DURATION")]
        since: Option<String>,
    },
}

/// Drain-state introspection. An `aida queue work --auto-complete`
/// orchestrator writes `.aida/drain-state.json` while a drain is live; this
/// subcommand reads it so a user inside the orchestrator-spawned session can
/// see what command launched the drain, whether it is a single-spec or a batch
/// drain, how far through it is, and what happens when they exit the session.
// trace:STORY-301 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DrainCommand {
    /// Show the active `--auto-complete` drain: the launching command, the
    /// members and their lifecycle state, the current phase, and a prediction
    /// of what happens on session exit. Prints `No drain in progress.` (exit
    /// 0) when none is running. A drain-state file whose orchestrator process
    /// is no longer alive is reported as stale — `--clear` removes it.
    Status {
        /// Emit the drain state as JSON instead of the human summary.
        #[clap(long)]
        json: bool,
        /// Remove a stale drain-state file (one whose orchestrator process is
        /// no longer running). Refuses while the drain is still live — a live
        /// orchestrator removes the file itself on exit.
        #[clap(long)]
        clear: bool,
    },
}

// trace:STORY-248 | ai:claude
/// Stack-graph introspection. `aida queue work --stack /
/// --base` records each stacked branch's parent in `.aida/stacks.json`;
/// `aida stack show` renders the chain tree; `aida stack list` prints one
/// chain per line for scripting.
// trace:STORY-248 | ai:claude
#[derive(Subcommand, Debug)]
pub enum StackCommand {
    /// Render the stack tree — one indented line per branch, grouped by
    /// chain. Stale entries (branch missing locally + on origin) are
    /// suffixed with `(stale)` and can be cleaned by passing
    /// `--prune-stale`.
    Show {
        /// Emit the chains as JSON instead of the indented tree.
        #[clap(long)]
        json: bool,
        /// Remove stack entries whose branch no longer exists locally or
        /// on origin. Idempotent — safe to re-run.
        #[clap(long)]
        prune_stale: bool,
    },
    /// One line per chain in the form `bottom → mid → top`. Quieter than
    /// `show`; useful for scripts and the statusline.
    List {
        /// Emit the chains as JSON instead of the arrow form.
        #[clap(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum BriefCommand {
    /// List pending brief files.
    List {
        /// Only list briefs routed to this agent.
        #[clap(long = "for-agent")]
        for_agent: Option<String>,

        /// Include acknowledged briefs.
        #[clap(long)]
        include_acked: bool,
    },

    /// Mark a brief acknowledged so it stops appearing in default lists.
    Ack {
        /// Brief file path, either absolute or relative to the current directory.
        brief_file: PathBuf,
    },

    /// Read a brief file and print its contents to stdout.
    Read {
        /// Brief file path, shortcut (<agent>/<filename>), or agent name.
        brief_file: String,

        /// Read the most recent unacked brief for the given agent.
        #[clap(long)]
        latest: bool,
    },
}

/// Launch and track external AI agent processes under AIDA supervision.
// trace:STORY-432 | ai:codex
#[derive(Subcommand, Debug)]
pub enum AgentCommand {
    /// Launch a new agent process.
    ///
    /// This lane spawns a one-shot agent that does its work, ships a PR, and
    /// exits. It is NOT the orchestrated pipeline — it does not run CI, the
    /// reviewer phase, or the merge for you. For a supervised end-to-end drain
    /// (implementer → CI → reviewer → merge → pull), use
    /// `aida queue work <SPEC> --auto-complete` instead.
    // trace:TASK-626 | ai:claude — plain `//` keeps the marker out of `--help`.
    // trace:TASK-837 | ai:claude — the subcommand is optional so a bare
    // `aida agent new` opens an arrow-key agent-type picker instead of
    // erroring to clap help (mirrors `aida human`/`aida solo`).
    New {
        #[clap(subcommand)]
        command: Option<AgentNewCommand>,
    },

    /// Register an already-running raw-launched agent process.
    // trace:TASK-543 | ai:codex
    Register {
        /// PID of the already-running process to register.
        pid: u32,

        /// Agent type: codex, claude, antigravity, or web.
        #[clap(long = "type", value_name = "TYPE")]
        agent_type: String,

        /// Agent role: implementer, advisor, reviewer, or integrator.
        #[clap(long, value_name = "ROLE")]
        role: String,

        /// SPEC-ID currently owned by the agent.
        #[clap(long, value_name = "SPEC-ID")]
        spec: Option<String>,

        /// Optional human-readable instance name.
        #[clap(long)]
        name: Option<String>,
    },

    /// List active agent processes.
    // trace:TASK-542 | ai:antigravity
    Ls,

    /// Show active agent processes (alias of `ls`).
    // trace:STORY-528 | ai:claude
    Status,

    /// Set a dispatch marker (NOT a process pause): marks the agent busy
    /// (e.g. budget exhausted / rate-limited) so status and brief-time dispatch
    /// logic skip it. The process keeps running — use `aida agent stop` to
    /// terminate it.
    // trace:STORY-528 | ai:claude
    // trace:BUG-521 | ai:claude
    Pause {
        /// Agent name, `<type>#<pid>`, or `<type>-<pid>` id.
        agent: String,

        /// Why it is paused: budget, rate-limit, manual, or unknown.
        #[clap(long, value_name = "REASON", default_value = "manual")]
        reason: String,

        /// When the quota is expected to reset — an RFC3339 timestamp
        /// (2026-06-09T18:00:00Z) or a relative duration (2h, 90m, 45s).
        #[clap(long, value_name = "WHEN")]
        resets: Option<String>,
    },

    /// Clear an agent's paused state back to available.
    // trace:STORY-528 | ai:claude
    Resume {
        /// Agent name, `<type>#<pid>`, or `<type>-<pid>` id.
        agent: String,
    },

    /// Stop an active agent process by name.
    // trace:TASK-542 | ai:antigravity
    Stop {
        /// Name of the agent to stop.
        name: String,
    },

    /// List the supported agent roles in the taxonomy.
    // trace:TASK-587 | ai:antigravity
    #[clap(name = "list-roles")]
    ListRoles {
        /// Format output as JSON.
        #[clap(long)]
        json: bool,
    },
}

/// Agent-specific launchers. Each variant maps AIDA's supervision layer onto
/// the corresponding CLI's launch flags.
// trace:STORY-432 | ai:codex
#[derive(Subcommand, Debug)]
pub enum AgentNewCommand {
    /// Spawn Claude Code with project-correct cwd/env and registry tracking.
    Claude {
        /// Role propagated to the spawned Claude process as AIDA_SESSION_ROLE.
        #[clap(long)]
        role: Option<String>,

        /// SPEC-ID to work. Creates a scoped worktree + lease before launch.
        #[clap(long)]
        spec: Option<String>,

        /// Override the focus-scope drift guard: launch on a spec
        /// outside the active `aida focus` subtree even when `[focus]
        /// out_of_scope = "block"` would refuse, and silence the `warn` nudge.
        // trace:STORY-717 | ai:claude
        #[clap(long)]
        force: bool,

        /// AIDA project root or descendant used as the launch base.
        #[clap(long, value_name = "PATH")]
        cwd: Option<PathBuf>,

        /// Claude Code permission mode passed through to `claude`. When
        /// omitted, no `--permission-mode` is injected — Claude uses its
        /// native posture (the faithful default; turn on bypass fleet-wide
        /// with `[agents] bypass = true` in agents.toml).
        // trace:STORY-495 | ai:claude
        #[clap(long)]
        permission_mode: Option<String>,

        /// Launch Claude in contained mode: strict Bash sandboxing, no
        /// unsandboxed fallback, destructive-command deny rules, and
        /// project-relative edit auto-allow only.
        #[clap(long)]
        sandbox: bool,

        /// Do not write/inject the AIDA launch-context snapshot.
        #[clap(long)]
        no_context: bool,

        /// Print the generated launch-context snapshot before spawning.
        #[clap(long)]
        show_context: bool,

        /// Initial message to pass to the spawned Claude session.
        #[clap(long)]
        prompt: Option<String>,

        /// Do not auto-generate an initial message when --spec is supplied.
        #[clap(long)]
        no_prompt: bool,

        /// Do not read per-agent default flags from ~/.aida/agents.toml or .aida/agents.toml.
        #[clap(long)]
        no_default_flags: bool,

        /// Append one additional raw flag to the spawned Claude argv.
        #[clap(long = "extra-flag", value_name = "FLAG", allow_hyphen_values = true)]
        extra_flags: Vec<String>,

        /// Optional human-readable instance name.
        #[clap(long)]
        name: Option<String>,

        /// Dispatch to Claude Code's background supervisor via `claude
        /// --bg`. The session detaches from this terminal and shows up
        /// in `claude agents` (and `aida status`). When `--spec` is
        /// also passed, AIDA records the captured sessionId on the
        /// lease so the cross-substrate view links them automatically.
        /// Without `--bg`, the foreground launch path is unchanged.
        // trace:SPIKE-34 | ai:claude
        #[clap(long = "bg")]
        bg: bool,
    },

    /// Spawn Codex CLI with project-correct cwd/env and registry tracking.
    // trace:STORY-433 | ai:codex
    Codex {
        /// Role propagated to the spawned Codex process as AIDA_SESSION_ROLE.
        #[clap(long)]
        role: Option<String>,

        /// SPEC-ID to work. Creates a scoped worktree + lease before launch.
        #[clap(long)]
        spec: Option<String>,

        /// Override the focus-scope drift guard: launch on a spec
        /// outside the active `aida focus` subtree even when `[focus]
        /// out_of_scope = "block"` would refuse, and silence the `warn` nudge.
        // trace:STORY-717 | ai:claude
        #[clap(long)]
        force: bool,

        /// AIDA project root or descendant used as the launch base.
        #[clap(long, value_name = "PATH")]
        cwd: Option<PathBuf>,

        /// Pass Codex's unsafe autonomous-mode flag.
        ///
        /// This is opt-in by design. Interactive launches keep Codex's normal
        /// approval/sandbox behavior unless the operator explicitly asks for
        /// the empirical autonomous-drain posture used in prior dogfood runs.
        #[clap(long)]
        bypass_sandbox: bool,

        /// Do not write/inject the AIDA launch-context snapshot.
        #[clap(long)]
        no_context: bool,

        /// Print the generated launch-context snapshot before spawning.
        #[clap(long)]
        show_context: bool,

        /// Initial message to pass to the spawned Codex session.
        #[clap(long)]
        prompt: Option<String>,

        /// Do not auto-generate an initial message when --spec is supplied.
        #[clap(long)]
        no_prompt: bool,

        /// Do not read per-agent default flags from ~/.aida/agents.toml or .aida/agents.toml.
        #[clap(long)]
        no_default_flags: bool,

        /// Append one additional raw flag to the spawned Codex argv.
        #[clap(long = "extra-flag", value_name = "FLAG", allow_hyphen_values = true)]
        extra_flags: Vec<String>,

        /// Optional human-readable instance name.
        #[clap(long)]
        name: Option<String>,
    },

    /// Spawn Antigravity CLI with project-correct cwd/env and registry tracking.
    // trace:STORY-434 | ai:codex
    Antigravity {
        /// Role propagated to the spawned Antigravity process as AIDA_SESSION_ROLE.
        #[clap(long)]
        role: Option<String>,

        /// SPEC-ID to work. Creates a scoped worktree + lease before launch.
        #[clap(long)]
        spec: Option<String>,

        /// Override the focus-scope drift guard: launch on a spec
        /// outside the active `aida focus` subtree even when `[focus]
        /// out_of_scope = "block"` would refuse, and silence the `warn` nudge.
        // trace:STORY-717 | ai:claude
        #[clap(long)]
        force: bool,

        /// AIDA project root or descendant used as the launch base.
        #[clap(long, value_name = "PATH")]
        cwd: Option<PathBuf>,

        /// Pass Antigravity CLI's unsafe permission-skipping flag.
        ///
        /// This is opt-in by design. Interactive launches keep Antigravity's
        /// normal approval/sandbox behavior unless the operator explicitly
        /// asks for autonomous-drain posture.
        #[clap(long)]
        bypass_sandbox: bool,

        /// Do not write/inject the AIDA launch-context snapshot.
        #[clap(long)]
        no_context: bool,

        /// Print the generated launch-context snapshot before spawning.
        #[clap(long)]
        show_context: bool,

        /// Initial message to pass to the spawned Antigravity session.
        #[clap(long)]
        prompt: Option<String>,

        /// Do not auto-generate an initial message when --spec is supplied.
        #[clap(long)]
        no_prompt: bool,

        /// Do not read per-agent default flags from ~/.aida/agents.toml or .aida/agents.toml.
        #[clap(long)]
        no_default_flags: bool,

        /// Append one additional raw flag to the spawned Antigravity argv.
        #[clap(long = "extra-flag", value_name = "FLAG", allow_hyphen_values = true)]
        extra_flags: Vec<String>,

        /// Optional human-readable instance name.
        #[clap(long)]
        name: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Add a new requirement
    Add {
        /// Title, positional — the newcomer-friendly form: `aida add "Add a task
        /// from the CLI"`. Same as `--title`; if both are given, `--title` wins.
        // trace:TASK-725 | ai:claude
        title_positional: Option<String>,

        /// Title of the requirement
        #[clap(long)]
        title: Option<String>,

        /// Description of the requirement
        #[clap(long)]
        description: Option<String>,

        /// Read the description body from a file. Mutually exclusive with
        /// --description and --description-stdin.
        // trace:BUG-17 | ai:claude
        #[clap(long, conflicts_with_all = ["description", "description_stdin"])]
        description_from_file: Option<PathBuf>,

        /// Read the description body from stdin. Mutually exclusive with
        /// --description and --description-from-file.
        // trace:BUG-17 | ai:claude
        #[clap(long, conflicts_with_all = ["description", "description_from_file"])]
        description_stdin: bool,

        /// Status of the requirement (draft, approved, planned, in-progress,
        /// done, completed, rejected)
        #[clap(long)]
        status: Option<String>,

        /// Priority of the requirement (high, medium, low)
        #[clap(long)]
        priority: Option<String>,

        /// Type of requirement: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc
        #[clap(long)]
        r#type: Option<String>,

        /// Owner of the requirement (defaults to AIDA_AUTHOR env var or system user)
        #[clap(long)]
        owner: Option<String>,

        /// Feature the requirement belongs to (defaults to REQ_FEATURE env var or "Uncategorized")
        #[clap(long)]
        feature: Option<String>,

        /// Tags for the requirement (comma-separated)
        #[clap(long)]
        tags: Option<String>,

        /// Custom ID prefix override (uppercase letters only, e.g., SEC, PERF)
        #[clap(long)]
        prefix: Option<String>,

        /// Parent requirement ID (UUID or SPEC-ID) to link as child
        #[clap(long)]
        parent: Option<String>,

        // trace:STORY-446 | ai:claude
        /// SPEC-ID this new requirement is blocked by (repeatable; alias
        /// `--depends-on`). Creates the BlockedBy edge + the inverse Blocks
        /// edge atomically, so the pickability gate holds pickup until the
        /// blocker is Completed.
        #[clap(
            long = "blocked-by",
            visible_alias = "depends-on",
            value_name = "SPEC-ID"
        )]
        blocked_by: Vec<String>,

        /// Override the guard that refuses `--parent <X>` when X is in
        /// a terminal status (Completed/Rejected). Use when intentionally
        /// backfilling a forgotten child onto a closed epic for
        /// traceability.
        // trace:BUG-64 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        force_parent: bool,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,

        /// Mark this spec human-only so the orchestrator never auto-picks it.
        /// Spikes default to human-only automatically (research is human-driven);
        /// pass this on a non-Spike to opt in explicitly.
        // trace:TASK-130 | ai:claude
        #[clap(long, conflicts_with = "no_human_only")]
        human_only: bool,

        /// Force this spec to NOT be human-only even when its type would default
        /// to it (use on a Spike that is genuinely heads-down implementation).
        // trace:TASK-130 | ai:claude
        #[clap(long, conflicts_with = "human_only")]
        no_human_only: bool,

        /// Open-time effort estimate: 15m, 1h, 4h, 1d, or 1w. Captured
        /// to `.aida/effort-calibration/<SPEC>.yaml` and stamped as an
        /// `effort:open:<value>` tag. `1d` means 8 work-hours; `1w`
        /// means 5 work-days / 40 work-hours.
        // trace:STORY-451 | ai:codex
        #[clap(long, value_enum, value_name = "BUCKET")]
        effort: Option<crate::effort_calibration::EffortBucket>,

        /// File, approve, AND enqueue in one shot — places the new spec on the
        /// queue right after creating it, equivalent to a follow-up `aida
        /// backlog groom`. Only an Approved spec is enqueueable: pass with
        /// `--status approved` (the new spec must end up Approved, not draft).
        /// Refused the same way `aida queue add` is when the session lacks
        /// authority to commit work to the pipeline.
        // trace:TASK-754 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        queue: bool,

        /// When enqueuing with `--queue`, tag the queued spec `batch:NAME` so
        /// it composes with `aida queue work --batch NAME`. No effect without
        /// `--queue`.
        // trace:TASK-754 | ai:claude
        #[clap(long, value_name = "NAME")]
        batch: Option<String>,

        /// When enqueuing with `--queue`, route the spec to ROLE's queue
        /// (mirrors `aida queue add --for`). Defaults to `implementer` — the
        /// common case is an advisor filing implementation work — rather than
        /// the filer's own session role. Pass `--for advisor` to keep it on the
        /// advisor queue, or `--for any` to leave it unrouted. No effect without
        /// `--queue`.
        // trace:BUG-528 | ai:claude
        #[clap(long = "for", value_name = "ROLE")]
        r#for: Option<String>,
    },

    /// File a small change into the fasttrack lane in one shot
    ///
    /// Two tiers on one lane. The default **trivial** tier is the
    /// low-ceremony entry for genuinely trivial work — a doc tweak, a
    /// one-line UX papercut, a string fix. Files the spec Approved, queues
    /// it, and tags it for the fasttrack bucket (`batch:fasttrack` +
    /// `lifecycle:no-review`) so the human-review round-trip is skipped.
    ///
    /// The **express** tier (`--express`) is for an easy bug or a small
    /// single-purpose feature: same one-shot Approved + queued filing, but
    /// tagged `batch:express` and carrying NO lifecycle skip — so the full
    /// CI + reviewer + build gate runs. Fast because it is reliably routed,
    /// not because it is less gated.
    ///
    /// This is a thin wrapper over `aida add ... --status approved --queue`
    /// — it owns the lane's filing convention in one place so the
    /// `/aida-fasttrack` skill can call it instead of re-typing the tags.
    ///
    /// CI is NOT skipped in either tier. The trivial tier drops the
    /// human-review ceremony only; the express tier keeps every gate.
    // trace:TASK-777 | ai:claude — plain `//` keeps the marker out of `--help`.
    // trace:TASK-905 | ai:claude — `status` subcommand added below; the bare
    // `aida fasttrack <title>` filing form stays the default (title optional so
    // `aida fasttrack status` parses as the subcommand, not a titled file).
    // trace:STORY-692 | ai:claude — `--express` files batch:express + full gate.
    #[clap(args_conflicts_with_subcommands = true)]
    Fasttrack {
        /// One-line description of the small change (becomes the title).
        ///
        /// Omit it (and pass no subcommand) and you get a usage error; pass a
        /// subcommand instead (`aida fasttrack status`) for the lane view.
        #[clap(value_name = "TITLE")]
        title: Option<String>,

        /// Requirement type: use `bug` for a papercut/defect, `task` for a
        /// chore or doc tweak. Defaults to `task`.
        #[clap(long, default_value = "task")]
        r#type: String,

        /// File into the express tier: an easy bug or small feature that gets
        /// the FULL CI + reviewer + build gate (tagged `batch:express`, no
        /// lifecycle skip), rather than the trivial tier's review-skipped
        /// default. Fast because reliably routed, not because less gated.
        // trace:STORY-692 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        express: bool,

        /// Lane-introspection subcommand (e.g. `status`). When absent, the
        /// positional title files a small change as before.
        #[clap(subcommand)]
        command: Option<FasttrackCommand>,
    },

    /// List all requirements
    List {
        /// Optional positional shortcut (e.g. `aida list approved`).
        ///
        /// A positional alternative to `--status` / `--user` / the lens flags.
        /// Accepts a single status, a comma-separated OR set (`draft,approved`),
        /// a status alias, a `user:<name>` / `me` owner-or-assignee filter, or a
        /// lens word. An unrecognized token errors with guidance rather than
        /// being silently ignored.
        ///
        /// Statuses (one per line):
        ///   draft           - filed, not yet approved
        ///   approved        - blessed, ready to plan/queue
        ///   planned         - designed, awaiting implementation
        ///   in-progress     - actively being worked
        ///   needs-attention - parked by a drain; needs a human triage
        ///   done            - work finished on a branch (pre-merge)
        ///   completed       - merged to the default branch
        ///   rejected        - declined; will not be done
        ///
        /// Status aliases:
        ///   open            - Draft, Approved, Planned, InProgress, NeedsAttention
        ///   closed          - Done, Completed, Rejected
        ///
        /// User filters (owner OR assignee):
        ///   me              - your shell identity ($AIDA_USER / $USER)
        ///   user:<name>     - the named person (e.g. `user:joe`)
        ///
        /// Lens words (route to a focused view):
        ///   human           - the "what needs me?" human-attention view
        ///   queue           - your role's queue (= `aida queue list`)
        ///   advisor         - the advisor dashboard (= `aida advisor`)
        ///   why             - the burndown classifier (= `aida burndown explain`)
        ///   inflight        - active leases + drain status
        // trace:TASK-0415 | ai:claude — plain `//` keeps the marker out of `--help`.
        // trace:STORY-662 | ai:claude — verbatim_doc_comment preserves the
        // one-shortcut-per-line layout (clap otherwise reflows into paragraphs).
        #[clap(value_name = "STATUS", verbatim_doc_comment)]
        shortcut: Option<String>,

        /// Filter by status. Accepts a comma-separated OR set
        /// (`--status draft,approved`) and the `open` / `closed` aliases:
        /// `open` = Draft, Approved, Planned, InProgress, NeedsAttention;
        /// `closed` = Done, Completed, Rejected.
        // trace:TASK-0415 | ai:claude
        #[clap(long)]
        status: Option<String>,

        /// Filter by priority
        #[clap(long)]
        priority: Option<String>,

        /// Filter by type
        #[clap(long)]
        r#type: Option<String>,

        /// Filter by feature
        #[clap(long)]
        feature: Option<String>,

        /// Filter by tags (comma separated). A trailing `*` is a prefix-glob:
        /// `aida:queue:*` matches every tag under that surface (and an exact
        /// `aida:queue`). e.g. `--tags 'aida:queue:*'`.
        // trace:TASK-527 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        tags: Option<String>,

        /// Bypass the active role's scope filters for this command.
        // trace:TASK-1-021 | ai:claude
        #[clap(long)]
        no_scope: bool,

        /// Show the origin id alongside the canonical id. The origin id is
        /// the original spec_id assigned when the requirement was created
        /// (zero-padded form for legacy reqs, node-aware
        /// `<TYPE>-<node>-<NNN>` for distributed-mode reqs). The canonical
        /// id is the agreed-id post-merge-gate when one exists, else the
        /// origin id itself.
        // trace:FR-1-070 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, alias = "verbose", short = 'v')]
        show_origin: bool,

        /// Include META requirements (AI prompt customization seeded by
        /// `aida init`) in the output. By default META rows are hidden so
        /// they don't drown out user-authored reqs on small projects;
        /// pass `--include-meta` to see them, or filter explicitly with
        /// `--type meta`.
        // trace:BUG-27 | ai:claude
        #[clap(long)]
        include_meta: bool,

        /// Restrict the listing to direct children of <id> (UUID or
        /// SPEC-ID). Composes with --status / --type / --tags etc., so
        /// e.g. `aida list --parent <epic-id> --status approved` shows
        /// what's still open under that EPIC. Add --recursive to widen this
        /// to the WHOLE transitive subtree instead of just direct children.
        // trace:STORY-62 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, value_name = "ID")]
        parent: Option<String>,

        /// Widen --parent from direct children to the WHOLE transitive subtree:
        /// the parent plus every descendant via parent->child edges, at any
        /// depth. So `aida list open --parent <epic> --recursive` returns the
        /// open items across the epic's entire subtree, not just its direct
        /// children. Requires --parent; composes with the [STATUS] positional,
        /// --status / --type / --tags. Cache-fast (a single recursive query over
        /// the materialized edge graph, no full-store load).
        // trace:TASK-955 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, visible_alias = "subtree", requires = "parent")]
        recursive: bool,

        /// Pull from `origin/aida-store` before listing. Opt-in
        /// freshness for collaborators / multi-session workflows;
        /// fast path stays default. No-op when the local orphan
        /// branch is already current; warns and falls back to the
        /// local view when offline.
        // trace:STORY-78 | ai:claude
        #[clap(long)]
        sync: bool,

        /// Include archived AND deferred requirements (the
        /// everything-escape-hatch). By default `aida list` hides both archived
        /// and deferred rows; archived/deferred ≠ status, so freshly-Completed
        /// specs stay visible until archived. Mutually exclusive with
        /// `--archived` and `--deferred` (the only-X views).
        // trace:STORY-441 | ai:claude — supersedes TASK-64's terminal-status hide.
        // trace:STORY-584 | ai:claude — now widens the defer axis too.
        #[clap(long, conflicts_with_all = ["archived", "deferred"])]
        all: bool,

        /// Show only archived requirements. Use this to audit the archive
        /// itself; `--all` shows the union of active + deferred + archived.
        // trace:STORY-441 | ai:claude
        #[clap(long, conflicts_with_all = ["all", "deferred"])]
        archived: bool,

        /// Show only deferred requirements — the primed/conditional shelf,
        /// with each spec's revisit trigger. Honors both the deferred flag and
        /// legacy `deferred:*` parking tags. `--all` shows the union.
        // trace:STORY-584 | ai:claude
        #[clap(long, conflicts_with_all = ["all", "archived"])]
        deferred: bool,

        /// Emit `[{spec_id,title,req_type,status,tags}]` as JSON instead
        /// of the human table. Internal-use surface for the TUI launcher
        /// to populate its Backlog / History panes; the
        /// schema may change without notice.
        // trace:STORY-244 | ai:claude
        #[clap(long, hide = true)]
        json: bool,

        /// Group the listing by parent EPIC for visual clustering —
        /// children indented under their EPIC, groups sorted by item
        /// count desc, requirements with no EPIC parent under a final
        /// "Unscoped" group. Mirrors `aida queue list --tree`. Composes
        /// with the --status / --type / --priority / --tags / --parent
        /// filters. Mutually exclusive with --json.
        // trace:TASK-568 | ai:claude
        #[clap(long, conflicts_with = "json")]
        tree: bool,

        /// Surface each row's tags as an extra column right of Title.
        /// The chip set is truncated to the first three tags with a
        /// "+N more" suffix when more exist. Composes with every
        /// existing filter (`--tags`, `--status`, `--type`, …).
        // trace:TASK-569 | ai:claude
        #[clap(long)]
        show_tags: bool,

        /// Also mark specs blocked behind an incomplete blocker with a leading
        /// ⊘ glyph. Off by default; the cheap queued ↑ / in-flight ▶ overlay is
        /// always on.
        // trace:TASK-670 | ai:claude
        // trace:TASK-902 | ai:claude — blocked is now read from the cache (a
        // projected column), no longer a full-store load.
        #[clap(long)]
        blocked: bool,

        /// Drop the leading work-routing glyph column (↑ queued / ▶
        /// in-flight / ⊘ blocked) but keep the status glyphs. "Show me
        /// the list without the routing overlay."
        // trace:TASK-670 | ai:claude
        #[clap(long)]
        no_flow: bool,

        /// Strip ALL glyphs (status + work-routing) for plain-text
        /// output — scripting, grep, non-Unicode terminals,
        /// accessibility. Glyphs are on by default; this is the opt-out.
        // trace:TASK-670 | ai:claude
        #[clap(long)]
        no_glyph: bool,

        /// Emit one bare canonical spec ID per line — no header, no count
        /// footer, no color. Directly usable in `$(...)` / xargs:
        /// `aida edit $(aida list open --short --type bug) ...`. Composes
        /// with every filter (`--status` / shortcut / `--type` / `--tags`
        /// / `--parent`) and honors the same archive default as plain
        /// `list`. Mutually exclusive with --json / --tree.
        // trace:TASK-743 | ai:claude
        #[clap(long, visible_alias = "ids-only", visible_alias = "quiet", short = 'q', conflicts_with_all = ["json", "tree"])]
        short: bool,

        /// The "what needs me?" view: every OPEN spec needing a human nudge,
        /// grouped by reason — held-for-review, awaiting-decision, drafts to
        /// approve, NeedsAttention to triage. Excludes the self-resolving rest
        /// (in-flight, deferred, awaiting-merge, long-lived, actionable). Reuses
        /// the `aida burndown explain` classifier as the single source of truth.
        /// Also reachable as the positional alias `aida list human`. Composes
        /// with `--short` (prints just the IDs).
        // trace:STORY-562 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, conflicts_with_all = ["json", "tree", "status"])]
        human: bool,

        /// Order the results. `modified` (default) = freshest first; `heft` =
        /// most graph-connected first (the deterministic in+out-degree weight),
        /// so load-bearing specs surface at the top.
        // trace:STORY-632 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_name = "ORDER", default_value = "modified")]
        sort: String,

        /// Cap the output at the first N rows, applied AFTER sorting — so
        /// `--limit N` returns the N most-recent (with the default
        /// `--sort modified`) or the N most-connected (`--sort heft`).
        /// Composes with every filter; applies to both the human table and
        /// `--json`. Omit for the full (filtered) listing.
        // trace:TASK-900 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_name = "N")]
        limit: Option<usize>,

        /// Show only specs assigned to you (assignee == your shell identity).
        /// Composes with every other filter. Mutually exclusive with
        /// `--assigned`.
        // trace:STORY-639 | ai:claude
        #[clap(long, conflicts_with = "assigned")]
        mine: bool,

        /// Show only specs assigned to <user>. Composes with every other
        /// filter.
        // trace:STORY-639 | ai:claude
        #[clap(long, value_name = "USER")]
        assigned: Option<String>,

        /// Show only specs whose OWNER or ASSIGNEE is <user>. `me` resolves to
        /// your shell identity ($AIDA_USER / $USER — the same resolution the
        /// queue uses). Broader than `--assigned`, which matches assignee only.
        /// Also reachable as the positional `aida list me` / `aida list
        /// user:<name>`. Composes with every other filter.
        // trace:STORY-662 | ai:claude
        #[clap(long, value_name = "USER")]
        user: Option<String>,

        /// Ignore the active focus for this listing — show the whole project
        /// instead of the focused subtree. The focus-specific escape hatch
        /// (`--all` also widens, but additionally reveals archived/deferred);
        /// no-op when no focus is set or when `--parent` was passed explicitly.
        // — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        no_focus: bool,

        /// AGENT-MODE only: widen the token-efficient TOON column schema. By
        /// default agent-mode `aida list` emits the minimal `id,title,status,type`
        /// set; pass a comma-separated field list to expand it, e.g.
        /// `--fields id,title,status,priority,tags`. Valid fields: id, title,
        /// status, type, priority, feature, owner, assignee, tags, heft, queued,
        /// in_flight, blocked. No effect on the human TTY table.
        // trace:TASK-964 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_name = "CSV")]
        fields: Option<String>,
    },

    /// Show details for a specific requirement
    Show {
        /// The ID of the requirement to show
        id: String,

        /// Print comment bodies inline after the description (instead of just
        /// the count). Equivalent to following up with `aida comment list <ID>`.
        #[clap(long, short = 'c')]
        comments: bool,

        /// Render an indented hierarchy of <id> and its descendants instead
        /// of the standard detail view. Each row shows status + title.
        // trace:STORY-62 | ai:claude
        #[clap(long)]
        tree: bool,

        /// Depth limit for `--tree` (default 3). Only meaningful with
        /// `--tree`.
        // trace:STORY-62 | ai:claude
        #[clap(long, default_value = "3", value_name = "N")]
        depth: usize,

        /// Pull from `origin/aida-store` before reading the requirement.
        /// See `aida list --sync`.
        // trace:STORY-78 | ai:claude
        #[clap(long)]
        sync: bool,

        /// Skip the git-linkage section (commits / files traced /
        /// branch / PR). Faster for read-only contexts that only need
        /// the requirement fields.
        // trace:TASK-241 | ai:claude
        #[clap(long)]
        no_git: bool,

        /// Expand the git-linkage section: every referencing commit
        /// (not just the most recent 5) plus per-commit diff stats.
        // trace:TASK-241 | ai:claude
        #[clap(long, short = 'v')]
        verbose: bool,

        /// Render a compact, boxed "spec card" instead of the linear
        /// detail view: a header rule, an ID/type/priority/status
        /// one-liner, key fields, a truncated description, the
        /// acceptance criteria, and the git-linkage summary. The
        /// /aida-pickup skill renders this at session start so the
        /// spec's contract stays in terminal scrollback.
        #[clap(long)]
        card: bool,

        /// With --card: minimal density — a single-line
        /// id/type/priority/status/title summary, no box. For
        /// autonomous or scripted flows that just need the spec named.
        #[clap(long, conflicts_with = "full")]
        brief: bool,

        /// With --card: full density — the complete description with
        /// no paragraph truncation. For deep dives.
        #[clap(long)]
        full: bool,

        // trace:TASK-102 | ai:claude
        /// Enumerate every relationship edge inline (direction + type +
        /// target), regardless of count. Alias `--relations`. Without it,
        /// `aida show` lists edges only when there are few (≤5) and otherwise
        /// prints a count + a pointer to `aida rel list`.
        #[clap(long = "rels", visible_alias = "relations")]
        rels: bool,

        /// Emit the requirement as a JSON object instead of the human detail
        /// view. Includes the deterministic in/out-degree centrality + heft
        /// score so agents can read a spec's load-bearing weight directly.
        // trace:STORY-632 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, conflicts_with_all = ["tree", "card"])]
        json: bool,
    },

    /// Query the cross-spec relationship graph from a root spec. The default
    /// mode is the epic-rollup tree (`--tree`); other modes answer the
    /// transitive blocked-by / blocks and reverse-impact questions a flat
    /// per-feature spec tool structurally can't. Read-only. Pick at most one
    /// mode.
    // trace:STORY-489 | ai:claude
    // trace:TASK-778
    Graph {
        /// The root spec (SPEC-ID or UUID) to query from.
        id: String,

        /// Transitive BlockedBy chain: every spec the root is directly or
        /// indirectly blocked by.
        #[clap(long)]
        blocked_by: bool,

        /// Transitive Blocks chain: every spec the root directly or
        /// indirectly blocks.
        #[clap(long)]
        blocks: bool,

        /// Parent/Child descendants of the root with a status rollup — the
        /// epic-rollup view. The default when no mode flag is given.
        #[clap(long)]
        tree: bool,

        /// Reverse impact: every spec that is (transitively) blocked by the
        /// root — what is at risk if the root slips.
        #[clap(long)]
        impact: bool,

        /// Follow a custom (or built-in) relationship type by name, outgoing —
        /// e.g. `--follow begets`. Repeatable to walk several types at once.
        /// The traversal a flat per-feature spec tool can't do over arbitrary edges.
        // trace:FR-282 | ai:claude
        #[clap(long, value_name = "TYPE")]
        follow: Vec<String>,

        /// Limit traversal to N hops from the root (default: unbounded).
        #[clap(long, value_name = "N")]
        depth: Option<usize>,

        /// Emit the result as JSON for agents / scripts.
        #[clap(long)]
        json: bool,
    },

    /// Mark a spec done — the simple "I finished it". e.g. `aida done TASK-1`.
    /// A newcomer-friendly shortcut for completing a task without the
    /// `edit --status completed` jargon.
    // trace:TASK-727 | ai:claude
    Done {
        /// The spec to mark done (e.g. TASK-1).
        spec: String,
    },

    /// Edit an existing requirement
    Edit {
        /// The ID of the requirement to edit
        id: String,

        /// New title for the requirement
        #[clap(long)]
        title: Option<String>,

        /// New description for the requirement
        #[clap(long)]
        description: Option<String>,

        /// New status (draft, approved, planned, in-progress, done,
        /// completed, rejected). Needs Attention is set via `aida punt`.
        #[clap(long)]
        status: Option<String>,

        /// New priority (high, medium, low)
        #[clap(long)]
        priority: Option<String>,

        /// New type: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc
        #[clap(long)]
        r#type: Option<String>,

        /// New owner
        #[clap(long)]
        owner: Option<String>,

        /// New feature
        #[clap(long)]
        feature: Option<String>,

        /// New tags (comma-separated, replaces existing).
        /// Use --add-tag/--remove-tag for partial edits that don't clobber.
        #[clap(long)]
        tags: Option<String>,

        /// Add a tag without touching existing tags. Repeatable
        /// (--add-tag x --add-tag y). Adding a present tag is a no-op.
        /// Conflicts with --tags.
        // trace:TASK-351 | ai:claude
        #[clap(long = "add-tag", value_name = "TAG", conflicts_with = "tags")]
        add_tag: Vec<String>,

        /// Remove a tag without touching the rest. Repeatable
        /// (--remove-tag x --remove-tag y). Removing an absent tag is a no-op.
        /// Conflicts with --tags.
        // trace:TASK-351 | ai:claude
        #[clap(long = "remove-tag", value_name = "TAG", conflicts_with = "tags")]
        remove_tag: Vec<String>,

        // trace:STORY-446 | ai:claude
        /// Add a BlockedBy edge to this spec (repeatable; alias
        /// `--depends-on`). Creates the BlockedBy edge + inverse Blocks edge
        /// atomically; idempotent on a re-add.
        #[clap(
            long = "blocked-by",
            visible_alias = "depends-on",
            value_name = "SPEC-ID"
        )]
        blocked_by: Vec<String>,

        /// Remove a BlockedBy edge from this spec (repeatable). Removes both
        /// the BlockedBy edge and the inverse Blocks edge; no-op if absent.
        #[clap(long = "remove-blocked-by", value_name = "SPEC-ID")]
        remove_blocked_by: Vec<String>,

        // trace:STORY-476 | ai:claude
        /// Attach a one-way external issue reference (repeatable). Format
        /// `provider:id` where provider is linear, jira, or github — e.g.
        /// `--add-ref linear:LIN-123 --add-ref github:owner/repo#123`.
        /// Rendered as a link in `aida show` and searchable. AIDA stores the
        /// pointer; it does NOT sync state back to the external system.
        #[clap(long = "add-ref", value_name = "PROVIDER:ID")]
        add_ref: Vec<String>,

        /// Remove a previously-attached external issue reference (repeatable).
        /// Matches the stored `provider:id` form; removing an absent ref is a
        /// no-op.
        #[clap(long = "remove-ref", value_name = "PROVIDER:ID")]
        remove_ref: Vec<String>,

        /// Use interactive mode (launches editor)
        #[clap(long, short = 'i')]
        interactive: bool,

        /// Treat session-lease conflicts as a hard error rather than the
        /// configured default. Equivalent to `[session] enforcement =
        /// "block"` for this single invocation.
        // trace:STORY-48 | ai:claude
        #[clap(long)]
        strict: bool,

        /// Bypass the guard that refuses re-opening a Completed or
        /// Rejected requirement (e.g. flipping `--status in-progress`
        /// on something already shipped). Use when intentionally
        /// re-opening: usually you should instead file a new req that
        /// supersedes the closed one.
        // trace:TASK-47 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        force: bool,

        /// Mark this spec as work no agent can do — a person-in-the-room
        /// task, sign-off, or physical activity. Pre-pickup gate skips
        /// human-only specs so the orchestrator never spawns a doomed
        /// phase-1 implementer on them.
        // trace:STORY-333 | ai:claude
        #[clap(long, conflicts_with = "no_human_only")]
        human_only: bool,

        /// Clear the human-only marker (inverse of `--human-only`). Use
        /// when a spec previously thought to be human-only turns out to
        /// be agent-doable.
        // trace:STORY-333 | ai:claude
        #[clap(long, conflicts_with = "human_only")]
        no_human_only: bool,
    },

    /// Delete a requirement
    Del {
        /// The ID (UUID or SPEC-ID) of the requirement to delete
        id: String,

        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Triage findings filed by headless drain phases (reviewer + implementer).
    /// Bare `aida findings` defaults to `aida findings list` — the command every
    /// failure message points the user at, so it must run without a subcommand.
    // trace:STORY-278 trace:STORY-285 trace:STORY-732 | ai:claude
    Findings {
        #[clap(subcommand)]
        cmd: Option<FindingsCommand>,
    },

    /// The async decision inbox — structured questions the advisor distilled
    /// from forks it couldn't resolve, which you answer OUTSIDE any agent
    /// (plain CLI, no LLM session). Bare `aida questions` lists the inbox and,
    /// at a TTY with pending items, offers to enter the answer loop;
    /// `aida questions list` is the pure read; `aida questions answer` records
    /// answers. The recorded answer is a pure data op — slice 1 records it,
    /// a later loop pass applies the chosen resolution.
    // trace:STORY-522 | ai:claude
    Questions {
        #[clap(subcommand)]
        cmd: Option<QuestionsCommand>,
    },

    /// Resolve a spec that needs a human — the natural entry point for the
    /// decision verbs under `aida questions`. Smart-routes: if the spec has a
    /// PENDING decision request (enumerated choices), this answers it
    /// interactively (like `aida questions answer <spec>`); otherwise the spec
    /// is under-specified, so this launches the interactive clarifier (like
    /// `aida questions clarify <spec>`). A thin alias — the logic lives in
    /// `aida questions`.
    // trace:TASK-779 | ai:claude
    Decide {
        /// The spec (SPEC-ID or UUID) to resolve.
        spec: String,
    },

    /// Dispatch a SPIKE to a headless research agent: it produces a
    /// source-grounded analysis (attached to the spike) plus a recommendation,
    /// and escalates any strategic decision to `aida questions` for you to
    /// answer. Propose-mode — the agent does the legwork; nothing is
    /// auto-applied. The agent-able counterpart to the implementer drain.
    // trace:STORY-568 | ai:claude
    Research {
        /// The spike to research (SPEC-ID or UUID).
        id: String,

        /// Compose + show the research prompt, classification, and artifact
        /// paths without spawning the agent or writing anything.
        #[clap(long)]
        dry_run: bool,

        /// Directory for the dated analysis + decision-sidecar artifacts.
        #[clap(long, default_value = "docs/research")]
        artifact_dir: String,
    },

    /// The advisor seat. Bare `aida advisor` prints the advisor's actionable
    /// worklist — the mirror of bare `aida human`, but grouped by ADVISOR
    /// action (groom drafts, distill decisions, triage findings, bless the
    /// queue, close delivered epics). Subcommands manage the live-advisor
    /// registration the `--no-human=both` orchestrator reads, the dashboard,
    /// and scheduling.
    // trace:STORY-360 trace:STORY-618 | ai:claude
    Advisor {
        /// Bare spec-ids from the worklist, one per line — usable in `$(...)` /
        /// xargs (mirrors `aida human --short`). Ignored when a subcommand is
        /// given.
        // trace:STORY-618 | ai:claude
        #[clap(long)]
        short: bool,
        #[clap(subcommand)]
        command: Option<AdvisorCommand>,
    },

    /// Punt a spec to Needs Attention — pause it with a structured reason
    /// instead of guessing past a design-fork. The safety net an autonomous
    /// drain reaches for when it hits a decision it cannot safely make.
    // trace:STORY-332 | ai:claude
    Punt {
        /// The spec (UUID or SPEC-ID) to punt. Must currently be In Progress.
        id: String,

        /// The kind of obstacle. One of: design-fork, ambiguous-spec,
        /// missing-context, blocked-dependency, other.
        #[clap(long, short = 'c')]
        category: String,

        /// Human-readable description of the fork / obstacle that stopped you.
        #[clap(long, short = 'r')]
        reason: String,

        /// Optional best-guess answer if forced to choose — recorded
        /// distinctly from the reason so triage can see the agent's lean.
        #[clap(long, short = 'l')]
        lean: Option<String>,
    },

    /// Browse and analyze the local design-fork punt ledger.
    // trace:STORY-325 | ai:claude
    // trace:TASK-956 | ai:claude — hidden from top-level --help (still runs).
    #[clap(subcommand, hide = true)]
    Punts(PuntsCommand),

    /// Autonomy + calibration views. Today this carries the three-way
    /// complexity-calibration surface from `aida autonomy calibration
    /// mismatches`; the broader autonomy-report views land alongside
    /// the autonomy-metric work that owns them.
    // trace:STORY-439 | ai:claude
    // trace:TASK-852 | ai:claude — hidden from top-level --help (still runs).
    #[clap(subcommand, hide = true)]
    Autonomy(AutonomyCommand),

    // trace:TASK-394 | ai:claude
    /// Persist (or revoke) the one-time `--no-human` scope acknowledgement so
    /// an overnight `aida queue work --auto-complete --no-human=both` loop
    /// doesn't re-prompt every iteration.
    #[clap(subcommand)]
    NoHuman(NoHumanCommand),

    /// Quantitative effort/load views. Effort buckets are 15m, 1h, 4h,
    /// 1d (8 work-hours), and 1w (5 work-days / 40 work-hours).
    // trace:STORY-451 | ai:codex
    // trace:TASK-852 | ai:claude — hidden from top-level --help (still runs).
    #[clap(subcommand, hide = true)]
    Load(LoadCommand),

    /// Mark one or more requirements as archived (hidden from default
    /// views but preserved in graph traversals and the audit trail).
    ///
    /// Archive is a view-level flag distinct from status — a freshly
    /// Completed spec stays visible until archived. Use `--older-than`
    /// for bulk sweeps over closed work; the default csv targets
    /// completed and rejected statuses.
    // trace:STORY-441 | ai:claude
    Archive {
        /// SPEC-ID to archive (mutually exclusive with --older-than).
        id: Option<String>,

        /// Bulk-archive every spec last touched before this duration
        /// (e.g. `30d`, `12h`, or RFC3339). Pairs with `--status`.
        // trace:STORY-441 | ai:claude
        #[clap(long, value_name = "DURATION", conflicts_with = "id")]
        older_than: Option<String>,

        /// Restrict the --older-than sweep to specs with one of these
        /// statuses (comma-separated). Default: completed,rejected.
        // trace:STORY-441 | ai:claude
        #[clap(long, value_name = "CSV", requires = "older_than")]
        status: Option<String>,

        /// Print the sweep plan without writing — pairs with --older-than.
        // trace:STORY-441 | ai:claude
        #[clap(long)]
        dry_run: bool,

        /// Skip the confirmation guard when archiving a non-terminal
        /// (Draft/Approved/Planned/InProgress) or queued spec, and allow
        /// the --older-than sweep to include non-terminal statuses. Archive
        /// is for the closed long-tail; this opts past that safety rail.
        // trace:BUG-492 | ai:claude
        #[clap(long)]
        force: bool,

        /// List each archived id during an --older-than sweep (instead of
        /// just the throttled progress tick + completion line).
        // trace:BUG-497 | ai:claude
        #[clap(long, requires = "older_than")]
        verbose: bool,
    },

    /// Inverse of `aida archive` — clears the archive flag on a spec so
    /// it reappears in default `aida list` / `aida history` views.
    // trace:STORY-441 | ai:claude
    Unarchive {
        /// SPEC-ID or UUID to unarchive.
        id: String,
    },

    /// Defer a spec — park it as primed/conditional work, hidden from the
    /// default open-work view but not filed away the way archive is.
    ///
    /// Defer is a view-level flag distinct from status (it does not touch the
    /// lifecycle state machine). Use `--until` to record the revisit trigger —
    /// the free-text condition that brings the spec back (e.g.
    /// `--until "when a slice verb ships"`). That trigger is the one thing
    /// distinguishing deferred (prospective, primed) from archived
    /// (retrospective, filed). Deferred rows are hidden from `aida list`,
    /// `aida search`, and `aida history` by default; surface them with
    /// `--deferred` (only) or `--all` (union).
    // trace:STORY-584 | ai:claude
    Defer {
        /// SPEC-ID or UUID to defer.
        id: String,

        /// The revisit trigger — the condition that brings this spec back
        /// (free text). Stored alongside the spec and shown in the deferred
        /// view so you can scan what is primed and what returns each item.
        // trace:STORY-584 | ai:claude
        #[clap(long, value_name = "CONDITION")]
        until: Option<String>,
    },

    /// Inverse of `aida defer` — clears the deferred flag (and its revisit
    /// trigger) so the spec reappears in default `aida list` / `aida history`
    /// / `aida search` views.
    // trace:STORY-584 | ai:claude
    Undefer {
        /// SPEC-ID or UUID to undefer.
        id: String,
    },

    /// Record a spec-scoped CLAIM so advisor-fanned work is visible to the
    /// duplicate-dispatch gates.
    ///
    /// AIDA-launched work (`aida queue work`, `aida agent new --spec`) writes a
    /// spec-scoped session lease, so a second pickup of the same spec is
    /// refused. But when an advisor fans an implementer via the Claude Agent
    /// tool (a worktree-isolated subagent), that work takes only a generic
    /// `harness-worktree` lease — NOT spec-scoped — so the gate can't see it.
    /// `aida claim <spec>` closes that gap: it writes a lightweight advisory
    /// lease whose scope IS the spec id, keyed to the calling session's pid, so
    /// the same gate now refuses a duplicate `aida queue work <spec>` and warns
    /// on `aida edit <spec>`.
    ///
    /// Liveness-aware: the claim only holds while the claiming process is alive;
    /// a dead claimer's claim is ignored (no crash-deadlock). Idempotent —
    /// re-claiming your own spec is a no-op refresh. Clear it with
    /// `aida unclaim <spec>`. Surfaces in `aida status <spec>` and `aida ps`.
    // trace:TASK-957 | ai:claude
    Claim {
        /// SPEC-ID, agreed-id, or UUID to claim.
        spec: String,

        /// Optional worktree path to record on the claim (where the fanned
        /// implementer is working). Informational; the claim's liveness is the
        /// claiming process, not this path.
        #[clap(long, value_name = "PATH")]
        worktree: Option<String>,
    },

    /// Inverse of `aida claim` — removes the caller's spec-scoped claim so a
    /// fresh `aida queue work <spec>` / `aida edit <spec>` no longer sees it.
    // trace:TASK-957 | ai:claude
    Unclaim {
        /// SPEC-ID, agreed-id, or UUID to unclaim.
        spec: String,
    },

    /// Assign a spec to a team member — sets the durable assignee and routes
    /// the spec into that member's work queue so it shows in their
    /// `aida queue list`. Idempotent: re-running with the same target is a
    /// no-op. Surface assigned work with `aida list --mine` (yours) or
    /// `aida list --assigned <user>`.
    // trace:STORY-639 | ai:claude
    Assign {
        /// SPEC-ID or UUID to assign.
        id: String,

        /// The team member to assign to (a username / handle, matching the
        /// shell identity that `aida queue list` keys off — see
        /// `current_user_id`).
        #[clap(long, value_name = "USER")]
        to: String,
    },

    /// Inverse of `aida assign` — clears the assignee. By default this leaves
    /// the spec in the assignee's queue (the queue is the now-doing list, not
    /// the assignment of record); pass `--from-queue` to also remove it.
    // trace:STORY-639 | ai:claude
    Unassign {
        /// SPEC-ID or UUID to unassign.
        id: String,

        /// Also remove the spec from the (former) assignee's work queue.
        #[clap(long)]
        from_queue: bool,
    },

    /// Feature management commands
    #[clap(subcommand, hide = true)]
    Feature(FeatureCommand),

    /// Database management commands
    #[clap(subcommand, hide = true)]
    Db(DbCommand),

    /// SQLite cache view commands (git-canonical mode only)
    #[clap(subcommand, hide = true)]
    Cache(CacheCommand),

    /// Inspect or prune the durable per-spec processing record — the audit
    /// trail of what was done + why, captured at completion time.
    // trace:STORY-582 | ai:claude — plain `//` keeps the id out of `--help`.
    #[clap(subcommand, hide = true)]
    Record(RecordCommand),

    /// Throwaway sandbox store for drain-testing / scenario play. Creates a
    /// discardable git-canonical store under a temp dir; point `aida` at it
    /// with the printed `AIDA_STORE=...` export so drains and test specs never
    /// touch the project's real store. `reset` re-seeds, `destroy` removes it.
    // trace:SPIKE-48 | ai:claude
    #[clap(subcommand)]
    Sandbox(SandboxCommand),

    /// Inter-agent mailbox: peer↔peer messaging (send / inbox / thread).
    // trace:STORY-493 | ai:claude
    #[clap(subcommand)]
    Mailbox(MailboxCommand),

    /// Manage per-clone node identity (acquire/release node ids in the
    /// shared registry, list registered clones). Each clone of an AIDA
    /// project gets a unique node id; that id is the namespace for
    /// node-aware spec ids (e.g. `FR-<node>-<NNN>`) until the merge gate
    /// promotes them to short agreed-ids.
    // trace:EPIC-1-052 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(subcommand)]
    Node(NodeCommand),

    /// Show "where am I right now" — unified view of session, branch,
    /// PR/CI, queue, cache, and project state. Spiritual cousin of
    /// `git status`: one screen, no flag-guessing.
    ///
    /// Default sections (each graceful-degrades when its data isn't
    /// available):
    ///   Session  — active lease covering cwd: id, scope, role, worktree, age
    ///   Branch   — current branch, dirty state, ahead/behind origin
    ///   PR / CI  — open PR for the branch + check rollup (via `gh`)
    ///   Queue    — top items routed to the active role + total count
    ///   Cache    — orphan-store cache freshness
    ///   Project  — storage mode, requirement counts (existing view)
    ///
    /// With a SPEC argument (`aida status <spec>`), switch to the per-spec
    /// liveness view: the spec's lifecycle status plus a LIVENESS section
    /// answering "is a live session actually working this, or is the
    /// In-Progress flag orphaned?". For a spec-scoped session lease it shows
    /// the session id, role, worktree, started-at, elapsed, and a verdict:
    /// live (the holder process is alive) vs STALE (no live process — the
    /// flag is orphaned). An In-Progress spec with no spec-scoped lease reads
    /// flag-only (the status is not liveness-backed).
    ///
    // trace:TASK-220 | ai:claude
    Status {
        /// Optional spec id (SPEC-ID / agreed id). When given, show the
        /// per-spec liveness view (status + live/STALE/flag-only verdict)
        /// instead of the project-wide "where am I" snapshot.
        // The trace marker stays a plain `//` comment so this SPEC-ID never
        // leaks into `--help` output (the provenance convention).
        // trace:STORY-694
        spec: Option<String>,
        /// With a SPEC argument: minutes of idle a spec-scoped lease may sit
        /// with a live process but no spec movement before it is reported as
        /// STALLED rather than live. Default 180 (3h). Ignored without a SPEC.
        // trace:BUG-623
        #[clap(long, default_value_t = 180)]
        idle_minutes: u64,
        /// Suppress the AIDA-development-context section even when running
        /// inside the aida repo
        #[clap(long)]
        no_dev_context: bool,
        /// One-line summary (spiritual cousin of `aida statusline`).
        /// Suppresses every section except a compact role/scope/branch
        /// readout.
        // trace:TASK-220 | ai:claude
        #[clap(long)]
        short: bool,
        /// Machine-readable JSON output. Sections that fail
        /// (gh unavailable, no session, etc.) appear as `null` so
        /// consumers can detect "section absent" without parsing prose.
        // trace:TASK-220 | ai:claude
        #[clap(long)]
        json: bool,
        /// Focus on the queue section only — skip everything else.
        // trace:TASK-220 | ai:claude
        #[clap(long, conflicts_with_all = ["ci", "short"])]
        queue: bool,
        /// Focus on the PR / CI section only — skip everything else.
        // trace:TASK-220 | ai:claude
        #[clap(long, conflicts_with_all = ["queue", "short"])]
        ci: bool,
        /// Skip the gh-backed PR / CI lookup (offline / fast path).
        /// The PR/CI section is omitted from text output and reported
        /// as `null` in `--json`.
        // trace:TASK-220 | ai:claude
        #[clap(long)]
        no_ci: bool,
        /// Focus on the cleanup section only — print just "Needs attention"
        /// (sticky In-Progress specs, missed auto-bumps, dormant leases,
        /// stale-on-merged reviewer leases, branches ahead with no PR,
        /// worktrees with uncommitted work, orphan project dirs, open PRs
        /// awaiting merge). Default `aida status` adds a one-line summary
        /// at the bottom when the section is non-empty.
        ///
        /// This is a READ-ONLY view — it surfaces what needs cleanup, it
        /// does not change anything. To actually fix the auto-healable
        /// items, run `aida doctor heal` (or `aida doctor check` to
        /// diagnose first). Think of it as glance (status --cleanup) → fix
        /// (doctor heal).
        // trace:STORY-385 | ai:claude
        // trace:TASK-753 | ai:claude
        #[clap(long, conflicts_with_all = ["queue", "ci", "short", "awaiting"])]
        cleanup: bool,
        /// Focus on live advisor/external activity recorded in
        /// `.aida/advisor-activity.jsonl`.
        ///
        /// This is a READ-ONLY view. Default `aida status` shows a short
        /// recent footer when activity exists; pass `--activity` for the full
        /// filtered feed.
        // trace:STORY-405 | ai:codex
        #[clap(long, conflicts_with_all = ["queue", "ci", "short", "cleanup", "awaiting"])]
        activity: bool,
        /// With `--activity`, only show events after this relative duration
        /// (`30m`, `12h`, `2d`) or RFC3339 timestamp.
        // trace:STORY-405 | ai:codex
        #[clap(long, requires = "activity")]
        since: Option<String>,
        /// Focus on the "Awaiting you" section only — every human-gate item
        /// (mergeable PRs with no pending CI / requested changes, unacked
        /// briefs, findings awaiting triage, NeedsAttention escalations).
        /// On default `aida status` this section also leads the report.
        // trace:STORY-465 | ai:claude
        #[clap(long, conflicts_with_all = ["queue", "ci", "short", "cleanup"])]
        awaiting: bool,
        /// With `--cleanup`: list every item per category instead of the
        /// first 3. With `--awaiting` (or on default status): lift the
        /// 5-item cap on the "Awaiting you" section to show every item.
        // trace:STORY-385 | ai:claude
        // trace:STORY-465 | ai:claude
        #[clap(long)]
        verbose: bool,
        /// Suppress the AIDA doctor "Hygiene" section entirely.
        // trace:STORY-464 | ai:antigravity
        #[clap(long)]
        no_hygiene: bool,
        /// Show the full fleet roster: reveal stale (dead-PID) agents the
        /// default view hides behind a footer count, and list every worktree
        /// instead of collapsing a large roster to a one-line summary. By
        /// default `aida status` reflects LIVE reality and points at
        /// `aida doctor heal` / `aida session gc` to reap the abandoned ones.
        // trace:BUG-609 | ai:claude
        #[clap(long)]
        all: bool,
        /// Reveal only the stale (dead-PID) agents the default "Active agents"
        /// headline hides — the narrow form of `--all` (worktrees stay
        /// summarized).
        // trace:BUG-609 | ai:claude
        #[clap(long)]
        stale: bool,
        /// Reproduce the exhaustive dump: expand every section the terse
        /// default collapses to a one-line summary (open-PR roster, recently
        /// merged, inferred remote activity, cross-clone coordination, the
        /// recent-activity feed, the per-status requirement breakdown, and the
        /// AIDA-dev-context block). By default `aida status` leads with the
        /// answer — session / branch / PR / queue / what-needs-you — and folds
        /// the long-tail rosters behind one-liners so the orientation command
        /// fits roughly one screen. `--all` implies `--full` (and also reveals
        /// the fleet roster). Use `--full` when you want the full picture
        /// without the stale-agent / worktree expansion `--all` adds.
        // trace:STORY-673 | ai:claude
        #[clap(long)]
        full: bool,
        /// Ignore the active focus for this snapshot — drop the focus banner.
        /// No-op when no focus is set. (`aida status --all` is the fleet
        /// roster, a different axis, so focus has its own escape here.)
        // — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        no_focus: bool,
    },

    /// The global running-work table — one row per active session/agent
    /// across the project (the `ps`/`docker ps` of AIDA work). Companion to
    /// `aida status <spec>` (per-spec): where that answers "is THIS spec
    /// liveness-backed?", `aida ps` answers "what is genuinely running on
    /// my machine right now, and what only CLAIMS to be?".
    ///
    /// Each session row shows: session id, spec (scope), role, worktree, pid,
    /// started, ELAPSED, and a liveness verdict reusing the same machinery
    /// `aida status <spec>` and `aida session leases` use
    /// (live / dormant / STALE).
    ///
    /// It ALSO surfaces ORPHANED In-Progress specs — specs at status
    /// In-Progress with NO spec-scoped lease backing them (the flag-only
    /// case) — so a crashed or never-started session can't hide behind a
    /// status flag. AIDA-launched work (`aida queue work` / `aida agent new`)
    /// gets spec-scoped leases (the rich, spec-linked rows); advisor
    /// Agent-tool fan-outs take generic `harness-worktree` leases (shown by
    /// session/worktree, spec unknown).
    // trace:STORY-696 | ai:claude
    Ps {
        /// Machine-readable JSON output: `{ sessions: [...], orphaned: [...] }`.
        // trace:STORY-696
        #[clap(long)]
        json: bool,
        /// Include STALE (dead-pid / leaked) session rows that the default
        /// view folds behind a footer count.
        // trace:STORY-696
        #[clap(long)]
        all: bool,
    },

    /// The integrator seat at a glance — a read-only throughput view (writes
    /// nothing, no drain).
    ///
    /// One screen, scoped to the active focus, answering "what would I work
    /// next, and is main actually moving?": the focus-scoped queue (specs + the
    /// role each is routed to), live throughput (time since the last merge to
    /// the default branch, recent-merge counts, and a main-idle indicator), and
    /// the active fan-out (which sessions/agents hold which specs right now).
    /// A cheap cache-backed read — no full store load, no network beyond a
    /// local git read.
    // trace:TASK-1034 | ai:claude
    Integrate {
        /// Emit the view as JSON instead of the human summary.
        // trace:TASK-1034
        #[clap(long)]
        json: bool,
    },

    /// Stream the drain event feed and wake on actionable verbs only.
    ///
    /// Follows `.aida/events.jsonl` like `tail -f`, classifies each event in
    /// cheap code, and prints ONE wake line only when something actionable
    /// happens (CI terminal, a PR shipped/merged, a punt, a shelve, an
    /// escalation, the drain finished) — staying silent on the benign majority
    /// (phase churn, run bookkeeping). Intended to back the harness `Monitor`
    /// tool so a supervising session burns nothing while nothing is happening.
    /// If the drain's orchestrator process has died, prints one
    /// `WAKE drain-crashed` line and exits so a follower never blocks forever.
    // trace:TASK-990 | ai:claude
    Watch {
        /// Emit wake lines for actionable events (the default behavior — this
        /// flag makes it explicit for the `Monitor`-tool invocation).
        #[clap(long)]
        emit_wakes: bool,

        /// Also surface the benign (non-actionable) events as an indented debug
        /// feed, not just the wake lines.
        #[clap(long)]
        all: bool,

        /// Drain the current backlog, classify it, and exit instead of
        /// following the stream (cron / test mode).
        #[clap(long)]
        once: bool,
    },

    /// List the team: every registered node/clone sharing this store
    /// (`registry/nodes.toml`) — node id, host, email, clone path, when it
    /// registered, and whether it currently holds a coordination claim
    /// (lease / drain / solo). The roster that makes a team visible.
    ///
    /// With a subcommand, manage per-user roles (`set-role`, `my-role`).
    // trace:STORY-640 | ai:claude
    // trace:STORY-646 | ai:claude
    Team {
        /// Machine-readable JSON output (bare `aida team` roster view only).
        #[clap(long)]
        json: bool,
        #[clap(subcommand)]
        cmd: Option<TeamCommand>,
    },

    /// Manage the shared person-alias registry — link the different identity
    /// strings one human registers under across machines (`joe`,
    /// `joe.mooney@gmail.com`) so the queue, team roster, and block list
    /// collapse them to one canonical person. Composes with the case-fold.
    // trace:TASK-845 | ai:claude
    Identity {
        #[clap(subcommand)]
        cmd: IdentityCommand,
    },

    /// Inspect locally-recorded CLI usage. Reads `~/.aida/usage.jsonl`
    /// (written one line per `aida ...` invocation when telemetry is
    /// enabled — opt out via `[telemetry] enabled = false` or
    /// `AIDA_TELEMETRY=0`). Default: top-20 most-used commands over the
    /// last 30 days.
    ///
    /// Privacy: only command shapes are logged (e.g. "queue list",
    /// "show", "rel add") — never arg values, paths, or req content.
    ///
    // trace:STORY-122 | ai:claude
    Usage {
        /// Show commands used within the last N days. Default 30.
        // trace:STORY-122 | ai:claude
        #[clap(long, value_name = "Nd", default_value = "30d")]
        since: String,
        /// Show commands NOT used in the last N days (deprecation
        /// candidates). Mutually exclusive with --errors.
        // trace:STORY-122 | ai:claude
        #[clap(long, value_name = "Nd", conflicts_with = "errors")]
        unused: Option<String>,
        /// Show commands with the highest error rate (`exit_code != 0`
        /// over total invocations). UX-gap candidates.
        // trace:STORY-122 | ai:claude
        #[clap(long)]
        errors: bool,
        /// JSON output (commands as an array of {cmd, count, errors,
        /// avg_ms} objects).
        // trace:STORY-122 | ai:claude
        #[clap(long)]
        json: bool,
        /// Limit the number of rows returned. Default 20.
        // trace:STORY-122 | ai:claude
        #[clap(long, default_value = "20")]
        limit: usize,
        /// Switch to the `--auto-complete` orchestrator telemetry view.
        /// Reads `~/.aida/auto-complete.jsonl` instead of the per-command
        /// usage log: a success/failure summary plus the recent phase
        /// failures and the Draft BUGs auto-filed for them.
        // trace:TASK-266 | ai:claude — plain `//` so the SPEC-ID stays out
        // of `--help` output (TASK-268).
        #[clap(long = "auto-complete", conflicts_with_all = ["unused", "errors"])]
        auto_complete: bool,
        /// With `--auto-complete`: list every recent orchestrator failure
        /// in full (date, spec, failed phase, drafted BUG + its status).
        // trace:TASK-266 | ai:claude
        #[clap(long, requires = "auto_complete")]
        failures: bool,
        /// With `--auto-complete`: show which phases fail most often — the
        /// signal for where to invest orchestrator fixes.
        // trace:TASK-266 | ai:claude
        #[clap(long, requires = "auto_complete", conflicts_with = "failures")]
        pattern: bool,
        /// Show the deterministic project-health catalog: phase-failure
        /// distribution, reap-vs-kill breakdown, drain halt-rate, recovery
        /// latency, draft-inbox depth, and burn-down velocity. Reads the
        /// orchestrator telemetry log + headless session logs + the spec graph.
        // trace:STORY-530 | ai:claude — plain `//` so the SPEC-ID stays out of
        // `--help` output (TASK-268).
        #[clap(long, conflicts_with_all = ["unused", "errors", "failures", "pattern"])]
        health: bool,
        /// Trace-read-rate audit: classify logged commands into graph READS
        /// (list/show/search/graph/why/history/queue list/…) vs graph WRITES
        /// (add/edit/comment add/rel add/queue add/defer/archive/…) and report
        /// the read:write ratio over the window — evidence for whether the
        /// intent graph is consulted, not just written.
        // trace:TASK-872 | ai:claude — plain `//` so the SPEC-ID stays out of
        // `--help` output (TASK-268).
        #[clap(long = "read-write", conflicts_with_all = ["unused", "errors", "failures", "pattern", "auto_complete", "health"])]
        read_write: bool,
        /// Performance lens: rank command shapes by latency (p50/p95/max +
        /// call count), slowest-first, from the captured `duration_ms`. The
        /// perf-debug view — e.g. `status` topped it at ~26s before its
        /// latency fix landed. Honors --since/--limit/--json.
        // trace:STORY-709 | ai:claude — plain `//` so the SPEC-ID stays out of
        // `--help` output (TASK-268).
        #[clap(long, conflicts_with_all = ["unused", "errors", "failures", "pattern", "auto_complete", "health", "read_write"])]
        slowest: bool,
        /// Raw recent event stream: each invocation's ts, cmd, duration_ms,
        /// and exit_code, newest-first. Filter with --cmd / --slower-than;
        /// cap with --limit (default 25). Honors --since/--json.
        // trace:STORY-709 | ai:claude — plain `//` so the SPEC-ID stays out of
        // `--help` output (TASK-268).
        #[clap(long, conflicts_with_all = ["unused", "errors", "failures", "pattern", "auto_complete", "health", "read_write", "slowest"])]
        events: bool,
        /// With --events: keep only events for this exact command shape
        /// (e.g. `queue list`).
        // trace:STORY-709 | ai:claude
        #[clap(long, value_name = "shape", requires = "events")]
        cmd: Option<String>,
        /// With --events: keep only events whose `duration_ms` is at or
        /// above this threshold.
        // trace:STORY-709 | ai:claude
        #[clap(long, value_name = "Nms", requires = "events", value_parser = parse_duration_ms)]
        slower_than: Option<u64>,
    },

    /// Dogfood agent-lift metrics over the recorded telemetry substrate.
    /// A reporting layer that surfaces the coordination signals already
    /// derivable from the autonomous-drain log and the human CLI usage log:
    /// drain success rate, autonomous runs over distinct specs/builds,
    /// stale-base recoveries, and the autonomous-vs-human split. Useful for
    /// case studies, release notes, and proving coordination value.
    ///
    // trace:STORY-477 | ai:claude — plain `//` so the SPEC-ID stays out of
    // `--help` output (TASK-268).
    // trace:TASK-852 | ai:claude — hidden from top-level --help (still runs).
    #[clap(hide = true)]
    Metrics {
        #[clap(subcommand)]
        cmd: crate::cli::MetricsCommand,
    },

    /// Observe-only rule-adherence field study: harvest stated-rule verdicts
    /// from the git log and report adherence vs task span. Opt-in, local-only.
    /// A research/power surface — hidden from top-level --help.
    // trace:SPIKE-67 | ai:claude — hidden from top-level --help (still runs).
    #[clap(name = "field-study", hide = true)]
    FieldStudy {
        #[clap(subcommand)]
        cmd: crate::cli::FieldStudyCommand,
    },

    /// Generate a curated narrative work digest — Released / Major progress /
    /// Strategic direction / Next iteration / Process artifacts — for a time
    /// window. Editorial logic is mechanical: drop typo/chore/style commits,
    /// collapse cluster-PRs to one theme line, keep rejected specs only when
    /// they carry a supersedes/pivoted-from link, strip SPEC-IDs in customer
    /// mode. Default audience is `customer`, default window is
    /// `.aida/last-digest.toml` marker → 24h.
    // trace:STORY-252
    Digest {
        /// Window start: `Nd`/`Nh`/`Nm` duration, ISO date (`YYYY-MM-DD`), or a
        /// git tag/ref. Absent: marker's window_end, else last 24h.
        #[clap(long, value_name = "WINDOW")]
        since: Option<String>,
        /// Tailor framing + SPEC-ID visibility for the reader.
        #[clap(long, value_enum, default_value_t = crate::digest::DigestAudience::Customer)]
        audience: crate::digest::DigestAudience,
        /// Output format.
        #[clap(long, value_enum, default_value_t = crate::digest::DigestFormat::Markdown)]
        format: crate::digest::DigestFormat,
        /// Include forward-looking "Next iteration" section. Default on; pass
        /// `--include-next=false` to suppress.
        #[clap(
            long,
            value_name = "BOOL",
            num_args = 0..=1,
            default_missing_value = "true",
            require_equals = true
        )]
        include_next: Option<bool>,
        /// Include "Process artifacts" memory-pack section. Default: on for
        /// team/self, off for customer. Pass `--include-process=false` to
        /// suppress, bare `--include-process` to force on.
        #[clap(
            long,
            value_name = "BOOL",
            num_args = 0..=1,
            default_missing_value = "true",
            require_equals = true
        )]
        include_process: Option<bool>,
        /// Write digest to this file instead of stdout.
        #[clap(long, value_name = "PATH")]
        out: Option<PathBuf>,
        /// Copy the rendered digest to the system clipboard. Tries
        /// wl-copy / xclip / xsel / pbcopy / clip in turn; falls back
        /// to a warning and stdout if no clipboard tool is found.
        /// Composes with --out (writes both); composes with --format.
        // trace:TASK-381 | ai:claude
        #[clap(long)]
        copy: bool,
        /// Clear the cadence marker and exit without rendering. Use when the
        /// next digest should not auto-resume from the current window_end.
        #[clap(long)]
        reset: bool,
    },

    /// Set up a git `origin` for a project that has none — guided origin
    /// bootstrap. `create` offers GitHub (via gh), personal-GitLab
    /// push-to-create over SSH (no glab/token needed), or attach-existing;
    /// `attach <url>` wires an existing repo. Non-interactive prints the
    /// manual recipe and exits cleanly.
    #[clap(subcommand)]
    Remote(RemoteCommand),

    /// Push code branch AND orphan aida-store branch to origin. Use
    /// --code-only or --store-only to scope. Equivalent to running
    /// `git push` on the current branch followed by `aida db sync
    /// --push` — the two operations users routinely forget to do
    /// together. Skips a leg cleanly when nothing's pending (no
    /// upstream tracked, no commits ahead, no orphan drift). Set
    /// `AIDA_PUSH_DEFAULT=code|store` to flip the default scope.
    // trace:FR-264 TASK-106 | ai:claude
    Push {
        /// Skip the code push (only sync the orphan store).
        #[clap(long, conflicts_with = "store_only")]
        code_only: bool,
        /// Skip the orphan-store sync (only `git push`).
        #[clap(long, conflicts_with = "code_only")]
        store_only: bool,
        /// Commit any pending orphan-store changes with this message
        /// before pushing. Same as `aida db sync --message`.
        #[clap(long, short = 'm')]
        message: Option<String>,
        /// Skip pre-push interactive checks ("branch behind main" and
        /// "PR for this branch already merged"). Useful for CI /
        /// scripted pushes where the prompt would just hang waiting
        /// on stdin.
        // trace:TASK-54 BUG-88 | ai:claude
        #[clap(long)]
        no_rebase_check: bool,
        /// Show what each in-scope leg would push (commit count +
        /// subjects) and exit 0 without pushing anything. Honors
        /// --code-only / --store-only.
        // trace:TASK-108 | ai:claude
        #[clap(long)]
        dry_run: bool,
        /// Emit the dry-run plan as JSON instead of text. Implies
        /// --dry-run.
        // trace:TASK-108 | ai:claude
        #[clap(long)]
        json: bool,
        /// Suppress the non-blocking "N uncommitted change(s) not
        /// included" notice (also honors AIDA_PUSH_QUIET).
        // trace:TASK-863 | ai:claude
        #[clap(long)]
        no_notice: bool,
    },

    /// Fetch remote refs for both legs (code + orphan store) without
    /// merging or touching the worktree. Read-only counterpart to
    /// `aida pull` — refreshes what `origin/<branch>` points at so
    /// downstream checks (statusline behind-count, queue precheck,
    /// rebase dry-run) see the current remote state without paying
    /// the cost of two separate `git fetch` invocations or being
    /// surprised by an implicit merge. Bumps the
    /// `~/.aida/cache/last-fetch.toml` timestamp on success so the
    /// statusline freshness indicator reflects the fetch.
    // trace:TASK-107 | ai:claude
    Fetch {
        /// Skip the orphan-store fetch (only refresh code refs).
        #[clap(long, conflicts_with = "store_only")]
        code_only: bool,
        /// Skip the code-branch fetch (only refresh orphan-store refs).
        #[clap(long, conflicts_with = "code_only")]
        store_only: bool,
        /// Suppress progress + the "N new commits" summary. Errors still
        /// print. Useful for background callers (statusline, hooks).
        #[clap(long, short = 'q')]
        quiet: bool,
    },

    /// Substrate machinery invoked by hooks / scaffolding, not by humans.
    /// Hidden from `--help`. Currently carries the vendor-agnostic
    /// advisor-no-code-write gate the git pre-commit hook calls.
    // trace:STORY-684
    #[clap(hide = true)]
    Internal {
        #[clap(subcommand)]
        command: InternalCommand,
    },

    /// Author a conventional-format-compliant commit and run `git commit`.
    /// Builds the `[AI:tool]? type(scope): description (REQ-ID)` shape the
    /// commit-msg hook requires, so a plain-terminal commit can't be rejected
    /// for format. The REQ-ID is inferred from staged trace comments when a
    /// single spec is present; `--spec` sets it explicitly. The CLI-native
    /// counterpart to the `/aida-commit` skill.
    ///
    /// Example: aida commit --type fix --scope rel --message "accept --spec alias" --spec <SPEC-ID>
    // trace:STORY-663 | ai:claude
    Commit {
        /// Conventional commit type: feat, fix, docs, style, refactor, perf,
        /// test, build, ci, chore, revert.
        #[clap(long = "type", value_name = "TYPE")]
        commit_type: String,
        /// Optional scope (component/area), e.g. --scope rel.
        #[clap(long)]
        scope: Option<String>,
        /// The commit description (the part after the colon).
        #[clap(long, short = 'm')]
        message: String,
        /// Spec id for the (REQ-ID) trailer, e.g. `--spec BUG-N`. When omitted,
        /// inferred from staged trace comments if exactly one spec is present.
        #[clap(long)]
        spec: Option<String>,
        /// AI tool for the [AI:tool] prefix (default: claude when AI-authored
        /// traces are staged). Use --no-ai to force it off.
        #[clap(long)]
        ai: Option<String>,
        /// Omit the [AI:tool] prefix even if AI-authored traces are staged.
        #[clap(long, conflicts_with = "ai")]
        no_ai: bool,
        /// Stage and commit all tracked changes (git commit -a).
        #[clap(long, short = 'a')]
        all: bool,
        /// Print the assembled message without committing.
        #[clap(long)]
        dry_run: bool,
    },

    /// Pull code branch AND orphan aida-store branch from origin. Use
    /// --code-only or --store-only to scope. Symmetric to `aida push`:
    /// equivalent to `git pull --ff-only` on the current branch
    /// followed by `aida db sync --pull`. Skips a leg cleanly when
    /// there's nothing to pull (no upstream tracked, no orphan
    /// remote).
    // trace:TASK-43 TASK-106 | ai:claude
    Pull {
        /// Skip the code pull (only sync the orphan store).
        #[clap(long, conflicts_with = "store_only")]
        code_only: bool,
        /// Skip the orphan-store sync (only `git pull`).
        #[clap(long, conflicts_with = "code_only")]
        store_only: bool,
        /// Suppress the per-commit change summary printed after the
        /// orphan-store pull. Errors still print. Useful in scripts that
        /// pipe pull output.
        // trace:TASK-73 | ai:claude
        #[clap(long, short = 'q')]
        quiet: bool,
        /// Skip the post-pull merge-gate run. By default `aida pull`
        /// promotes any pending node-aware ids to their agreed short
        /// form after a successful orphan-store pull (idempotent + cheap;
        /// no-ops when nothing's pending). Set the env var
        /// `AIDA_AUTO_MERGE_GATE=false` for a project-wide opt-out.
        // trace:TASK-78 | ai:claude
        #[clap(long)]
        no_gate: bool,
        /// Fetch both in-scope legs, then show what each would pull
        /// (commit count + subjects) and exit 0 without merging.
        /// Honors --code-only / --store-only.
        // trace:TASK-108 | ai:claude
        #[clap(long)]
        dry_run: bool,
        /// Emit the dry-run plan as JSON instead of text. Implies
        /// --dry-run.
        // trace:TASK-108 | ai:claude
        #[clap(long)]
        json: bool,
        /// Auto-rebase tracked stacked branches whose base merged in this
        /// pull. Walks `.aida/stacks.json` bottom-up; for each chain whose
        /// `parent_branch` was just deleted on origin, runs
        /// `git rebase --onto origin/main <recorded-parent-sha> <branch>`
        /// in that branch's worktree. Without `--auto`, prompts
        /// interactively and refuses non-interactively. Refuses to execute
        /// any rebase the `aida_core::rebase::classify` pure classifier
        /// flags as `diverged-risky` — drops the user into `/aida-rebase`
        /// for those.
        // trace:STORY-248 | ai:claude
        #[clap(long)]
        auto: bool,
    },

    /// Detect, classify, and (optionally) execute a rebase of the
    /// current branch onto its upstream. Four phases: detect
    /// (ahead/behind + file-path overlap), classify (clean / ahead-only
    /// / behind-only / diverged-safe / diverged-risky), execute (auto
    /// for safe cases, prompt for risky), report (structured, --json).
    /// Stateless — safe to invoke anywhere.
    // trace:TASK-103 | ai:claude
    Rebase {
        /// Execute safe rebases (behind-only, diverged-safe) without a
        /// confirmation prompt. Risky (file-overlap) cases still prompt.
        #[clap(long)]
        auto: bool,
        /// Classify only — report the state and exit 0 without
        /// touching the working tree.
        #[clap(long)]
        dry_run: bool,
        /// Skip the `git fetch` during detection; classify against the
        /// already-cached upstream ref.
        // trace:TASK-103 | ai:claude
        #[clap(long)]
        no_fetch: bool,
        /// Refuse to run on a dirty working tree instead of the default
        /// auto-stash + pop around the rebase.
        #[clap(long)]
        no_stash: bool,
        /// Machine-readable JSON output for skill / agent consumers.
        #[clap(long)]
        json: bool,
        /// Target ref to rebase onto (default: the current branch's
        /// tracked upstream `@{u}`).
        #[clap(long)]
        branch: Option<String>,
    },

    // trace:STORY-472 | ai:claude
    /// One-verb release: sync the store, run the version bump + tag + push,
    /// wait for the published tarballs, and upgrade sibling installs. Wraps the
    /// whole sequence so you don't have to remember it. Use --check first to
    /// preview without acting.
    Release {
        /// Bump the patch version (default if no level is given).
        #[clap(long)]
        patch: bool,
        /// Bump the minor version.
        #[clap(long)]
        minor: bool,
        /// Bump the major version.
        #[clap(long)]
        major: bool,
        /// Preview the planned release (current → target version + the step
        /// sequence + repo/branch/tree state) without acting.
        #[clap(long)]
        check: bool,
        /// Land an in-flight PR first: wait for PR-<N>'s checks, merge it
        /// (--squash --delete-branch), and sync local main before releasing.
        /// Refuses if the PR's checks fail.
        #[clap(long, value_name = "N")]
        after_pr: Option<u64>,
        /// Skip the cross-platform CI pre-release gate (not recommended for a
        /// published release).
        #[clap(long)]
        skip_xplat_check: bool,
    },

    // trace:STORY-527 | ai:claude
    /// Plan an autonomous burn-down: list which specs are ready to fan out vs
    /// parked, via the pickability gate. The run itself is the /aida-burndown
    /// skill in Claude Code, not a CLI subcommand — `burndown plan` is the
    /// read-only foundation it drives its worktree fan-out from.
    #[clap(subcommand)]
    Burndown(BurndownCommand),

    /// Vital-signs read: is this project HEALTHY right now? One screen across
    /// backlog state (ready/stale/blocked/aging work, burn-down direction) and
    /// coordination state (queue depth, live/stale leases, drains, open
    /// findings, parked work). Worst-anchored verdict, a remedy per warning.
    /// Pure + cache-backed; safe to glance at once a day.
    // trace:STORY-658 | ai:claude
    Health {
        /// Machine-readable JSON (overall grade + every vital with its grade,
        /// value, and remedy).
        // trace:STORY-658 | ai:claude
        #[clap(long)]
        json: bool,
        /// Print only the one-line headline verdict (good for prompts/scripts).
        // trace:STORY-658 | ai:claude
        #[clap(long, conflicts_with = "json")]
        brief: bool,
    },

    // trace:STORY-560 trace:STORY-623 trace:STORY-708 | ai:claude
    /// GROOM the open specs: fire a cold-boot advisor agent that reads all open
    /// specs, applies worth-doing judgment, and proposes approve/reject/park/
    /// queue per spec. GROOM is the advisor's canonical verb for deciding a
    /// draft's fate (distinct from REVIEW, which is code review of a PR).
    /// PROPOSE-MODE BY DEFAULT — writes NOTHING until `--apply`. The advisor-side
    /// analog of `aida burndown run` (which fires implementer agents). Policy
    /// knobs live under `[intake]` in `.aida/config.toml`; the safe defaults work
    /// with zero config. Also reachable as `aida advisor assess`; `aida assess`
    /// and `aida intake` are the retained (deprecated) aliases — both normalize
    /// to `groom`.
    ///
    /// Caveat: the headless advisor is a COLD-BOOT `claude -p`, not your live
    /// session — same model, less context. Autonomy-eligible is not the same as
    /// worth-doing; keep the propose-mode review load-bearing before `--apply`.
    // `assess` / `intake` are the deprecated aliases of `groom` (STORY-708):
    // kept working for back-compat but hidden from top-level help so only
    // `groom` (canonical) is advertised. The pre-clap rewrite normalizes both
    // to `groom` and prints a one-line deprecation hint.
    // trace:TASK-850 trace:STORY-708 | ai:claude
    #[command(alias = "assess", alias = "intake")]
    Groom {
        /// Execute the proposed approvals + queue groom. Without it, `intake`
        /// is propose-only (the value judgment stays reviewable). Even with
        /// `--apply`, the do-not-approve classes and `needs-human`/`strategic`
        /// specs are fenced out — the agent can never bless them.
        #[clap(long)]
        apply: bool,
        /// Cap the number of drafts the agent may approve this run.
        #[clap(long, value_name = "N")]
        max_approvals: Option<usize>,
        /// Only consider specs carrying this tag.
        #[clap(long, value_name = "TAG")]
        only_tag: Option<String>,
        /// Never consider specs carrying this tag.
        #[clap(long, value_name = "TAG")]
        exclude_tag: Option<String>,
        /// Exclude candidates riskier than this ceiling (low / medium / high /
        /// unknown). Same risk chip `aida backlog list` shows. Default: medium.
        #[clap(long, value_name = "MAX", default_value = "medium")]
        risk: String,
        /// After queuing, chain straight into a burndown drain (overrides the
        /// `[intake] on_apply` config to `drain` for this run). Only meaningful
        /// with `--apply`.
        #[clap(long)]
        then_drain: bool,
        /// Show the candidate fence + the exact `claude -p` command that would
        /// run, then exit without launching.
        #[clap(long)]
        dry_run: bool,
        /// Claude permission mode for the headless pass. Defaults to
        /// `bypassPermissions` so the unattended advisor can read/edit without
        /// stalling on prompts. Override with e.g. `acceptEdits`.
        #[clap(long)]
        permission_mode: Option<String>,
    },

    // trace:STORY-547 | ai:claude
    /// Explain why one open spec is still open — a single-spec drill-down using
    /// the same classifier as `burndown explain`. Answers "what's keeping
    /// SPEC-ID from being done?" with a bucket + reason derived from store
    /// signals (status, type, tags, blockers, decisions, live leases).
    Why {
        /// The SPEC-ID to explain (a story, task, or bug id).
        id: String,
        /// Machine-readable JSON (`{spec, bucket, reason, needs_human}`).
        #[clap(long)]
        json: bool,
    },

    /// Set, show, or clear the current FOCUS — a persistent, per-worktree
    /// context (an epic or spec) that scopes the read commands to that spec's
    /// transitive subtree. The kubectl-namespace / gcloud-config pattern for
    /// AIDA's requirement graph.
    ///
    ///   aida focus EPIC-55     set the focus to EPIC-55 (+ its subtree)
    ///   aida focus             show the current focus + a progress rollup
    ///   aida focus --clear     drop the focus
    ///
    /// With a focus set, `aida list`, `aida status`, and `aida queue list`
    /// scope to the focused subtree and print a loud header naming it; pass
    /// `--all` / `--no-focus` on those commands to widen back to everything.
    /// The focus is stored in `.aida/focus` (per-worktree, gitignored) and
    /// surfaced on the statusline so it is never silently active. Precedence:
    /// `AIDA_FOCUS` env > `.aida/focus` > none.
    // — plain `//` keeps the marker out of `--help`.
    Focus {
        /// The epic or spec to focus on (SPEC-ID / agreed id). Omit to show
        /// the current focus.
        #[clap(value_name = "SPEC", conflicts_with = "clear")]
        target: Option<String>,
        /// Clear the current focus (remove `.aida/focus`).
        #[clap(long)]
        clear: bool,
        /// Show the current focus (the default when no SPEC is given).
        #[clap(long)]
        show: bool,
    },

    // trace:STORY-656 | ai:claude
    /// Spec-quality tooling — checks you run ON a spec before work begins.
    /// Today: `aida spec dryrun <SPEC>` (an implementer-readiness pre-check).
    #[clap(subcommand)]
    Spec(SpecCommand),

    // trace:STORY-716 | ai:claude
    /// Manage epic-scoped git worktrees — the workspace layer that
    /// mirrors `git worktree`. `aida worktree add <epic>` creates a worktree
    /// off origin/main (default `~/ai/aida-<epic-slug>` on `<epic>-work`) and
    /// auto-scopes it to that epic via `aida focus`; `aida worktree enter
    /// <epic>` creates-if-missing then cd's you in (run it BARE — the `aida()`
    /// shell wrapper auto-evals the emitted `cd`, NOT wrapped in `eval`);
    /// `aida worktree list` shows AIDA-managed worktrees + each one's focus.
    #[clap(subcommand)]
    Worktree(WorktreeCommand),

    // trace:STORY-631 | ai:claude
    /// Show the AI-generated plain-terms comprehension of WHY a spec exists —
    /// its intent, distilled from the spec + its graph neighborhood. Distinct
    /// from `aida why` (a deterministic state classifier): this is a cached,
    /// drift-stamped LLM synthesis. Generated on first call (or `--refresh`),
    /// printed from cache thereafter; a STALE marker shows when the
    /// neighborhood drifted since generation.
    Intent {
        /// The SPEC-ID to comprehend (any story/task/bug/feature id).
        id: String,
        /// Which register to print: `layman` (prose for a human skimmer) or
        /// `llm` (denser/structured, for an agent loading the spec).
        #[clap(long, value_parser = ["layman", "llm"], default_value = "layman")]
        audience: String,
        /// Force regeneration even if a cached comprehension exists and is fresh.
        #[clap(long)]
        refresh: bool,
        /// Machine-readable JSON
        /// (`{spec, audience, comprehension, generated_at, model, stale}`).
        #[clap(long)]
        json: bool,
    },

    /// AIDA-developer-only commands: activate the in-repo dev binary,
    /// run dev servers, install shell helpers. End users don't need these.
    #[clap(subcommand, hide = true)]
    Dev(DevCommand),

    /// Diagnose and heal AIDA multi-agent state drift.
    // trace:EPIC-19 trace:STORY-462
    Doctor {
        /// Apply safe fixes after scanning. Without this, doctor is read-only.
        #[clap(long)]
        heal: bool,

        /// Skip per-category confirmation prompts while healing.
        #[clap(long, short = 'y')]
        yes: bool,

        /// Restrict scan/heal to one category.
        #[clap(long, value_name = "CATEGORY")]
        category: Option<String>,

        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,

        /// Permit riskier destructive fixes such as branch deletion.
        #[clap(long)]
        force: bool,

        /// Show older completed-without-commit findings individually.
        #[clap(long)]
        all: bool,

        /// Exempt specs completed before this point from the
        /// completed-without-commit integrity check (a git ref/tag whose
        /// commit date is the cutoff, or an ISO date). Quiets noise on
        /// legacy history predating trace conventions. Falls back to the
        /// AIDA_DOCTOR_COMPLETED_SINCE env var.
        // trace:TASK-673 | ai:claude
        #[clap(long, value_name = "REF_OR_DATE")]
        since: Option<String>,

        /// Print the guided, copy-pasteable steps to bring the OS sandbox
        /// (bubblewrap write-confinement) up on this host: detect the current
        /// state, the exact sysctl to persist if the kernel blocks unprivileged
        /// user namespaces, how to opt in, and the verify command. Prints the
        /// sequence — sudo steps are clearly marked "run this yourself"; it
        /// never silently runs sudo. Runs the non-sudo availability re-probe as
        /// a smoke check.
        // trace:STORY-665 | ai:claude
        #[clap(long)]
        fix_sandbox: bool,

        /// Legacy maintenance subcommand or focused doctor action.
        #[clap(subcommand)]
        cmd: Option<DoctorCommand>,
    },

    /// Inspect and align the orphan-store SHA against code commits.
    /// Pairs with the prepare-commit-msg hook (`aida-store-pair.sh`)
    /// that pins the store SHA into every code commit's trailer via an
    /// `Aida-Store: <sha>` line.
    // trace:EPIC-21 | ai:claude
    #[clap(subcommand)]
    Store(StoreCommand),

    /// Manage personas / hats — persistent named contexts that resume
    /// across shells. `aida role enter <name>` switches; `aida role list`
    /// shows what's defined for this project. State at `.aida/roles/`.
    #[clap(subcommand)]
    Role(RoleCommand),

    /// Inspect or resume past Claude Code sessions in this project,
    /// enriched with the AIDA role + most-recent spec for each session
    /// (extracted from the SessionStart hook output stored in each
    /// session's `.jsonl`). Wrapper around `claude --resume` since
    /// Claude Code's auto-generated subject can't be customized.
    // trace:FR-1-043 | ai:claude
    #[clap(subcommand)]
    Session(SessionCommand),

    /// Acquire / release / inspect a per-scope disposition (triage) lease —
    /// the intake gate's "one disposing advisor per scope" guard. The
    /// authority gate decides WHO may dispose; this lease decides HOW MANY
    /// (exactly one live advisor per scope). A second advisor disposing the
    /// same scope is refused, naming the holder. Per-scope, so non-
    /// overlapping subsystem advisors dispose concurrently.
    // trace:TASK-661 | ai:claude
    #[clap(subcommand)]
    Triage(TriageCommand),

    /// The one-shot HUMAN-implementer finish. From inside a worktree where
    /// you implemented a spec, run the whole finish ceremony in ONE command:
    /// commit any uncommitted work with the (SPEC-ID) trailer, rebase onto
    /// current origin/main, push, open the PR, wait for CI green, squash-merge,
    /// run `aida pull` (Done → Completed auto-bump), and remove the worktree.
    ///
    /// It is the orchestrator's finish phases with phase-1 (implement) done by
    /// you instead of a spawned agent — the human-implementer counterpart to
    /// `aida queue work <id> --auto-complete`. The spec is resolved from the
    /// current branch (or the active session lease); pass `<spec>` to override.
    ///
    /// Stop earlier with `--no-merge` (open the PR, don't merge) or `--no-pr`
    /// (just rebase + push). `--keep-worktree` leaves the worktree in place
    /// after a successful merge.
    Ship {
        /// Spec to finish. When omitted, resolved from the current branch
        /// name (e.g. `story-720-ship` → STORY-720) or the active session
        /// lease covering this worktree.
        #[clap(value_name = "SPEC")]
        spec: Option<String>,

        /// Commit subject to use when committing uncommitted work. The
        /// `(SPEC-ID)` trailer is appended automatically. When omitted, a
        /// conventional default is used. Ignored when the worktree is clean.
        #[clap(long, short = 'm', value_name = "MSG")]
        message: Option<String>,

        /// Stop after opening the PR — do not watch CI or merge. The PR is
        /// left open for a human to review + merge.
        #[clap(long)]
        no_merge: bool,

        /// Stop after rebase + push — do not open a PR. Useful when you want
        /// to open the PR yourself, or when CI is configured to gate on push.
        #[clap(long, conflicts_with = "no_merge")]
        no_pr: bool,

        /// Keep the worktree after a successful merge (skip the
        /// `aida session end` cleanup step).
        #[clap(long)]
        keep_worktree: bool,

        /// Print the resolved finish sequence without executing any of it.
        #[clap(long)]
        dry_run: bool,

        /// Bypass the client-side trailer spec-ID check before shipping.
        // trace:STORY-469 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        no_trailer_check: bool,
    },

    /// Pull-request side-effects intended to fire from the /aida-pr
    /// skill. Today: `auto-queue-review` files the reviewer story right
    /// after `gh pr create` so the trigger lives where context is
    /// freshest. `aida session end` keeps the same logic as an
    /// idempotent backup for PRs opened outside the skill.
    // trace:STORY-90 | ai:claude
    #[clap(subcommand)]
    Pr(PrCommand),

    /// Inspect the corroborated orchestrator context — whether this process is
    /// a genuine `--auto-complete` phase child or a standalone session.
    /// Hidden: it is a skill / orchestrator-child introspection hook, not a
    /// daily-driver command.
    // trace:BUG-233 | ai:claude
    #[clap(subcommand, hide = true)]
    Orchestrator(OrchestratorCommand),

    /// One-shot AUTONOMOUS implement + ship from a spec OR a one-line thought.
    /// Pass an approved spec id to drive it; pass free text to draft a spec from
    /// the thought, file it, and drive that — thought to (gated) merged in one
    /// line. Auto-queues the spec, then drives it through the SAME
    /// `--auto-complete` orchestrator `aida burndown` uses (implement → CI →
    /// review → merge → pull). FULLY HEADLESS by default — fire several for
    /// INDEPENDENT specs in parallel and walk away; an INDEPENDENT reviewer
    /// always runs before the auto-merge. Use `--supervised` to drive the
    /// implementer yourself. The auto-implement counterpart to `aida ship`
    /// (which finishes a HUMAN-implemented spec). With no argument, prints help.
    // trace:STORY-721 | ai:claude — plain `//` keeps the marker out of `--help`.
    // trace:STORY-725 | ai:claude
    Zen {
        /// An approved SPEC id to drive (e.g. TASK-123), OR a free-text thought
        /// to draft into a spec and drive. Free text is drafted into a title +
        /// description + acceptance criteria, filed as a draft, then routed
        /// through the approve-gate before driving.
        #[clap(value_name = "SPEC")]
        spec: Option<String>,

        /// Supervised drive: YOU drive the implementer interactively, and the
        /// reviewer still runs headless as an independent gate before the
        /// auto-merge. The opt-in counterpart to the fire-and-forget default
        /// (which runs the implementer headless too). Maps to the
        /// orchestrator's `--no-human=reviewer-only`.
        #[clap(long, conflicts_with = "no_human")]
        supervised: bool,

        /// Explicit autonomy mode override, forwarded to the orchestrator. The
        /// reviewer ALWAYS runs headless as an independent gate; this only
        /// governs the implementer. The DEFAULT is `both` (fully headless
        /// fire-and-forget — fire several for INDEPENDENT specs and walk away).
        /// `--no-human=reviewer-only` (= `--supervised`) keeps the implementer
        /// interactive. Aliases: `--unattended`, `--headless`.
        #[clap(
            long,
            value_name = "MODE",
            num_args = 0..=1,
            default_missing_value = "both",
            aliases = ["unattended", "headless"]
        )]
        no_human: Option<String>,

        /// Skip the post-merge `aida pull` (the Done → Completed auto-bump) —
        /// forwarded to the orchestrator.
        #[clap(long)]
        no_pull: bool,

        /// Override the soft suitability warnings (under-specified / coupled)
        /// and drive anyway. Does NOT override the hard refusals (epic,
        /// keystone, blocked).
        // trace:TASK-1037 | ai:claude
        #[clap(long)]
        force: bool,

        /// Own worktree + own PR: split this spec out of its scope (parent epic
        /// / active focus) instead of routing into the scope worktree. The
        /// explicit exception to the default-into-scope routing.
        // trace:ADR-6 | ai:claude
        #[clap(long, conflicts_with = "into_epic")]
        solo: bool,

        /// Force the cluster route: drive inside the scope (parent epic / focus)
        /// worktree even when the default would otherwise pick its own. The
        /// counterpart to `--solo`.
        // trace:ADR-6 | ai:claude
        #[clap(long)]
        into_epic: bool,

        /// Resolve + validate the spec and PRINT the plan WITHOUT driving it.
        #[clap(long)]
        dry_run: bool,

        /// Inspect the corroborated zen context — whether `AIDA_ZEN=1` is backed
        /// by a verifiable provenance or is a stale / leaked value. A skill
        /// introspection hook, not a daily-driver.
        // trace:BUG-237 | ai:claude
        #[clap(subcommand)]
        command: Option<ZenCommand>,
    },

    /// Inspect the active `aida queue work --auto-complete` drain — what
    /// command launched it, the batch members and their progress, and what
    /// happens to the queue when the current session exits.
    // trace:STORY-301 | ai:claude
    #[clap(subcommand)]
    Drain(DrainCommand),

    /// Inspect the stacked-branch graph: `aida queue work --stack` records
    /// each stacked branch's parent in `.aida/stacks.json`; `aida stack
    /// show` / `list` render the chains. Pairs with the auto-rebase
    /// cascade that `aida pull --auto` runs when a base merges.
    // trace:STORY-248 | ai:claude
    // trace:TASK-852 | ai:claude — hidden from top-level --help (still runs).
    #[clap(subcommand, hide = true)]
    Stack(StackCommand),

    /// Read `.aida/headless-logs/<spec>-<lease>.jsonl` cleanly — wraps the
    /// non-obvious jq filter (assistant content blocks, text-only by default,
    /// `--with-tools` to interleave tool calls) so the user gets clean text
    /// instead of pages of `null`.
    // trace:TASK-398 | ai:claude
    // trace:TASK-956 | ai:claude — hidden from top-level --help (still runs).
    #[clap(subcommand, hide = true)]
    Headless(HeadlessCommand),

    /// Inspect the `aida-worker` shell function's directive channel —
    /// `aida worker directives` lists the FIFO of pending directives the
    /// worker will act on next.
    // trace:TASK-294 | ai:claude
    // trace:TASK-956 | ai:claude — hidden from top-level --help (still runs).
    #[clap(subcommand, hide = true)]
    Worker(WorkerCommand),

    /// Generate, list, and acknowledge per-agent pickup briefs.
    // trace:TASK-492 | ai:codex
    Brief {
        /// Agent name for brief creation, e.g. codex or antigravity.
        agent: Option<String>,

        /// Requirement id for brief creation.
        spec: Option<String>,

        /// Optional operator context. Use '-' to read a multi-line note from stdin.
        #[clap(long)]
        note: Option<String>,

        /// SPEC-ID this brief must be picked up after.
        #[clap(long = "depends-on")]
        depends_on: Option<String>,

        /// Print a `claude-cli://open` deep link after writing the brief.
        /// Click → Claude Code opens in the spec's worktree with a
        /// short pickup prompt (inert until Enter). Eliminates the
        /// paste step. Requires Claude Code v2.1.91+ for the URL scheme.
        // trace:SPIKE-33 | ai:claude
        #[clap(long = "as-deep-link")]
        as_deep_link: bool,

        // trace:TASK-502 | ai:claude
        /// Mark the brief urgent: write a `.pending` sentinel so an idle
        /// agent's `aida status` (and statusline) surfaces it without a
        /// heartbeat. Omit for FYI-only briefs that shouldn't interrupt.
        #[clap(long)]
        notify: bool,

        #[clap(subcommand)]
        cmd: Option<BriefCommand>,
    },

    /// Run one spec through N vendors headless, in isolated worktrees, then an
    /// objective gate — the neutral cross-vendor quality layer. For each vendor
    /// AIDA creates a per-vendor worktree+branch, assembles the implementer
    /// brief from the spec, runs the vendor headless (claude/codex), commits the
    /// result, and runs a deterministic gate that mirrors PR CI by default
    /// (fmt-check + build + tests + clippy + glyph-lint, so a gate-passing
    /// winner is actually mergeable). Reports a
    /// table, ranks the gate-passers (smaller/focused diff first), optionally
    /// runs a rubric LLM judge (--judge), and leaves every branch in place to
    /// pick. Report-only: it recommends a winner but never merges.
    // trace:STORY-659 trace:STORY-660 | ai:claude
    Compete {
        /// SPEC-ID (or UUID) to run through the vendors.
        spec: String,

        /// Comma-separated vendors to run, e.g. `claude,codex`. Headless
        /// vendors run directly; a non-headless vendor (antigravity) is emitted
        /// as a human-run brief instead.
        #[clap(long, value_delimiter = ',')]
        vendors: Vec<String>,

        /// Override the deterministic gate command run in each worktree.
        /// Default mirrors PR CI: fmt-check + build + the aida-cli test suite +
        /// clippy correctness + glyph-lint, so a gate-passing winner is
        /// mergeable. Pass your own command for non-Rust repos / custom CI.
        #[clap(long)]
        gate: Option<String>,

        /// Assemble the briefs + plan the run and print what WOULD happen
        /// without spawning any vendor or touching git.
        #[clap(long)]
        dry_run: bool,

        /// Add a rubric LLM judge after the gate: spawn a `claude -p` judge over
        /// the spec + each candidate's diff, scoring spec-adherence / correctness
        /// / simplicity / test-coverage (1-5) and recommending a winner. Opt-in
        /// and REPORT-ONLY — it never merges; the human/advisor still picks. The
        /// cheap deterministic ranking (smaller, focused diff first) is always
        /// shown regardless of this flag.
        // trace:STORY-660 | ai:claude
        #[clap(long)]
        judge: bool,

        /// Which vendor renders the rubric judgment (claude or codex). The judge
        /// PROMPT is identical for both — only the executing model changes — so a
        /// codex judge over a claude-vs-codex run is no longer the same model
        /// grading itself, removing the self-evaluation caveat. Defaults to
        /// claude (unchanged behaviour). Set AIDA_COMPETE_JUDGE to override the
        /// judge binary. Only applies with --judge.
        // trace:TASK-869 | ai:claude
        #[clap(long = "judge-vendor", default_value = "claude")]
        judge_vendor: String,
    },

    /// Launch and track AI agent processes for this AIDA project.
    // trace:STORY-432 | ai:codex
    #[clap(subcommand)]
    Agent(AgentCommand),

    /// Emit the seven-row finish-state preamble (Spec / Branch / PR / Drain
    /// / Tests / Fmt / Plan) deterministically. One source of truth for
    /// every `/aida-pickup` and `/aida-pr` next-steps template — the rows
    /// used to be filled in by hand from `git status`, `gh pr view`,
    /// `aida show`, `aida session show`, and drifted between sibling
    /// skills. Hidden: skill-internal, not a daily-driver command.
    // trace:TASK-391 | ai:claude
    #[clap(name = "state-snapshot", hide = true)]
    StateSnapshot {
        /// SPEC-ID whose title + status appear on the Spec row.
        #[clap(long)]
        spec: String,

        /// Free-form summary of the last `cargo test` run, used as the
        /// Tests row. Default `"not run"` — the caller (skill) is the
        /// authoritative source for transient runtime state.
        #[clap(long, default_value = "not run")]
        tests: String,

        /// Free-form summary of the last `cargo fmt --check`, used as the
        /// Fmt row. Default `"not run"` — same rationale as `--tests`.
        #[clap(long, default_value = "not run")]
        fmt: String,

        /// Emit the snapshot as JSON instead of the fixed-width text block.
        /// For Step 5b finding metadata (machine-readable snapshot beside
        /// the finding's prose body).
        #[clap(long)]
        json: bool,
    },

    /// One-line project + role summary suitable for shell prompts and
    /// the `statusLine.command` setting in ~/.claude/settings.json.
    /// Sub-50ms (reads the cache + the orphan-store queue YAML). Format:
    ///   aida · <project> · role:<name> · @SPEC · q:N · cache:fresh|stale
    /// Where `q:N` is the depth of the queue routed to the active role
    /// (omitted when zero).
    // trace:FR-1-041 | ai:claude
    Statusline {
        /// When to emit ANSI color: `auto` (default — color iff stdout
        /// is a TTY and `NO_COLOR` is unset), `always`, `never`.
        #[clap(long, value_parser = ["auto", "always", "never"], default_value = "auto")]
        color: String,

        /// Emit the one-liner wrapped in an OSC terminal-title escape
        /// sequence (`ESC ] 2 ; <line> BEL`) instead of printing it to the
        /// body, so the AIDA role/queue/inbox segment rides the terminal
        /// title bar / tmux window name. This is the in-agent parity surface
        /// for clients (e.g. Codex CLI) whose built-in footer is a fixed
        /// field set and cannot run `aida statusline` as a command: wire
        /// `aida statusline --title` into the shell prompt
        /// (`PROMPT_COMMAND` / `precmd`) and the AIDA segment shows in the
        /// terminal title during the agent session. Forces color off (an
        /// OSC title string carries no ANSI). Mutually exclusive with the
        /// `setup` subcommand.
        // trace:TASK-896
        #[clap(long)]
        title: bool,

        /// Opt-in bootstrap helper. With no subcommand, `aida statusline`
        /// renders the one-liner (the default, quiet behavior); the
        /// `setup` subcommand prints (or installs) client-appropriate
        /// statusline configuration.
        // trace:TASK-0414
        #[clap(subcommand)]
        action: Option<StatuslineAction>,
    },

    /// Mark yourself away from the keyboard. Sets a machine-global presence
    /// state (a timestamped `~/.aida/presence.toml` file — no daemon) that
    /// stays `away` until its TTL lapses (default 8h) or you return. An
    /// interactive command auto-flips you back to `home`.
    ///
    /// Presence is ADVISORY input to mode selection: while away, an
    /// `aida queue work --auto-complete` with no explicit `--no-human` /
    /// `--escalate-*` flag defaults to a headless drain; explicit flags always
    /// win and integrity gates (the kickoff scope-ack, CI, merge-on-green) always
    /// apply. Tune under `[presence]` in `.aida/config.toml`:
    ///   consumers  = "on" | "off"            (master switch; default on)
    ///   away_drain = "headless-both"         (default) | "headless-escalate-defaults" | "headless-park"
    ///   home_offer = "surface" | "dont-block" (home-side; default surface)
    ///
    /// Hidden alias for `aida presence away` (kept for muscle memory).
    // trace:TASK-756 trace:STORY-561 | ai:claude
    // trace:TASK-851 | ai:claude
    #[clap(hide = true)]
    Away,

    /// Mark yourself back at the keyboard (clears any away state). Hidden alias
    /// for `aida presence home` (kept for muscle memory).
    // trace:TASK-756 trace:STORY-561 | ai:claude
    // trace:TASK-851 | ai:claude
    #[clap(hide = true)]
    Home,

    /// Operator presence — the home/away state the autonomy ladder keys on.
    /// `aida presence` (or `aida presence status`) shows the current state;
    /// `aida presence away` / `aida presence home` set it. While away, an
    /// `aida queue work --auto-complete` with no explicit mode flag defaults to
    /// a headless drain (advisory; integrity gates always apply).
    // trace:TASK-756 | ai:claude
    // trace:TASK-851 | ai:claude
    Presence {
        #[clap(subcommand)]
        action: Option<PresenceCommand>,
    },

    /// Enter solo mode — mark this session as the advisor+integrator working the
    /// SAFE backlog end-to-end with maximum discretion (groom → implement →
    /// integrate → repeat), parking keystone/architecture for you. A timestamped
    /// flag (~/.aida/solo.toml, 24h safety TTL) the statusline surfaces. `--off`
    /// exits; `--status` prints current state; `--ttl <DURATION>` overrides the
    /// 24h cap (e.g. 8h, 2h30m).
    // trace:STORY-624 | ai:claude
    Solo {
        /// What to do: `run` (start the loop), `stop` (stop it), or `status`.
        /// Omit to enter solo MODE (the work-state flag). The legacy
        /// `--watch`/`--off`/`--status` flags are silent aliases for
        /// run/stop/status.
        // trace:STORY-627 | ai:claude
        #[clap(value_enum)]
        action: Option<SoloAction>,
        /// Exit solo mode.
        #[clap(long)]
        off: bool,
        /// Print whether solo mode is active, without changing it.
        #[clap(long)]
        status: bool,
        /// Safety TTL for the solo flag (default 24h); accepts 8h / 30m / 2h30m.
        #[clap(long, value_name = "DURATION")]
        ttl: Option<String>,
        /// Run the solo LOOP in the foreground: the single leave-it-running
        /// command that works the safe backlog end-to-end on a cadence
        /// (garden → assess/queue → implement → integrate → repeat), keystone
        /// parked. Subsumes `aida queue integrate --watch`. Stops on `aida solo
        /// --off`, Ctrl-C, or the TTL.
        // trace:STORY-625 | ai:claude
        #[clap(long)]
        watch: bool,
        /// With `--watch`: run ONE tick that PRINTS the cycle it would execute,
        /// then exit — verify the loop without running a live drain.
        #[clap(long)]
        dry_run: bool,
        /// With `--watch`: seconds between cycles (default 300).
        // trace:STORY-625 | ai:claude
        #[clap(long, value_name = "SECS", default_value_t = 300)]
        interval: u64,
    },

    /// Print the caller identity AIDA resolved — role, agent-type, agent-name,
    /// user-id, headless flag, ai-tool, and active session/scope — each line
    /// annotated with where the value came from (env var vs fallback vs
    /// default). Read-only: it runs the SAME resolvers the gating/queue/
    /// provenance code uses, so it answers "why did this refuse?" (role
    /// resolved to a default, not advisor, tripping the advisor-gate) and
    /// "why is my queue empty?" (your user/role identity differs from what
    /// queued the items). No project store is loaded; no state is written.
    ///
    /// Hidden alias for `aida node whoami` (identity introspection lives under
    /// the `node` namespace; kept top-level for muscle memory).
    // trace:TASK-784
    // trace:TASK-851 | ai:claude
    #[clap(hide = true)]
    Whoami,

    /// (internal) Background fetch worker spawned by `aida statusline`.
    /// Fetches `origin/<branch>` for the orphan store at <store-path>,
    /// updates `~/.aida/cache/last-fetch.toml`, and removes the
    /// per-project lockfile. Not intended for direct invocation —
    /// statusline forks this as a detached process with stdio nulled.
    // trace:STORY-79 | ai:claude
    #[clap(name = "_bg-fetch", hide = true)]
    BgFetch {
        /// Absolute path to the orphan-store worktree (`.aida-store/`).
        store_path: std::path::PathBuf,
    },

    /// Upgrade aida to the latest release (or a specified version).
    /// Detects how aida was installed (cargo / pre-built binary) and uses
    /// the matching upgrade strategy. From a developer build with no
    /// --target, scans common install locations and offers to upgrade
    /// any stale sibling installs.
    Upgrade {
        /// Compare current/sibling versions to latest release; report only, don't install.
        #[clap(long)]
        check: bool,

        /// Install a specific version instead of latest. Format: `v0.4.0`.
        #[clap(long)]
        version: Option<String>,

        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,

        /// Upgrade a specific binary path instead of the currently-running one.
        /// Useful from a developer shell to upgrade `~/.local/bin/aida` etc.
        #[clap(long)]
        target: Option<String>,

        /// When the dev build is ahead of the latest release tag, print a
        /// `git log --stat` of the unreleased commits so you can vet what
        /// `aida dev patch` would ship.
        #[clap(long)]
        diff: bool,
    },

    /// Relationship management commands
    #[clap(subcommand, hide = true)]
    Rel(RelationshipCommand),

    /// Relationship definition management commands
    #[clap(subcommand, hide = true)]
    RelDef(RelDefCommand),

    /// Manage comments on requirements
    #[clap(subcommand)]
    Comment(CommentCommand),

    /// ID configuration commands
    #[clap(subcommand, hide = true)]
    Config(ConfigCommand),

    /// Requirement type management commands
    #[clap(subcommand, name = "type", hide = true)]
    Type(TypeCommand),

    /// Introspect the storable substrate: object catalog, Requirement fields,
    /// and the controlled-vocabulary tokens (status/type/priority/relationship)
    // trace:STORY-538 | ai:claude
    Schema {
        /// Optional object to detail (any catalog object: requirement,
        /// finding, punt, lease, brief, directive, comment, …). Omit for the
        /// storable-object catalog.
        // trace:BUG-542
        object: Option<String>,

        /// Dump every object's full field detail in one pass: the catalog
        /// followed by each kind's field table, in catalog order.
        // `conflicts_with = "object"` makes `--all <object>` a clear clap error
        // — `--all` is the whole catalog, a positional is one object.
        // trace:TASK-799 trace:TASK-775
        #[clap(long, conflicts_with = "object")]
        all: bool,

        /// Add the explanatory layer: per-field semantics (example value,
        /// who sets it, when/why) plus each object's lifecycle block. Pure
        /// opt-in — the default output is unchanged.
        // trace:STORY-630
        #[clap(long, visible_alias = "details")]
        explain: bool,

        /// Emit machine-readable JSON instead of the human table.
        #[clap(long)]
        json: bool,
    },

    /// Export requirements to different formats
    #[clap(hide = true)]
    Export {
        /// Output format: mapping, json, spec, impl, tree. The default is
        /// `mapping`; the export -> import round-trip needs `--format tree`.
        // trace:TASK-778
        #[clap(long, short = 'f', default_value = "mapping")]
        format: String,

        /// Output file path
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,

        /// Requirement ID (UUID or SPEC-ID) for tree export - exports this requirement and all descendants
        #[clap(long)]
        id: Option<String>,
    },

    /// Import requirements from a tree JSON file
    #[clap(hide = true)]
    Import {
        /// Path to the tree JSON file to import
        file: PathBuf,

        /// Parent requirement ID (UUID or SPEC-ID) to attach imported tree under
        #[clap(long)]
        parent: Option<String>,

        /// Conflict strategy: skip, rename, replace (default: skip)
        #[clap(long, default_value = "skip")]
        on_conflict: String,
    },

    /// Open the user guide in the default browser
    #[clap(hide = true)]
    UserGuide {
        /// Open in dark mode
        #[clap(long)]
        dark: bool,
    },

    /// Server management commands (requires --server or AIDA_SERVER)
    #[clap(subcommand, hide = true)]
    Server(ServerCommand),

    /// Code-to-requirement traceability commands
    #[clap(subcommand, hide = true)]
    Trace(TraceCommand),

    /// Drive human review of a held spec, or generate review prompts.
    ///
    /// `aida review <SPEC>` is the human-review counterpart to `aida queue
    /// work`: it locates the spec's review surface (an open draft PR, else
    /// the branch + commits, else "built locally, never pushed"), runs a
    /// reviewer over the diff against the spec's `## Acceptance` criteria,
    /// then presents the verdict and lets you decide — approve, request
    /// changes, open the diff, or defer. It never auto-merges.
    ///
    /// The `prompt` / `assemble` subcommands are the review-prompt helpers.
    // trace:STORY-67 | ai:claude
    // trace:STORY-553 | ai:claude
    Review {
        /// Spec to review (a story, task, or bug id). Resolves its review surface,
        /// runs a reviewer over the diff, and prompts you to decide. Omit
        /// when using a `prompt` / `assemble` subcommand.
        spec: Option<String>,

        /// Skip the reviewer-agent analysis: just locate + report the
        /// review surface and the recommended next command. Useful in
        /// non-interactive contexts or when you only want the diff pointer.
        #[clap(long)]
        no_agent: bool,

        /// Skip the stale-base warning. By default, when the spec's PR is
        /// behind its base branch, the review prints a warning naming the
        /// gap and the rebase command (the review still proceeds either
        /// way). Pass this flag when a deliberately stale review is
        /// intended — same opt-out the reviewer-role pickup honors.
        // trace:BUG-510 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        allow_stale_base: bool,

        #[clap(subcommand)]
        cmd: Option<ReviewCommand>,
    },

    /// Report generation commands
    #[clap(subcommand, hide = true)]
    Report(ReportCommand),

    /// Initialize AIDA in the current project
    Init {
        /// Skip generating agent skills and commands (.claude/*, .codex/skills/*, and .antigravity/skills/*)
        // trace:TASK-457 | ai:claude
        #[clap(long)]
        no_skills: bool,

        /// Agent profile to scaffold
        #[clap(long, default_value = "both", value_parser = ["claude", "codex", "both"])]
        agent: String,

        /// Skip generating commit validation hooks
        #[clap(long)]
        no_hooks: bool,

        /// Skip bootstrapping the default global role set (implementer, advisor,
        /// reviewer) into ~/.aida/roles/. By default `aida init` scaffolds them
        /// so a fresh machine is ready out of the box.
        // trace:TASK-638 | ai:claude
        #[clap(long)]
        no_roles: bool,

        /// Skip the first-machine-setup prompt for the agent permission
        /// posture (~/.aida/agents.toml). By default, the first `aida init` on
        /// a machine asks how `aida agent new` should handle permissions and
        /// records the choice globally. Non-interactive init never prompts.
        // trace:TASK-698 | ai:claude
        #[clap(long)]
        no_agent_config: bool,

        /// Overwrite existing files if already initialized
        #[clap(long)]
        force: bool,

        /// (deprecated, accepted for backwards compat) Initialize in
        /// distributed mode. Distributed is now the default, so this
        /// flag is a no-op. Use `--centralized` to opt out.
        // trace:EPIC-1-001 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long, hide = true)]
        distributed: bool,

        /// Initialize in legacy centralized mode (SQLite-canonical, single
        /// requirements.db). Deprecated — git-canonical is now the default.
        #[clap(long)]
        centralized: bool,

        /// Use a sibling repo instead of an orphan branch for the store.
        /// Recommended for multi-repo workspaces where multiple code repos
        /// share one store. Implies distributed mode.
        #[clap(long)]
        sibling: bool,

        /// Attach this repo to an EXISTING sibling store (multi-repo) instead of
        /// creating a new one. Writes the local config + rebuilds the cache and
        /// never touches the store's contents — the shared dispenser serializes
        /// id allocation across repos, so no separate node id is needed.
        // trace:STORY-674 | ai:claude
        #[clap(long, visible_alias = "join")]
        attach: bool,

        /// Explicit store location for distributed mode — a separate store repo
        /// at PATH (relative to this repo, or absolute). Several code repos that
        /// point at the SAME path share one store. `--sibling` is sugar for
        /// `--store-path ../aida-store`. Combine with `--attach` to join an
        /// existing store at PATH.
        // trace:STORY-676 | ai:claude
        #[clap(long, value_name = "PATH")]
        store_path: Option<String>,

        /// Git remote URL for the shared aida registry (used with --sibling).
        /// Example: git@github.com:org/aida-registry.git
        #[clap(long)]
        registry_remote: Option<String>,

        /// Verbose output — list every file scaffolded. Default is a brief
        /// summary suitable for first-run UX.
        // trace:BUG-19 | ai:claude
        #[clap(long, short = 'v')]
        verbose: bool,

        /// Project name. Stored in the store's metadata and used as the
        /// title in scaffolded CLAUDE.md / `aida status` output. Defaults
        /// to the current working directory's basename when omitted, so a
        /// fresh init in `~/projects/tzconv/` lands `name = "tzconv"`.
        // trace:BUG-25 | ai:claude
        #[clap(long)]
        name: Option<String>,

        /// Also write the starter memory pack — generic AIDA-using
        /// discipline as Claude Code project memories. Opt-in; off by
        /// default so first-time projects start with an empty memory dir.
        // trace:STORY-255 | ai:claude
        #[clap(long)]
        with_memories: bool,

        /// Refresh the starter memory pack: overlay updated versions of
        /// pack files you have not edited, leaving your own edits intact.
        /// Implies --with-memories.
        // trace:STORY-255 | ai:claude
        #[clap(long)]
        refresh: bool,

        /// Scope the starter memory pack to a subsystem. Memory files
        /// carrying a `subsystem:` frontmatter tag load only when their
        /// value matches; untagged memories are universal and always load.
        /// Omit to load the full pack.
        // trace:STORY-362 | ai:claude
        #[clap(long, value_name = "SUBSYSTEM")]
        focus: Option<String>,

        /// Run `git init` automatically when the current directory is not yet
        /// a git repository, instead of bailing. At a TTY this is offered
        /// interactively; in scripts pass this flag to opt into creating the
        /// repo (otherwise non-interactive init keeps the safe bail).
        // trace:STORY-552 | ai:claude
        #[clap(long)]
        git_init: bool,

        /// When bootstrapping a clone of an already-initialized AIDA project,
        /// commit the locally-written scaffold files to the code branch. The
        /// safe default leaves them uncommitted so a clone never pushes a
        /// scaffold dump to the shared default branch; pass this to opt into a
        /// deliberate scaffold commit. No effect on a genuinely-new first init,
        /// which always commits its own scaffolding.
        // trace:BUG-570 | ai:claude
        #[clap(long)]
        commit_scaffold: bool,

        /// Friendly name for this node, recorded alongside the node id in the
        /// shared roster (`aida team`). Defaults to `<host>-<user>-<seq>` (e.g.
        /// `imac-joe-1`). At a TTY with no flag you are prompted with the
        /// default pre-filled; non-interactive init uses the default silently.
        // trace:STORY-652 | ai:claude
        #[clap(long, value_name = "NAME")]
        node_name: Option<String>,
    },

    /// Starter-memory-pack drift discovery (`aida memories check`)
    // trace:STORY-410 | ai:claude
    #[clap(subcommand)]
    Memories(MemoriesCommand),

    /// Scaffolding management commands
    #[clap(subcommand, hide = true)]
    Scaffold(ScaffoldCommand),

    /// GitLab integration commands
    #[clap(subcommand, hide = true)]
    Gitlab(GitLabCommand),

    /// GitHub integration commands
    #[clap(subcommand, hide = true)]
    Github(GitHubCommand),

    /// Jira integration commands
    #[clap(subcommand, hide = true)]
    Jira(JiraCommand),

    /// Start MCP (Model Context Protocol) server over stdio
    ///
    /// Exposes AIDA requirements as MCP tools for Claude Code integration.
    /// Reads JSON-RPC 2.0 requests from stdin, writes responses to stdout.
    #[clap(hide = true)]
    McpServe,

    /// MCP server management commands (register agents, list tool surface).
    ///
    /// The AIDA MCP server's coordination tools are how non-Claude-Code
    /// agents (Codex, Cursor, …) participate in AIDA drains.
    /// `aida mcp register-agent` writes the AIDA server's entry into a
    /// project's `.mcp.json` so an MCP-speaking agent can connect.
    // trace:STORY-361 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(subcommand, hide = true)]
    Mcp(McpCommand),

    /// Launch the AIDA TUI — a terminal-UI shell that PTY-hosts Claude
    /// Code sessions.
    ///
    /// The TUI becomes the outer shell: it owns the terminal, reserves a
    /// bottom status strip, and hosts a Claude session as a PTY child
    /// (`aida queue work <scope>`). A prefix key (Ctrl-a, configurable
    /// via `[tui] prefix_key`) toggles command mode; `prefix q` quits and
    /// `prefix d` detaches (conversations persist on disk).
    ///
    // trace:STORY-132 | ai:claude
    Tui {
        /// Scope (an EPIC / STORY / … id) to host in the first tab
        /// (PTY-host mode) or to scope the Sessions section against
        /// (launcher mode). Omit to open the launcher dashboard / an
        /// empty PTY-host shell.
        scope: Option<String>,
        /// Skip crash-recovery re-attach of orphaned sessions on launch
        /// and discard any stale `.aida/tui-state.json`. PTY-host-mode
        /// only — the launcher does not own PTY children.
        // trace:STORY-135 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        no_recover: bool,
        // trace:STORY-244 | ai:claude
        /// Force launcher mode: the TUI renders the board, and on a user
        /// action dispatches the chosen command in-process before
        /// re-entering. Defaults to whatever `[tui] mode` resolves to
        /// (launcher unless overridden).
        // trace:STORY-244 STORY-681 | ai:claude
        #[clap(long)]
        launcher: bool,
        /// Power-user / legacy hook: emit one intent line to this file
        /// descriptor and exit (the legacy single-shot fd-3 protocol an
        /// external dispatcher consumes) instead of dispatching the intent
        /// in-process. Omit it — bare `aida tui` is self-sufficient and
        /// needs no fd-3 pipe or `aida-tui` shell wrapper.
        // trace:STORY-244 STORY-681 | ai:claude
        #[clap(long, value_name = "FD", hide = true)]
        intent_fd: Option<u32>,
    },

    /// Project the requirements graph as a layered docs tree.
    /// Constitution, vision, constraints, decisions, quality, glossary —
    /// each layer rendered from its corresponding RequirementType. The
    /// graph is the source; this is the projection.
    // trace:FR-1-077 | ai:claude
    #[clap(subcommand)]
    Docs(DocsCommand),

    /// Sync Claude Code path-gated rules from the spec graph. For each
    /// active spec (In Progress or Done) with spec-id markers in the code,
    /// emit `.claude/rules/aida-specs/<SPEC-ID>.md` with a `paths:` glob
    /// matching the marked files. Claude Code's runtime loads the rule
    /// on-demand when the implementer reads or edits one of those files —
    /// so the spec's scope + acceptance land in context exactly when
    /// load-bearing.
    // trace:SPIKE-31 | ai:claude
    #[clap(subcommand)]
    Rules(RulesCommand),

    /// Living-documentation entries — narrative captured during work
    /// (rationale, scenarios, recipes, gotchas) linked to the specs they
    /// explain. Different from `aida docs` (plural), which renders the
    /// graph as a layered docs tree; this is the *raw* doc entry surface
    /// powering the book/tutorial projection.
    // trace:STORY-104, EPIC-24 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(subcommand)]
    Doc(DocCommand),

    /// Search requirements for a pattern (like grep)
    #[clap(hide = true)]
    Grep {
        /// Pattern to search for (regex supported with -E)
        pattern: String,

        /// Case insensitive search
        #[clap(long, short = 'i')]
        ignore_case: bool,

        /// Use extended regex (ERE)
        #[clap(long, short = 'E')]
        extended_regex: bool,

        /// Show N lines of context after match
        #[clap(long, short = 'A', default_value = "0")]
        after_context: usize,

        /// Show N lines of context before match
        #[clap(long, short = 'B', default_value = "0")]
        before_context: usize,

        /// Show N lines of context before and after match
        #[clap(long, short = 'C')]
        context: Option<usize>,

        /// Search only in specific field(s): title, description, comments, tags, owner, feature
        #[clap(long, short = 'f')]
        field: Option<String>,

        /// Filter by status
        #[clap(long)]
        status: Option<String>,

        /// Filter by type
        #[clap(long)]
        r#type: Option<String>,

        /// Filter by feature
        #[clap(long)]
        feature: Option<String>,

        /// Only show matching SPEC-IDs (like grep -l)
        #[clap(long, short = 'l')]
        files_with_matches: bool,

        /// Show match count per requirement (like grep -c)
        #[clap(long, short = 'c')]
        count: bool,

        /// Invert match (show non-matching)
        #[clap(long, short = 'v')]
        invert_match: bool,
    },

    /// Personal work queue commands
    #[clap(subcommand, hide = true)]
    Queue(QueueCommand),

    /// Curate Approved-but-not-queued work into the queue with risk and
    /// file-overlap heuristics. See `aida backlog --help`.
    // trace:STORY-444 | ai:claude
    #[clap(subcommand)]
    Backlog(BacklogCommand),

    /// Top-level alias for `aida queue rework SPEC` — single verb for the
    /// recurring implementer → reviewer → fixup recovery sequence
    /// (status flip + queue add, with optional `--work` session launch).
    /// Mirrors `aida relations` → `aida rel list` discoverability pattern.
    /// See `aida queue rework --help` for the full flag set.
    // trace:TASK-218 | ai:claude
    Rework {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// Also launch a session for the spec.
        #[clap(long)]
        work: bool,
        /// Override the routing role.
        #[clap(long)]
        r#for: Option<String>,
        /// Override the smart target status.
        #[clap(long, value_name = "STATE")]
        status: Option<String>,
        /// Capture a comment on the spec at rework time.
        #[clap(long)]
        reason: Option<String>,
        /// Chain `aida queue work --resume`.
        #[clap(long)]
        resume: bool,
        /// Bypass terminal-status / already-in-progress guards.
        #[clap(long)]
        force: bool,
        /// Pass `--steal` through to chained `aida queue work`.
        #[clap(long)]
        steal: bool,
        /// Permission mode (only used with `--work`).
        #[clap(long, value_name = "MODE")]
        permission_mode: Option<String>,
        /// Skip pre-pickup pull (only used with `--work`).
        #[clap(long)]
        no_pull: bool,
        /// User ID (defaults to AIDA_USER or system user).
        #[clap(long)]
        user: Option<String>,
    },

    /// Simple search for requirements (case-insensitive by default)
    Search {
        /// Search query (searches title, description, and comments)
        query: String,

        /// Make search case-sensitive (default is case-insensitive)
        #[clap(long, short = 's')]
        case_sensitive: bool,

        /// Filter by status
        #[clap(long)]
        status: Option<String>,

        /// Filter by feature
        #[clap(long)]
        feature: Option<String>,

        /// Maximum number of matches to return (default: 200)
        #[clap(long, short = 'n', default_value = "200")]
        limit: usize,

        /// Pull from `origin/aida-store` before searching. See
        /// `aida list --sync`.
        // trace:STORY-78 | ai:claude
        #[clap(long)]
        sync: bool,

        /// Include the seeded META AI-prompt specs in results. Default hides
        /// them, consistent with `aida list`.
        // trace:BUG-488 | ai:claude
        #[clap(long)]
        include_meta: bool,

        /// Include archived AND deferred requirements in the search
        /// (everything-escape-hatch). Default excludes both for consistency
        /// with `aida list`.
        // trace:STORY-441 | ai:claude
        // trace:STORY-584 | ai:claude — now widens the defer axis too.
        #[clap(long, conflicts_with_all = ["archived", "deferred"])]
        all: bool,

        /// Show only archived requirements.
        // trace:STORY-441 | ai:claude
        #[clap(long, conflicts_with_all = ["all", "deferred"])]
        archived: bool,

        /// Show only deferred requirements (the primed/conditional shelf).
        /// Honors both the deferred flag and legacy `deferred:*` tags.
        // trace:STORY-584 | ai:claude
        #[clap(long, conflicts_with_all = ["all", "archived"])]
        deferred: bool,

        /// Emit one bare canonical spec ID per line — no header, no count
        /// footer, no color. Directly usable in `$(...)` / xargs:
        /// `aida edit $(aida search "auth" --short) ...`. Composes with
        /// every filter and honors the same archive/defer default as plain
        /// `search`. Mutually exclusive with --json. Mirrors `aida list
        /// --short`.
        // trace:BUG-531 | ai:claude
        #[clap(
            long,
            visible_alias = "ids-only",
            visible_alias = "quiet",
            short = 'q',
            conflicts_with = "json"
        )]
        short: bool,

        /// Emit results as a JSON array. Mutually exclusive with --short.
        /// Mirrors `aida list --json`.
        // trace:BUG-531 | ai:claude
        #[clap(long, conflicts_with = "short")]
        json: bool,
    },

    /// Project activity — what's been touched and how it stands now.
    /// Default mode is a per-requirement digest sorted by last-touch
    /// time, intended for "what was I up to last session?" Pass
    /// `--events` to switch to a chronological per-event feed (slower;
    /// decodes each commit's YAML diff into status changes, comments
    /// added, etc.).
    // trace:FR-1-037 | ai:claude
    History {
        /// Number of items to show. In digest mode (default) this caps
        /// the number of distinct requirements; in --events mode it
        /// caps the number of decoded events.
        #[clap(long, short = 'n', default_value = "20")]
        limit: usize,

        /// Walk at most N commits on the orphan branch. Default 250 in
        /// digest mode (cheap to scan) and 5x --limit in --events mode.
        #[clap(long)]
        max_commits: Option<usize>,

        /// Switch to per-event chronological mode — each commit's YAML
        /// diff is decoded into one event line per change (status
        /// transitions, comments added, tags edited, etc.). Slower than
        /// digest because it shells out to `git show` per file per
        /// commit; useful for inspecting one requirement closely with
        /// --id, less useful as a general overview.
        #[clap(long)]
        events: bool,

        /// Only show entries for this requirement (SPEC-ID match).
        #[clap(long)]
        id: Option<String>,

        /// Only show entries for requirements of this type (functional,
        /// bug, …).
        #[clap(long)]
        r#type: Option<String>,

        /// Only show entries authored by this user (matches against the
        /// last_modified_by HLC field if present, else the git committer
        /// email).
        #[clap(long)]
        author: Option<String>,

        /// Only show events after this date (ISO 8601, e.g. 2026-05-01).
        #[clap(long)]
        since: Option<String>,

        /// Only show events before this date (ISO 8601).
        #[clap(long)]
        until: Option<String>,

        /// (--events only) filter to status transitions.
        #[clap(long)]
        status_changes: bool,

        /// Only recent Done→Completed ship transitions — the "did my ship
        /// register?" view. Unlike `--all` (a recency-blind dump of every
        /// terminal-status spec), this shows just what merged-to-default,
        /// newest first. Implies --events; composes with --since/--until/--limit.
        // trace:TASK-507 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        shipped: bool,

        /// (--events only) filter to comment events.
        #[clap(long)]
        comments: bool,

        /// (--events only) terse one-line-per-event format.
        #[clap(long)]
        oneline: bool,

        /// Include archived AND deferred requirements (everything-escape-hatch).
        /// Symmetric with `aida list --all`. By default `aida history`
        /// surfaces every active spec (including freshly-Completed ships) but
        /// hides archived and deferred rows; `--all` widens to the full union.
        // trace:STORY-441 | ai:claude — supersedes TASK-64's terminal-status hide.
        // trace:STORY-584 | ai:claude — now widens the defer axis too.
        #[clap(long, conflicts_with_all = ["archived", "deferred"])]
        all: bool,

        /// Show only archived requirements.
        // trace:STORY-441 | ai:claude
        #[clap(long, conflicts_with_all = ["all", "deferred"])]
        archived: bool,

        /// Show only deferred requirements (the primed/conditional shelf).
        // trace:STORY-584 | ai:claude
        #[clap(long, conflicts_with_all = ["all", "archived"])]
        deferred: bool,
    },

    /// Opt-in EARS-style quality lint for requirement text. AIDA stays a
    /// graph-first substrate (stable IDs, typed relationships, traces); this
    /// is an optional clarity lens, never a required schema. Scores a spec's
    /// description + acceptance criteria for vague triggers, missing expected
    /// behavior, conflicting constraints, and low-testability wording, and
    /// prints suggested rewrites as drafts only — it never edits a spec.
    /// Pass a SPEC-ID to lint one, or `--scope feature|task|story` to sweep
    /// every spec of that kind. Read-only and deterministic (no LLM call).
    // trace:TASK-0417 | ai:claude
    Lint {
        /// SPEC-ID to lint (e.g. `STORY-42`). Omit and pass `--scope` to
        /// sweep a group of specs.
        spec: Option<String>,

        /// Lint every spec of this kind instead of a single SPEC-ID. One of
        /// `feature`, `task`, or `story`. `feature` covers the requirement
        /// types (functional / non-functional / system / user).
        #[clap(long, value_name = "KIND", conflicts_with = "spec")]
        scope: Option<String>,

        /// Emit findings as JSON for agents / scripts.
        #[clap(long)]
        json: bool,
    },

    /// Spec-state lifecycle tooling. Today (generate-only): `aida lifecycle
    /// --diagram` prints a Mermaid state diagram generated from the single
    /// declared transition model in `aida-core`, so the picture can't drift
    /// from the code. `--check` pins the committed diagram in
    /// `docs/lifecycle.md` against the generated one and exits non-zero on
    /// drift (pre-commit-hook-able); with `--write` it inserts/refreshes the
    /// committed block instead. Phase 1 is purely additive — no guard
    /// enforcement, no empirical diffing.
    // trace:TASK-737 | ai:claude
    // trace:TASK-852 | ai:claude — hidden from top-level --help (still runs).
    #[clap(hide = true)]
    Lifecycle {
        /// Emit the Mermaid `stateDiagram-v2` generated from the declared
        /// transition model.
        #[clap(long)]
        diagram: bool,

        /// Compare the generated diagram against the committed mermaid block
        /// in `docs/lifecycle.md` (or `--doc <FILE>`). Exits non-zero on
        /// drift. Pair with `--write` to fix drift in place.
        #[clap(long)]
        check: bool,

        /// Insert or refresh the committed mermaid block in the doc with the
        /// generated diagram. Use with `--check` to auto-fix drift.
        #[clap(long)]
        write: bool,

        /// The doc whose first mermaid block is the pinned diagram. Defaults
        /// to `docs/lifecycle.md` under the project root.
        #[clap(long, value_name = "FILE")]
        doc: Option<String>,

        /// Reconstruct the OBSERVED state machine by walking the `history:`
        /// arrays in the spec store — every recorded status flip becomes one
        /// observed transition, tallied with counts. On its own, prints the
        /// observed transitions; pair with `--diff` to compare against the
        /// declared model.
        // trace:TASK-742 | ai:claude
        #[clap(long)]
        empirical: bool,

        /// Diff the declared model against the observed one: report observed
        /// transitions the declared model never authorized (undocumented
        /// flips) and declared transitions never observed (dead edges). Exits
        /// non-zero when any undocumented flip is found, so it is CI-gate-able.
        /// Implies `--empirical`.
        // trace:TASK-742 | ai:claude
        #[clap(long)]
        diff: bool,
    },

    /// Work with implementation plans archived under `docs/plans/`:
    /// `verify` lints a plan against the structured template, `helpers`
    /// derives a reusable-helpers section from the trace graph, `promote`
    /// moves Approved -> Planned when a plan file exists, and `fan-out`
    /// plans a whole set. Run `aida plan --help` for the full subcommand list.
    // trace:TASK-93 | ai:claude
    // trace:TASK-778
    #[clap(subcommand)]
    Plan(PlanCommand),

    /// Dependency-graph tooling. Today: `aida deps sweep` lists likely
    /// dependencies inferred read-only from the trace graph (shared
    /// trace-link files) plus same-parent siblings — a "did I miss a
    /// dependency before an overnight drain?" check. Read-only: it never
    /// writes edges (confirm them by hand with `aida edit <id>
    /// --blocked-by <dep>`).
    // trace:STORY-447 | ai:claude
    #[clap(subcommand)]
    Deps(DepsCommand),

    /// Generate `CHANGELOG.md` mechanically from git tags + the spec
    /// graph. Walks `v*` tags as release boundaries, scans commits
    /// between them for `(SPEC-ID)` references, classifies each spec
    /// (Features / Fixes / Documentation / Infrastructure / Internal /
    /// Other), and renders one structured markdown section per release.
    /// `generate` prints to stdout or `--out`; `refresh` writes
    /// `CHANGELOG.md` (idempotent — same git state → byte-identical
    /// output); `preview` is `[Unreleased]`-only.
    // trace:TASK-299 | ai:claude
    #[clap(subcommand)]
    Changelog(ChangelogCommand),

    /// Print the CLI-manual rationale section for a command — the *when /
    /// why / when-not* that sits next to `--help`'s *what*. Greps the manual
    /// chapters under `docs/cli/*.md` for the command's entry and prints it
    /// (paged when a pager is available). `aida <cmd> --help` stays the fact
    /// source — this is the rationale. Exits non-zero with a clear message
    /// when no manual entry matches.
    // trace:STORY-600 | ai:claude
    // trace:TASK-852 | ai:claude — hidden from top-level --help (still runs).
    #[clap(hide = true)]
    Manual {
        /// The command whose manual entry to print, e.g. `graph` (or
        /// `aida graph` — the leading `aida ` is optional).
        command: String,
    },

    /// Assemble a rich, structured planning prompt for a SPEC and hand it
    /// to `/ultraplan`. Pulls the spec's description, acceptance criteria,
    /// related-spec context, the spec's enrichment comments, the AIDA
    /// plan-template structure, and the trace-graph reusable helpers into
    /// one prompt — turning a terse ask into a fully-contextualised
    /// brief. Copies to the clipboard by default.
    // trace:TASK-113 TASK-247 | ai:claude
    Ultraplan {
        /// SPEC-ID (or UUID) to assemble the planning prompt for.
        spec: String,

        /// Print the assembled prompt to stdout instead of copying it to
        /// the clipboard. Also the headless fallback when no clipboard
        /// tool is available.
        #[clap(long, conflicts_with = "json")]
        stdout: bool,

        /// Emit the prompt + metadata (warnings, token estimate) as JSON
        /// for scripting.
        #[clap(long)]
        json: bool,

        /// Copy the assembled prompt to the clipboard (default behavior).
        /// This flag is an explicit alias for the default, provided for
        /// command-line discoverability and ease of use.
        // trace:TASK-514 | ai:antigravity
        #[clap(long, conflicts_with = "stdout", conflicts_with = "json")]
        copy: bool,

        /// Omit the `## Comments` section — the spec's enrichment
        /// comments are pulled in by default; pass this for the
        /// leaner comment-free prompt.
        // trace:TASK-247 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        no_comments: bool,
    },

    /// Import a saved plan file into AIDA conventions: archive it under
    /// `docs/plans/YYYY-MM-DD-<slug>.md` and pin it to its SPEC with a
    /// comment. `--request-review` additionally lands a master-review
    /// handshake — it tags the spec `plan-review:pending` and posts a
    /// "plan landed for master review" comment, so the plan is NOT yet
    /// treated as canonical: `aida queue work <SPEC>` warns before
    /// picking up a spec whose plan is still awaiting review.
    // trace:TASK-516 | ai:claude
    #[clap(name = "import-plan")]
    ImportPlan {
        /// The saved plan markdown file to import (e.g. the file
        /// `/ultraplan` wrote when you chose "save plan to file").
        file: String,

        /// SPEC-ID this plan targets. If omitted, AIDA tries to detect it
        /// from the filename (a `TYPE-N` pattern like `task-42`).
        #[clap(long, value_name = "SPEC-ID")]
        spec: Option<String>,

        /// Land the plan as awaiting-master-review rather than canonical:
        /// tag the spec `plan-review:pending` + post a review-requested
        /// comment. `aida queue work <SPEC>` then warns before pickup.
        // trace:TASK-516 | ai:claude
        #[clap(long)]
        request_review: bool,
    },

    /// Derive a machine-checkable completion condition from AIDA
    /// metadata, ready to paste into `/goal` (or `/schedule`). Each flag
    /// contributes one clause; multiple flags compose with `AND`. Every
    /// clause carries an explicit verification command so a small
    /// evaluator can check it deterministically — no vague "make the
    /// tests pass" conditions that loop forever.
    // trace:TASK-242 | ai:claude
    Goal {
        /// Condition: all specs tagged `batch:<NAME>` reach a terminal
        /// status (Completed/Rejected). `batch:` prefix optional.
        #[clap(long, value_name = "NAME")]
        batch: Option<String>,

        /// Condition: all direct children of EPIC/STORY `<ID>` reach a
        /// terminal status.
        #[clap(long, value_name = "ID")]
        epic: Option<String>,

        /// Condition: spec `<SPEC-ID>` reaches status Completed.
        #[clap(long, value_name = "SPEC-ID")]
        spec: Option<String>,

        /// Condition: PR `<N>` is merged.
        #[clap(long, value_name = "N")]
        pr: Option<u64>,

        /// Condition: the given role's queue is empty. Accepts
        /// `implementer` or `role:implementer`.
        #[clap(long, value_name = "ROLE")]
        queue_empty: Option<String>,

        /// Copy the assembled `/goal` line to the system clipboard.
        #[clap(long, conflicts_with = "invoke")]
        copy: bool,

        /// Print only the bare `/goal <condition>` line, no framing —
        /// for scripting / command substitution.
        #[clap(long)]
        invoke: bool,

        /// Print a `claude-cli://open` deep link with the `/goal …` line
        /// pre-filled. Click → Claude Code opens in cwd with the prompt
        /// ready (inert until Enter). Requires Claude Code v2.1.91+.
        // trace:SPIKE-33 | ai:claude
        #[clap(long = "as-deep-link", conflicts_with_all = ["copy", "invoke"])]
        as_deep_link: bool,
    },

    /// The 'human' role vector — the front door to "what needs a person?".
    /// Bare `aida human [--short]` shows the bottleneck view (every spec a
    /// human must decide, review, or triage), the same set as `aida list
    /// human`, grouped by WHY. `aida human unblock` emits a paste-ready advisor
    /// prompt that grooms the open items keeping themselves out of the
    /// burndown. `aida human away/home/presence` names the operator-presence
    /// verbs under the same role vector while keeping the top-level aliases.
    /// The human is the permanent terminus of the escalation cascade
    /// (implementer → advisor → human); this gives that role a first-class home
    /// symmetric with the agent roles.
    // trace:TASK-746 trace:STORY-563 | ai:claude — plain `//` keeps it out of `--help`.
    Human {
        /// Bare spec-ids, one per line — usable in `$(...)` / xargs (mirrors
        /// `aida list human --short`). Ignored when a subcommand is given.
        // trace:TASK-746 | ai:claude
        #[clap(long)]
        short: bool,
        #[clap(subcommand)]
        command: Option<HumanCommand>,
    },

    /// List the full command surface grouped by topic (same output as
    /// `aida help --all`). Bare `aida` / `aida help` show the curated
    /// Getting-started view instead.
    HelpAll,

    /// Stock and local skill tooling.
    #[clap(subcommand)]
    Skill(SkillCommand),

    /// Discoverable registry of AIDA's built-in shortcuts.
    ///
    /// One place that enumerates every accreted shortcut — the `aida list`
    /// status lenses (open/closed) + status-token shortcuts, the `aida list
    /// <lens>` argv rewrites (queue/why/human/inflight/me/user:<name>), and the
    /// command aliases (intake, advisor assess, bare `aida agent`) — grouped by
    /// surface, each with its canonical expansion + a one-line meaning. Bare
    /// `aida alias` behaves like `aida alias list`. Built-in shortcuts only.
    // trace:STORY-667 | ai:claude
    Alias {
        /// Emit the registry as JSON for machine consumers.
        #[clap(long)]
        json: bool,
        #[clap(subcommand)]
        command: Option<AliasCommand>,
    },
}

/// The `aida alias` verbs. Bare `aida alias` and `aida alias list` print the
/// registry (built-in shortcuts plus your personal/project aliases). `add` /
/// `remove` manage user-defined aliases.
// trace:STORY-667 | ai:claude
// trace:TASK-877 | ai:claude — user-defined alias CRUD verbs.
#[derive(Subcommand, Debug)]
pub enum AliasCommand {
    /// List built-in shortcuts plus your personal/project aliases.
    List {
        /// Emit the registry as JSON for machine consumers.
        #[clap(long)]
        json: bool,
    },
    /// Define a personal or project alias: `aida <name> ...` runs the command.
    ///
    /// Example: `aida alias add approved list --status approved` makes
    /// `aida approved` expand to `aida list --status approved`. Everything
    /// after the name is stored verbatim as the expansion, so put any scope
    /// flag BEFORE the name: `aida alias add --global approved list ...`. An
    /// alias may not shadow a real subcommand. Default scope is the project
    /// when inside an AIDA project, else personal; `--global` / `--project`
    /// force it.
    Add {
        /// The short name typed as `aida <name>`.
        name: String,
        /// The command (after `aida`) the alias expands to. Everything here is
        /// stored verbatim, e.g. `list --status approved`.
        #[clap(trailing_var_arg = true, required = true, num_args = 1..)]
        command: Vec<String>,
        /// Write to the personal store (`~/.aida/aliases.toml`).
        #[clap(long)]
        global: bool,
        /// Write to the project store (`.aida/aliases.toml`, git-trackable).
        #[clap(long)]
        project: bool,
    },
    /// Remove a personal or project alias by name.
    Remove {
        /// The alias name to remove.
        name: String,
        /// Remove from the personal store (`~/.aida/aliases.toml`).
        #[clap(long)]
        global: bool,
        /// Remove from the project store (`.aida/aliases.toml`).
        #[clap(long)]
        project: bool,
    },
}

/// The `aida human` role-vector: the human-tier attention verbs. `unblock` is
/// the deterministic prompt-assembler that ends the recurring "how do I get
/// open items into the burndown?" question by GENERATING the grooming question
/// for the advisor.
/// open items into the burndown?" question by GENERATING the grooming question
/// for the advisor.
// trace:STORY-563 | ai:claude — the role-vector design is SPIKE-57.
#[derive(Subcommand, Debug)]
pub enum HumanCommand {
    /// Mark yourself away from the keyboard. Alias for top-level `aida away`.
    // trace:TASK-770 | ai:codex
    Away,

    /// Mark yourself back at the keyboard. Alias for top-level `aida home`.
    // trace:TASK-770 | ai:codex
    Home,

    /// Show current effective presence. Alias for top-level `aida presence`.
    // trace:TASK-770 | ai:codex
    Presence,

    /// Show current effective presence. Alias for `aida human presence`.
    // trace:TASK-770 | ai:codex
    Status,

    /// Emit a paste-ready advisor prompt to groom the open items that are
    /// keeping themselves out of the burndown ready set. Read-only +
    /// deterministic — no LLM, no writes; it classifies each open spec by
    /// WHAT keeps it out (needs-approval, approved-unqueued, under-specified,
    /// build-supervised, decision-pending, deferred, blocked-by) and assembles
    /// the prompt that tells the advisor to queue the autonomous-able, clarify
    /// the under-specified first, and leave the rest parked. The advisor (the
    /// grooming skill / live session) is the actor the prompt drives.
    // trace:STORY-563 | ai:claude
    Unblock {
        /// Copy the assembled prompt to the system clipboard (like
        /// `aida goal --copy` / `aida ultraplan`). Default prints framed
        /// output to the terminal.
        // trace:STORY-563 | ai:claude
        #[clap(long, conflicts_with_all = ["stdout", "json"])]
        copy: bool,

        /// Print ONLY the bare assembled prompt to stdout, no framing — for
        /// piping / command substitution.
        // trace:STORY-563 | ai:claude
        #[clap(long, conflicts_with = "json")]
        stdout: bool,

        /// Emit the classification as JSON (`[{spec,class,action,reason}]`)
        /// instead of the prompt — for machine consumers / the TUI.
        // trace:STORY-563 | ai:claude
        #[clap(long)]
        json: bool,
    },

    /// Answer a pending decision inline. Thin alias for `aida questions answer
    /// <spec> <choice>` — resolve a decision listed by `aida human` without
    /// switching commands. Pure pass-through; the canonical verb owns the logic.
    // trace:STORY-611 | ai:claude
    Answer {
        /// The spec carrying the pending decision (SPEC-ID or UUID).
        spec: String,
        /// The choice to record (1-based index, or the choice label).
        choice: String,
        /// Attach a counter-proposal note to the answer.
        #[clap(long)]
        note: Option<String>,
    },

    /// Answer a pending decision inline. Alias of `aida human answer` (reads
    /// better for some) — delegates to `aida questions answer`.
    // trace:STORY-611 | ai:claude
    Decide {
        /// The spec carrying the pending decision (SPEC-ID or UUID).
        spec: String,
        /// The choice to record (1-based index, or the choice label).
        choice: String,
        /// Attach a counter-proposal note to the answer.
        #[clap(long)]
        note: Option<String>,
    },

    /// Review a spec inline. Thin alias for `aida review <spec>` — drain a
    /// review listed by `aida human` without switching commands. Pure
    /// pass-through; the canonical verb owns the surface-detection + reviewer.
    // trace:STORY-611 | ai:claude
    Review {
        /// The spec to review (SPEC-ID or UUID).
        spec: String,
        /// Skip launching the reviewer agent — just report the surface.
        #[clap(long)]
        no_agent: bool,
        /// Proceed even if the PR's base branch is stale.
        #[clap(long)]
        allow_stale_base: bool,
    },
}

/// Opt-in statusline bootstrap actions. AIDA's bootstrap goal is to make
/// other projects agent-ready without forcing a house style, so the
/// AIDA-aware statusline is a convenience the user enables deliberately —
/// the bare `aida statusline` render stays the quiet default.
// trace:TASK-0414
#[derive(Subcommand, Debug)]
pub enum StatuslineAction {
    /// Print (or install) client-appropriate statusline configuration so a
    /// user can enable the AIDA-aware statusline segment. Prints by default;
    /// pass `--install` to write the Claude Code `settings.json` entry.
    // trace:TASK-0414
    Setup {
        /// Which client to emit setup for: `claude` (command-backed
        /// statusLine in settings.json), `codex` (built-in TUI footer
        /// fields — Codex does not run arbitrary commands in its footer),
        /// or `all` (default — print guidance for every supported client).
        #[clap(long, value_parser = ["claude", "codex", "all"], default_value = "all")]
        client: String,

        /// Install the configuration instead of only printing it. Only the
        /// `claude` client supports install (it writes/merges the
        /// `statusLine` block into `.claude/settings.json`); other clients
        /// stay print-only because their footer config is hand-edited.
        #[clap(long)]
        install: bool,
    },
}

/// Stock and local skill tooling.
#[derive(Subcommand, Debug)]
pub enum SkillCommand {
    /// Render a stock skill merged with its optional local override (.claude/skills/<name>.md + .local.md) to stdout.
    Render {
        /// The name of the skill to render (e.g. `agent-contract`).
        name: String,
    },

    /// Lint skills that reference an implementation plan. Scans every skill
    /// under `.claude/skills/` for plan references (`docs/plans/*.md`
    /// paths); for each skill that references a plan it runs the same
    /// checks `aida plan verify` runs on each referenced plan (drifted
    /// refs / missing files / absent sections) and a raw-glyph check on
    /// the skill body. A skill pointing at a drifted or missing plan is
    /// an error. Read-only; never rewrites. Exits non-zero on any error so
    /// it can run as a CI gate.
    // trace:TASK-927 | ai:claude
    Lint {
        /// Lint a single skill file instead of the whole `.claude/skills/`
        /// tree. Path may be absolute or relative to the project root.
        #[clap(value_name = "SKILL")]
        skill: Option<PathBuf>,

        /// Emit the result as JSON for agents / scripts.
        #[clap(long)]
        json: bool,

        /// Suppress per-skill OK lines; print only warnings, errors, and
        /// the final verdict. Useful for CI logs.
        #[clap(long, short = 'q')]
        quiet: bool,
    },
}

/// Dependency-inference tooling. Read-only: surfaces likely dependency
/// edges for human confirmation; it never writes the graph itself.
// trace:STORY-447 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DepsCommand {
    /// List likely dependencies inferred from the trace graph, without
    /// writing anything. For each spec, ranks other specs that share
    /// trace-link files (≥2 shared files: high; 1: medium) and, weaker,
    /// share a parent. Confirm a real edge by hand with
    /// `aida edit <id> --blocked-by <dep>`.
    // trace:STORY-447 | ai:claude
    Sweep {
        /// Limit the sweep to a single source spec (SPEC-ID or UUID)
        /// instead of every spec in the store.
        #[clap(long, value_name = "SPEC")]
        for_spec: Option<String>,

        /// Emit the result as JSON for agents / scripts.
        #[clap(long)]
        json: bool,
    },
}

/// Implementation-plan tooling. Plans live in `docs/plans/` and follow
/// the structured template at `docs/plans/_TEMPLATE.md`.
// trace:TASK-92 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum PlanCommand {
    /// Verify a plan file against the structured template: report
    /// drifted `path:line` refs (with suggested corrections), missing
    /// files, and absent required sections. Exits non-zero on any
    /// failure so it can run as a pre-commit hook on `docs/plans/`.
    // trace:TASK-93 | ai:claude
    Verify {
        /// Path to the plan markdown file (e.g.
        /// `docs/plans/2026-05-13-story-86-done-status.md`).
        file: PathBuf,

        /// Rewrite drifted `path:line` refs in place to corrected line
        /// numbers (or symbol form where a symbol is named). Without
        /// this flag, `verify` only reports.
        #[clap(long)]
        fix: bool,

        /// Suppress the per-check OK lines; print only warnings, errors,
        /// and the final verdict. Useful for pre-commit hooks.
        #[clap(long, short = 'q')]
        quiet: bool,
    },

    /// Derive a `## Reusable helpers` section for a spec from the trace
    /// graph: walk sibling specs (same parent), same-feature specs, and
    /// tag-sharing specs, harvest their AIDA trace annotations, and report
    /// the files + helpers they already touch so the implementer reuses
    /// rather than re-invents.
    // trace:TASK-94 | ai:claude
    Helpers {
        /// SPEC-ID of the requirement to derive reusable helpers for.
        spec: String,

        /// Append the generated section to this plan file instead of
        /// printing it to stdout.
        #[clap(long)]
        append: Option<PathBuf>,
    },

    /// Promote Approved spec(s) to Planned when a plan file exists for them
    /// under `docs/plans/` — a file whose `Specs:` header line lists the
    /// SPEC-ID. Formalizes the Approved -> Planned lifecycle step so
    /// plan-ahead work (plan now, implement later — the parallel-pipelining
    /// workflow) is visible in the queue. Pass a SPEC-ID to promote one, or
    /// `--all` to sweep every Approved spec that has a matching plan.
    // trace:STORY-265 | ai:claude
    Promote {
        /// SPEC-ID to promote. Omit and pass `--all` to sweep every
        /// Approved spec that has a plan file.
        spec: Option<String>,

        /// Promote every Approved spec that has a matching plan file.
        #[clap(long, conflicts_with = "spec")]
        all: bool,

        /// Report what would be promoted without writing.
        #[clap(long)]
        dry_run: bool,
    },

    /// Plan-only fan-out over a set of Approved specs: run the plan step
    /// for each spec in the set (sequentially) and promote each
    /// Approved -> Planned once its plan file lands. The set is selected
    /// by `--batch NAME` (every spec tagged `batch:NAME`), `--epic ID`
    /// (every spec tagged `parent:ID`), or an explicit list of SPEC-IDs.
    /// The workable-set discipline drops the low-priority tail by default
    /// (avoids speculative plan-slop on specs that may never be built) —
    /// pass `--include-low` to plan those too. True parallelism is the
    /// harness's job: this driver hands the set to the orchestrating agent
    /// by running each `aida queue work <spec> --plan-only` in turn;
    /// promotion is contention-free pre-work, never a merge.
    // trace:STORY-519 | ai:claude
    FanOut {
        /// Explicit SPEC-IDs to fan out over. Mutually exclusive with
        /// --batch / --epic; pick exactly one selection mode.
        specs: Vec<String>,

        /// Select every Approved spec tagged `batch:NAME`.
        #[clap(long, conflicts_with_all = ["epic", "specs"])]
        batch: Option<String>,

        /// Select every Approved spec tagged `parent:ID` (the epic/story
        /// rollup convention).
        #[clap(long, conflicts_with_all = ["batch", "specs"])]
        epic: Option<String>,

        /// Include low-priority specs in the fan-out. Off by default so
        /// the workable set excludes the low-priority tail.
        #[clap(long)]
        include_low: bool,

        /// Report the resolved set without launching any plan session or
        /// promoting anything.
        #[clap(long)]
        dry_run: bool,

        /// Skip the per-spec plan session launch and only run the
        /// Approved -> Planned promotion for specs that already have a
        /// plan file. Useful after the harness has fanned the plan
        /// sessions out itself and just needs the lifecycle bumps.
        #[clap(long)]
        promote_only: bool,
    },

    /// Synthesize a `docs/plans/` file from a merged/open PR's description
    /// and commit log. For plans authored via the web `/ultraplan` flow
    /// that land a PR directly without ever writing a local plan file —
    /// this reconciles them back into AIDA's plan-archival convention.
    /// Reads `gh pr view <PR> --json title,body,commits,number` plus
    /// `gh pr diff <PR> --name-only`, fills the 11-section template, and
    /// writes `docs/plans/<date>-<slug>-from-pr-<N>.md`. Idempotent —
    /// re-running overwrites the same file deterministically. The output
    /// is shaped to pass `aida plan verify`.
    // trace:TASK-305 | ai:claude
    Capture {
        /// PR number to capture (e.g. `65`). Accepts a bare number or a
        /// `PR-65` / `#65` form.
        pr: String,

        /// Print the synthesized plan to stdout instead of writing a file
        /// under `docs/plans/`.
        #[clap(long)]
        stdout: bool,
    },

    /// Context-grounding pre-plan scan for a spec. Before you generate a
    /// plan (or import a Spec-Kit / OpenSpec-style artifact), this gathers a
    /// read-only snapshot of the code the work will touch: the files +
    /// symbols related specs already trace to (current APIs / architectural
    /// constraints) and a list of likely-stale assumptions — code paths the
    /// spec text names that no longer exist in the tree. Read-only by
    /// default; the result is provenance you can attach to the spec or a
    /// plan file so the grounding travels with the work.
    ///
    /// Compose it with external artifact generators: run the scan first,
    /// hand its summary to Spec Kit / OpenSpec as grounding context, then
    /// `--attach` the provenance so the imported spec records what the tree
    /// actually looked like at plan time.
    // trace:TASK-0418
    Scan {
        /// SPEC-ID (or agreed id / UUID) of the requirement to scan for.
        spec: String,

        /// Attach the scan summary to the spec as a provenance comment.
        /// This is the only write the command performs; without it the
        /// scan is strictly read-only and only prints.
        #[clap(long)]
        attach: bool,

        /// Append the scan summary as a `## Pre-plan scan` section to this
        /// plan file instead of (or in addition to) printing it.
        #[clap(long, value_name = "PATH")]
        append: Option<PathBuf>,

        /// Emit the scan as JSON for machine consumption (e.g. piping into
        /// an external plan/spec generator) instead of the markdown report.
        #[clap(long)]
        json: bool,
    },
}

/// Auto-generated changelog tooling. Mirrors `PlanCommand`'s shape.
// trace:TASK-299 | ai:claude
#[derive(Subcommand, Debug)]
pub enum ChangelogCommand {
    /// Print the full changelog (or a bounded slice) to stdout, or write
    /// it to `--out`. The default scope is every `v*` tag plus an
    /// `[Unreleased]` section for commits since the most recent tag.
    // trace:TASK-956 | ai:claude — folded into the `changelog` surface: hidden
    // from `changelog --help` (the canonical idempotent rewrite is `changelog
    // refresh`, used by release.sh), but the `generate` name still runs its own
    // print-to-stdout logic. It is NOT a true alias of `refresh` — `generate`
    // defaults to stdout over the full history with `--since/--until` range
    // flags, whereas `refresh` writes CHANGELOG.md with `--released-as`, so
    // collapsing it would change the sink and drop the range flags. Hiding
    // preserves the name + behavior, off `--help`.
    #[clap(hide = true)]
    Generate {
        /// Only include releases at or after this tag (inclusive). May
        /// be combined with `--until` to bound the span.
        #[clap(long, value_name = "TAG")]
        since: Option<String>,

        /// Only include releases at or before this tag (inclusive).
        #[clap(long, value_name = "TAG")]
        until: Option<String>,

        /// Write to this file instead of stdout.
        #[clap(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },

    /// Regenerate `CHANGELOG.md` at the repo root with every release.
    /// Idempotent — same git state + spec store → byte-identical output.
    /// Used by `release.sh` to land an up-to-date changelog with the
    /// version-bump commit.
    Refresh {
        /// Render commits-since-the-last-tag under `[<version>] —
        /// <today>` instead of `[Unreleased]`. `release.sh` passes the
        /// about-to-be-tagged version here so the changelog commits
        /// with the bump (the new tag does not exist yet).
        #[clap(long, value_name = "VERSION")]
        released_as: Option<String>,

        /// Write to this file instead of `<repo-root>/CHANGELOG.md`.
        #[clap(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },

    /// Print only the `[Unreleased]` section (commits since the most
    /// recent tag) — a quick "what would land in the next release" view.
    Preview,
}

/// GitHub integration commands
#[derive(Subcommand, Debug)]
pub enum GitHubCommand {
    /// Configure GitHub connection
    Config {
        /// Repository in owner/repo format (e.g., "myorg/myproject")
        #[clap(long)]
        repo: Option<String>,

        /// Personal Access Token (or set AIDA_GITHUB_TOKEN env var)
        #[clap(long)]
        token: Option<String>,

        /// GitHub API URL (default: https://api.github.com)
        #[clap(long)]
        api_url: Option<String>,

        /// Show current configuration
        #[clap(long)]
        show: bool,
    },

    /// Test connection to GitHub
    Test,

    /// List issues from GitHub
    List {
        /// Filter by state (open, closed, all)
        #[clap(long, default_value = "open")]
        state: String,

        /// Filter by labels (comma-separated)
        #[clap(long)]
        labels: Option<String>,

        /// Maximum number of issues to show
        #[clap(long, default_value = "20")]
        limit: u32,
    },

    /// Show a specific GitHub issue
    Show {
        /// Issue number (e.g., 42 or GH-42)
        number: String,
    },

    /// Push a requirement to GitHub as an issue
    Push {
        /// Requirement ID (spec_id or agreed_id)
        id: String,
    },

    /// Pull GitHub issues into AIDA as requirements
    Pull {
        /// Only pull issues with these labels (comma-separated)
        #[clap(long)]
        labels: Option<String>,

        /// Only pull open issues (default: true)
        #[clap(long, default_value = "true")]
        open_only: bool,

        /// Maximum number of issues to pull
        #[clap(long, default_value = "50")]
        limit: u32,

        /// Dry run — show what would be imported without creating requirements
        #[clap(long)]
        dry_run: bool,
    },

    /// Sync: detect drift between AIDA requirements and GitHub issues
    Sync {
        /// Only check linked items (those with [GH-N] prefix or github URL)
        #[clap(long)]
        linked_only: bool,

        /// Apply changes (default: dry-run showing what would change)
        #[clap(long)]
        apply: bool,
    },

    /// List labels in the GitHub repository
    Labels {
        /// Create default AIDA labels if missing
        #[clap(long)]
        create_missing: bool,
    },
}

/// MCP server management.
// trace:STORY-361 | ai:claude
#[derive(Subcommand, Debug)]
pub enum McpCommand {
    /// Register the AIDA MCP server in `.mcp.json` so MCP-speaking agents
    /// (Codex, Cursor, …) can call its coordination tools.
    // trace:STORY-361 | ai:claude
    // trace:TASK-487 | ai:claude
    ///
    /// For local single-machine use the entry runs `aida mcp-serve` over
    /// stdio. Cross-machine MCP is deferred to a follow-up SPIKE; the
    /// printed URL is `stdio://aida` so configs can switch transport later.
    RegisterAgent {
        /// Name to register the AIDA server under (default: "aida").
        #[clap(long, default_value = "aida")]
        name: String,
        /// Print the rendered config and tool surface to stdout instead of
        /// writing `.mcp.json`.
        #[clap(long)]
        print: bool,
        /// Overwrite an existing entry with the same name.
        #[clap(long)]
        force: bool,
    },

    /// Manage and render agent skills
    #[clap(subcommand)]
    Skill(SkillCommand),
}

/// Jira integration commands
#[derive(Subcommand, Debug)]
pub enum JiraCommand {
    /// Configure Jira connection and field mapping
    Config {
        /// Jira Cloud instance URL (e.g., https://myorg.atlassian.net)
        #[clap(long)]
        url: Option<String>,

        /// Jira project key (e.g., AIDA)
        #[clap(long)]
        project: Option<String>,

        /// Email for API authentication
        #[clap(long)]
        email: Option<String>,

        /// Show current configuration
        #[clap(long)]
        show: bool,

        /// Show the field mapping spec
        #[clap(long)]
        show_mapping: bool,
    },

    /// Test connection to Jira
    Test,

    /// List issues from Jira
    List {
        /// JQL query (default: all issues in configured project)
        #[clap(long)]
        jql: Option<String>,

        /// Maximum results
        #[clap(long, default_value = "20")]
        limit: u32,
    },

    /// Show a specific Jira issue
    Show {
        /// Issue key (e.g., PROJ-123)
        key: String,
    },

    /// Push a requirement to Jira as an issue
    Push {
        /// Requirement ID (spec_id or agreed_id)
        id: String,
    },

    /// Sync: detect drift between AIDA requirements and linked Jira issues
    Sync {
        /// Apply changes — push AIDA state to Jira for drifted items
        #[clap(long)]
        apply: bool,
    },

    /// Pull Jira issues into AIDA as requirements
    Pull {
        /// JQL filter (default: all open issues in project)
        #[clap(long)]
        jql: Option<String>,

        /// Maximum issues to pull
        #[clap(long, default_value = "50")]
        limit: u32,

        /// Dry run — show what would be imported
        #[clap(long)]
        dry_run: bool,
    },
}

/// Parse a duration threshold into whole milliseconds, accepting an optional
/// unit suffix. The `--slower-than` flag's help text is `Nms`, which invites
/// users to type `500ms`; bare `500` (the historical form) is still accepted,
/// and `2s` is sugar for `2000`.
///
/// Accepted: `500`, `500ms`, `2s` (case-insensitive, surrounding whitespace
/// trimmed). A bare number is interpreted as milliseconds.
// trace:TASK-1055 — plain `//` so the SPEC-ID stays out of `--help` output.
pub fn parse_duration_ms(raw: &str) -> Result<u64, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("expected a duration like 500, 500ms, or 2s".to_string());
    }
    let lower = s.to_ascii_lowercase();
    let (digits, multiplier) = if let Some(num) = lower.strip_suffix("ms") {
        (num.trim_end(), 1u64)
    } else if let Some(num) = lower.strip_suffix('s') {
        (num.trim_end(), 1000u64)
    } else {
        (lower.as_str(), 1u64)
    };
    let value: u64 = digits.parse().map_err(|_| {
        format!("invalid duration '{raw}' — expected a number optionally suffixed with ms or s (e.g. 500, 500ms, 2s)")
    })?;
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("duration '{raw}' overflows the millisecond range"))
}

// trace:BUG-227 | ai:claude
#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// TASK-1055: `--slower-than` accepts a bare number (ms), an explicit `ms`
    /// suffix, or an `s` suffix (seconds → ms). The help text reads `Nms`,
    /// which used to reject `500ms` with "invalid digit found in string".
    #[test]
    fn parse_duration_ms_accepts_bare_and_suffixed() {
        assert_eq!(parse_duration_ms("500"), Ok(500));
        assert_eq!(parse_duration_ms("500ms"), Ok(500));
        assert_eq!(parse_duration_ms("2s"), Ok(2000));
        // case-insensitive + surrounding whitespace tolerated
        assert_eq!(parse_duration_ms(" 750MS "), Ok(750));
        assert_eq!(parse_duration_ms("0"), Ok(0));
        // garbage and bare units are rejected with a helpful message
        assert!(parse_duration_ms("fast").is_err());
        assert!(parse_duration_ms("ms").is_err());
        assert!(parse_duration_ms("").is_err());
        assert!(parse_duration_ms("1.5s").is_err());
    }

    /// TASK-1055: the parser is actually wired onto the clap flag, so
    /// `usage --events --slower-than 500ms` parses to 500 (not a parse error).
    #[test]
    fn slower_than_flag_accepts_ms_suffix() {
        let cli = Cli::try_parse_from(["aida", "usage", "--events", "--slower-than", "500ms"])
            .expect("`--slower-than 500ms` must parse");
        match cli.command {
            Command::Usage { slower_than, .. } => {
                assert_eq!(slower_than, Some(500));
            }
            other => panic!("expected Usage command, got {other:?}"),
        }
    }

    // True when `s` contains a real AIDA trace marker — the literal `trace:`
    // token followed by an uppercase SPEC-ID prefix, a dash, and a digit (e.g.
    // STORY-62, TASK-1-021). Legit help-text mentions of the trace machinery
    // use placeholders (REQ-ID, <SPEC-ID>) with no digit after the dash, so
    // this discriminates leaked breadcrumbs from prose.
    fn has_trace_marker(s: &str) -> bool {
        let mut search_from = 0;
        while let Some(pos) = s[search_from..].find("trace:") {
            let after = search_from + pos + "trace:".len();
            let rest = &s[after..];
            let prefix_len = rest.bytes().take_while(u8::is_ascii_uppercase).count();
            let tail = &rest[prefix_len..];
            if prefix_len > 0
                && tail
                    .strip_prefix('-')
                    .and_then(|t| t.chars().next())
                    .is_some_and(|c| c.is_ascii_digit())
            {
                return true;
            }
            search_from = after;
        }
        false
    }

    // Walk the clap command tree, recording every help/about string that
    // carries a leaked SPEC-ID trace marker.
    fn collect_leaks(cmd: &clap::Command, path: &str, leaks: &mut Vec<String>) {
        let here = if path.is_empty() {
            cmd.get_name().to_string()
        } else {
            format!("{path} {}", cmd.get_name())
        };
        let mut check = |label: &str, text: Option<String>| {
            if let Some(t) = text {
                if has_trace_marker(&t) {
                    leaks.push(format!("{here} :: {label}: {t}"));
                }
            }
        };
        check("about", cmd.get_about().map(ToString::to_string));
        check("long_about", cmd.get_long_about().map(ToString::to_string));
        check(
            "before_help",
            cmd.get_before_help().map(ToString::to_string),
        );
        check("after_help", cmd.get_after_help().map(ToString::to_string));
        for arg in cmd.get_arguments() {
            let id = arg.get_id();
            check(
                &format!("--{id} help"),
                arg.get_help().map(ToString::to_string),
            );
            check(
                &format!("--{id} long_help"),
                arg.get_long_help().map(ToString::to_string),
            );
        }
        for sub in cmd.get_subcommands() {
            collect_leaks(sub, &here, leaks);
        }
    }

    #[test]
    fn trace_marker_detector_discriminates_markers_from_prose() {
        assert!(has_trace_marker("limit allows. trace:STORY-59 | ai:claude"));
        assert!(has_trace_marker("trace:TASK-1-021"));
        assert!(has_trace_marker("see trace:BUG-87 here"));
        // Legit help-text mentions — placeholders with no digit after the dash.
        assert!(!has_trace_marker(
            "Scan source files for trace comments (// trace:REQ-ID format)"
        ));
        assert!(!has_trace_marker(
            "Walk the project root for `trace:<SPEC-ID>`"
        ));
        assert!(!has_trace_marker("remove `trace:<DANGLING>` annotations"));
        assert!(!has_trace_marker("no marker here at all"));
    }

    // Regression test for BUG-227: SPEC-IDs are developer breadcrumbs and must
    // never reach `--help`. A `///` doc comment on a clap field/variant doubles
    // as help text, so any trace marker on one leaks — keep it a plain `//`.
    #[test]
    fn help_text_carries_no_trace_markers() {
        let mut leaks = Vec::new();
        collect_leaks(&Cli::command(), "", &mut leaks);
        assert!(
            leaks.is_empty(),
            "SPEC-ID trace markers leaked into `--help` output — demote the \
             offending `///` doc comment to a plain `//` comment:\n  {}",
            leaks.join("\n  ")
        );
    }

    // Source-side hygiene check: no `///` doc comment in this file may contain
    // the literal `trace:` token. Catches potential leaks before clap builds
    // the tree (covers hidden subcommands, enum-level docs, even commented-out
    // `///` blocks that the runtime walker can't see). Pairs with
    // `help_text_carries_no_trace_markers` — runtime + source, belt + braces.
    #[test]
    fn source_doc_comments_carry_no_trace_token() {
        let src = include_str!("cli.rs");
        let offenders: Vec<(usize, &str)> = src
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                let trimmed = line.trim_start();
                trimmed.starts_with("///") && trimmed.contains("trace:")
            })
            .map(|(i, line)| (i + 1, line))
            .collect();
        assert!(
            offenders.is_empty(),
            "`///` doc comments in cli.rs must not contain the `trace:` token \
             (placeholders or real markers — both leak into `--help` output \
             when on a clap field). Demote to a plain `//` line above the \
             item, or reword the prose to drop the literal token:\n{}",
            offenders
                .iter()
                .map(|(n, l)| format!("  {n}: {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    // TASK-287 / BUG-629: a `///` clap doc comment doubles as `--help` text, so
    // *provenance* on one (a `trace:` marker, or a bare SPEC-ID standing in for
    // the developer breadcrumb) leaks the id into user-facing output. But a
    // *descriptive* mention of a SPEC-ID inside prose (e.g. "reuses the
    // STORY-122 usage log") is legitimate help text, not provenance — BUG-629
    // tightened the criterion from "any SPEC-ID token" (over-broad; it forced
    // agents to reword legit prose) down to "provenance only". The discriminator
    // is `doc_comment_is_provenance_leak`, mirrored verbatim by the fast
    // grep-based pre-commit hook (TASK-903) so the gate and CI agree. (This
    // comment uses `//`, not `///`, so it isn't itself scanned.)
    #[test]
    fn source_doc_comments_carry_no_spec_id_provenance() {
        let src = include_str!("cli.rs");
        let offenders: Vec<(usize, &str)> = src
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                let trimmed = line.trim_start();
                trimmed.starts_with("///") && doc_comment_is_provenance_leak(trimmed)
            })
            .map(|(i, line)| (i + 1, line))
            .collect();
        assert!(
            offenders.is_empty(),
            "`///` doc comments in cli.rs must not carry SPEC-ID provenance — a \
             `trace:` marker or a *bare* SPEC-ID (it leaks into `--help`). Move \
             the id to a `//` trace marker above the item, or reword as prose. A \
             descriptive prose mention of a SPEC-ID is allowed:\n{}",
            offenders
                .iter()
                .map(|(n, l)| format!("  {n}: {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    // BUG-629 / TASK-903: the discriminator shared by the CI provenance test
    // above and the grep-based pre-commit hook. `line` is a `///`-prefixed doc
    // line (already trimmed). It is a *provenance leak* — and so rejected — when
    // it carries a `trace:` marker, OR is a *bare* SPEC-ID (the line is
    // essentially nothing but SPEC-ID token(s) + punctuation, no descriptive
    // prose words). A descriptive prose mention of a SPEC-ID is NOT a leak.
    //
    // Mirror any change here into aida-core/templates/hooks/aida-pre-commit.sh
    // and the embedded fallback in aida-core/src/scaffolding/hooks.rs so the
    // fast gate and CI stay in lockstep.
    fn doc_comment_is_provenance_leak(line: &str) -> bool {
        let spec_id =
            regex::Regex::new(r"\b(STORY|TASK|BUG|EPIC|SPIKE|FR|CR|SPEC|ADR|PRIN)-[0-9]+").unwrap();
        // No SPEC-ID at all → nothing to leak.
        if !spec_id.is_match(line) {
            return false;
        }
        // A `trace:` marker on a `///` line is always provenance.
        if line.contains("trace:") {
            return true;
        }
        // Strip the `///` prefix, then remove every SPEC-ID token. What remains
        // is the surrounding text. If it still contains alphabetic words, this
        // is a descriptive mention (prose) — allow. If only punctuation /
        // whitespace / digits remain, the line is a *bare* SPEC-ID — reject.
        let body = line.trim_start_matches('/').trim();
        let residual = spec_id.replace_all(body, " ");
        let has_prose_word = residual.split_whitespace().any(|tok| {
            // A "word" is a token with two or more ascii-alphabetic chars, so a
            // stray "a"/"x" or pure punctuation doesn't count as prose.
            tok.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 2
        });
        !has_prose_word
    }

    // BUG-629: reject only provenance, allow descriptive prose. Reject and allow
    // cases use the same discriminator the CI scan and the hook share.
    #[test]
    fn doc_comment_provenance_leak_discriminates_provenance_from_prose() {
        // REJECT — trace: marker on a `///` line (the --help-leak trap).
        assert!(doc_comment_is_provenance_leak("/// trace:TASK-955"));
        assert!(doc_comment_is_provenance_leak(
            "/// resolve the path. trace:BUG-448 | ai:claude"
        ));
        // REJECT — bare SPEC-ID provenance, no surrounding prose.
        assert!(doc_comment_is_provenance_leak("/// TASK-955"));
        assert!(doc_comment_is_provenance_leak("/// BUG-448"));
        assert!(doc_comment_is_provenance_leak("/// (STORY-122, TASK-903)"));
        assert!(doc_comment_is_provenance_leak("/// FR-0042"));

        // ALLOW — a descriptive prose mention of a SPEC-ID is legitimate help
        // text, not provenance.
        assert!(!doc_comment_is_provenance_leak(
            "/// reuses the STORY-122 usage log"
        ));
        assert!(!doc_comment_is_provenance_leak(
            "/// reads the SPIKE-67 field-study log"
        ));
        assert!(!doc_comment_is_provenance_leak(
            "/// (e.g. `TASK-489` — treated as `--spec`)"
        ));
        // ALLOW — no SPEC-ID at all.
        assert!(!doc_comment_is_provenance_leak("/// Mark a spec done"));
    }

    // trace:TASK-0415 — the positional status shortcut parses into the List
    // command's `shortcut` field; `--status` still parses into `status`.
    #[test]
    fn list_positional_status_shortcut_parses() {
        let cli = Cli::try_parse_from(["aida", "list", "approved"]).unwrap();
        match cli.command {
            Command::List {
                shortcut, status, ..
            } => {
                assert_eq!(shortcut.as_deref(), Some("approved"));
                assert_eq!(status, None);
            }
            other => panic!("expected List, got {other:?}"),
        }

        let cli = Cli::try_parse_from(["aida", "list", "--status", "open"]).unwrap();
        match cli.command {
            Command::List {
                shortcut, status, ..
            } => {
                assert_eq!(shortcut, None);
                assert_eq!(status.as_deref(), Some("open"));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    // trace:STORY-662 — the `--user <name>` flag parses, and the positional
    // `me` / `user:<name>` tokens land in `shortcut` (peeled into the user
    // filter at runtime, not at the clap layer).
    #[test]
    fn list_user_flag_and_positional_parse() {
        // --user flag.
        let cli = Cli::try_parse_from(["aida", "list", "--user", "joe"]).unwrap();
        match cli.command {
            Command::List { user, shortcut, .. } => {
                assert_eq!(user.as_deref(), Some("joe"));
                assert_eq!(shortcut, None);
            }
            other => panic!("expected List, got {other:?}"),
        }

        // Positional `me`.
        let cli = Cli::try_parse_from(["aida", "list", "me"]).unwrap();
        match cli.command {
            Command::List { shortcut, user, .. } => {
                assert_eq!(shortcut.as_deref(), Some("me"));
                assert_eq!(user, None);
            }
            other => panic!("expected List, got {other:?}"),
        }

        // Positional `user:<name>`.
        let cli = Cli::try_parse_from(["aida", "list", "user:alice"]).unwrap();
        match cli.command {
            Command::List { shortcut, user, .. } => {
                assert_eq!(shortcut.as_deref(), Some("user:alice"));
                assert_eq!(user, None);
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    // TASK-955: `--recursive` parses only WITH `--parent` (clap `requires`),
    // composes with the positional [STATUS], and the `--subtree` alias resolves
    // to the same flag. trace:TASK-955
    #[test]
    fn list_recursive_requires_parent() {
        // --recursive + --parent + a status positional: parses, recursive=true.
        let cli =
            Cli::try_parse_from(["aida", "list", "open", "--parent", "EPIC-51", "--recursive"])
                .unwrap();
        match cli.command {
            Command::List {
                shortcut,
                parent,
                recursive,
                ..
            } => {
                assert_eq!(shortcut.as_deref(), Some("open"));
                assert_eq!(parent.as_deref(), Some("EPIC-51"));
                assert!(recursive);
            }
            other => panic!("expected List, got {other:?}"),
        }

        // The `--subtree` alias resolves to the same `recursive` field.
        let cli =
            Cli::try_parse_from(["aida", "list", "--parent", "EPIC-51", "--subtree"]).unwrap();
        match cli.command {
            Command::List { recursive, .. } => assert!(recursive),
            other => panic!("expected List, got {other:?}"),
        }

        // --recursive WITHOUT --parent errors (the requires-parent guard).
        let err = Cli::try_parse_from(["aida", "list", "--recursive"]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--parent"),
            "error should name the missing --parent requirement: {msg}"
        );
    }

    // trace:STORY-562 — `aida list human` parses as the positional shortcut and
    // `aida list --human` sets the flag; both route to the human-attention view.
    #[test]
    fn list_human_positional_and_flag_parse() {
        let cli = Cli::try_parse_from(["aida", "list", "human"]).unwrap();
        match cli.command {
            Command::List {
                shortcut, human, ..
            } => {
                assert_eq!(shortcut.as_deref(), Some("human"));
                assert!(!human);
            }
            other => panic!("expected List, got {other:?}"),
        }

        let cli = Cli::try_parse_from(["aida", "list", "--human"]).unwrap();
        match cli.command {
            Command::List {
                shortcut, human, ..
            } => {
                assert_eq!(shortcut, None);
                assert!(human);
            }
            other => panic!("expected List, got {other:?}"),
        }

        // Composes with `--short`.
        let cli = Cli::try_parse_from(["aida", "list", "human", "--short"]).unwrap();
        match cli.command {
            Command::List {
                shortcut,
                human,
                short,
                ..
            } => {
                assert_eq!(shortcut.as_deref(), Some("human"));
                assert!(!human);
                assert!(short);
            }
            other => panic!("expected List, got {other:?}"),
        }

        // `--human` conflicts with `--status` / `--json` / `--tree` at the clap layer.
        assert!(Cli::try_parse_from(["aida", "list", "--human", "--json"]).is_err());
        assert!(Cli::try_parse_from(["aida", "list", "--human", "--status", "draft"]).is_err());
    }

    // trace:TASK-900 — `aida list --limit N` parses into the optional
    // `limit` field; absent it is None (full listing). The handler applies
    // the cap AFTER the recency-first sort, so N most-recent come back.
    #[test]
    fn list_limit_flag_parses() {
        // Absent → None (no cap).
        let cli = Cli::try_parse_from(["aida", "list"]).unwrap();
        match cli.command {
            Command::List { limit, .. } => assert_eq!(limit, None),
            other => panic!("expected List, got {other:?}"),
        }

        // Present → Some(N).
        let cli = Cli::try_parse_from(["aida", "list", "--limit", "5"]).unwrap();
        match cli.command {
            Command::List { limit, .. } => assert_eq!(limit, Some(5)),
            other => panic!("expected List, got {other:?}"),
        }

        // Composes with other filters.
        let cli =
            Cli::try_parse_from(["aida", "list", "--status", "open", "--limit", "3"]).unwrap();
        match cli.command {
            Command::List { limit, status, .. } => {
                assert_eq!(limit, Some(3));
                assert_eq!(status.as_deref(), Some("open"));
            }
            other => panic!("expected List, got {other:?}"),
        }

        // Non-numeric value is a parse error.
        assert!(Cli::try_parse_from(["aida", "list", "--limit", "notanumber"]).is_err());
    }

    // trace:TASK-900 — the truncation semantics the list handler relies on:
    // `Vec::truncate(N)` keeps the FIRST N of the already-sorted (recency-first)
    // vector, i.e. the N most-recent. `N >= len` is a no-op (full listing).
    #[test]
    fn list_limit_truncates_to_n_most_recent() {
        // Stand-in for the recency-first `reqs` vector (freshest first).
        let sorted = vec!["newest", "newer", "mid", "older", "oldest"];

        let mut got = sorted.clone();
        got.truncate(3);
        assert_eq!(got, vec!["newest", "newer", "mid"]);

        // limit >= len → unchanged.
        let mut all = sorted.clone();
        all.truncate(99);
        assert_eq!(all, sorted);

        // limit 0 → empty.
        let mut none = sorted.clone();
        none.truncate(0);
        assert!(none.is_empty());
    }

    // trace:TASK-770 | ai:codex
    #[test]
    fn human_presence_aliases_parse() {
        for (word, want) in [
            ("away", "away"),
            ("home", "home"),
            ("presence", "presence"),
            ("status", "status"),
        ] {
            let cli = Cli::try_parse_from(["aida", "human", word]).unwrap();
            match cli.command {
                Command::Human {
                    short: false,
                    command: Some(cmd),
                } => {
                    let got = match cmd {
                        HumanCommand::Away => "away",
                        HumanCommand::Home => "home",
                        HumanCommand::Presence => "presence",
                        HumanCommand::Status => "status",
                        HumanCommand::Unblock { .. } => "unblock",
                        // trace:STORY-611 | ai:claude — action aliases.
                        HumanCommand::Answer { .. } => "answer",
                        HumanCommand::Decide { .. } => "decide",
                        HumanCommand::Review { .. } => "review",
                    };
                    assert_eq!(got, want);
                }
                other => panic!("expected human {word}, got {other:?}"),
            }
        }
    }

    // trace:TASK-1005 / SPIKE-70 — `--sequential` parses on `queue work`,
    // requires an autonomous drain, and conflicts with `--single-branch`.
    #[test]
    fn sequential_flag_parses_with_batch_and_autonomous() {
        let cli = Cli::try_parse_from([
            "aida",
            "queue",
            "work",
            "--batch",
            "tui",
            "--auto-complete",
            "--sequential",
        ])
        .unwrap();
        match cli.command {
            Command::Queue(QueueCommand::Work {
                sequential,
                single_branch,
                batch,
                ..
            }) => {
                assert!(sequential, "--sequential should parse as true");
                assert!(!single_branch);
                assert_eq!(batch.as_deref(), Some("tui"));
            }
            other => panic!("expected queue work, got {other:?}"),
        }
    }

    // trace:TASK-1005 — `--sequential` is autonomous-only; without
    // `--auto-complete` (or `--drain`) clap rejects it via `requires`.
    #[test]
    fn sequential_requires_autonomous_drain() {
        let err = Cli::try_parse_from(["aida", "queue", "work", "--batch", "tui", "--sequential"])
            .unwrap_err();
        assert!(
            err.to_string().contains("autonomous")
                || err.to_string().contains("auto-complete")
                || err.to_string().contains("required"),
            "expected a requires-autonomous error, got: {err}"
        );
    }

    // trace:TASK-1005 — `--sequential` (per-member PRs) and `--single-branch`
    // (one accumulating branch) are mutually exclusive drive shapes.
    #[test]
    fn sequential_conflicts_with_single_branch() {
        let err = Cli::try_parse_from([
            "aida",
            "queue",
            "work",
            "--batch",
            "tui",
            "--auto-complete",
            "--sequential",
            "--single-branch",
        ])
        .unwrap_err();
        assert!(
            err.to_string().contains("cannot be used with")
                || err.to_string().contains("single-branch"),
            "expected a conflicts-with error, got: {err}"
        );
    }

    // trace:TASK-1005 — the sequential mode pins concurrency to 1; the named
    // invariant the dispatch relies on.
    #[test]
    fn sequential_pins_concurrency_to_one() {
        assert_eq!(crate::SEQUENTIAL_DRAIN_CONCURRENCY, 1);
    }
}
