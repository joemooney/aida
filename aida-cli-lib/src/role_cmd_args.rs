//! `aida role_cmd` clap types, owned by `role_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

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
    /// The seat grant is bound to this terminal session: any process in this
    /// terminal session can use this grant, and no process outside it can.
    Enter {
        /// Role name — picker shown when omitted on a TTY
        name: Option<String>,

        /// Restore the role's last working directory (cd in the eval output)
        #[clap(long)]
        cd: bool,

        /// Skip the interactive recent-session resume prompt
        // trace:STORY-821 | ai:codex
        #[clap(long)]
        no_resume: bool,

        /// Do not set the terminal/tab title for this role enter.
        // trace:STORY-994 | ai:codex
        #[clap(long)]
        no_title: bool,

        /// Explicitly allow this session to issue child grants for this seat.
        /// May be repeated; omitted means this session cannot delegate seats.
        // trace:STORY-1473, TASK-1592 | ai:antigravity
        #[clap(long = "delegate-seat")]
        delegate_seats: Vec<String>,

        /// Grant time-to-live duration (e.g. 72h), capped by [seat] max_grant_hours (default 24, hard ceiling 168h).
        // trace:TASK-1787 | ai:antigravity
        #[clap(long)]
        ttl: Option<String>,

        /// Bind the grant to another running session instead of this
        /// terminal: the pid of that session's agent harness (claude, codex,
        /// agy) or shell, owned by you (`aida role status` shows a session's
        /// harness pid). Prints the grant id for that session to export and
        /// leaves this shell's role unchanged. Binds the process only: that
        /// session's hooks and MCP server can present the grant, but not its
        /// sandboxed tool calls, which need a session started from a role
        /// shell.
        // trace:BUG-1934 | ai:claude
        #[clap(long = "bind-pid", value_name = "PID")]
        bind_pid: Option<u32>,
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
    /// stdout when no role is active. Resolves the validated grant
    /// — no project-store load. Shell-friendly counterpart to
    /// `git branch --show-current`.
    // trace:TASK-42 trace:TASK-1594 | ai:claude
    Active,

    /// Print the active role's name on stdout (empty line when no role is
    /// active) and exit 0 either way. With `--check`, exit 1 instead when
    /// no role is active (still printing the name when one is). Resolves
    /// the validated grant — no project-store load. Scripting-friendly
    /// surface for agents without direct env access.
    // trace:STORY-64 | ai:claude
    Current {
        /// Exit 1 (instead of 0) when no role is active.
        #[clap(long)]
        check: bool,
    },

    /// Report how close this session's seat grant is to expiry. By default
    /// prints a one-line warning on stdout when the grant expires within
    /// `--warn-within` (or has expired), and nothing otherwise; always exits
    /// 0. With `--refuse-within`, prints the refusal on stderr and exits 3
    /// when less than that is left: a relay runs it before launching each
    /// seat session; it also exits 3 when `AIDA_SESSION_GRANT` is set but its
    /// grant can't be used (missing, malformed, or revoked). No grant at all,
    /// or an implementer grant, is never refused.
    // trace:TASK-1736 | ai:claude
    Expiry {
        /// Warn when the grant expires within this span (e.g. `2h`, `90m`).
        #[clap(long, default_value = "2h")]
        warn_within: String,

        /// Exit 3 with a refusal when less than this span is left.
        #[clap(long)]
        refuse_within: Option<String>,
    },

    /// Show this session's seat grant (seat, expiry, the processes it is
    /// bound to, and whether and how this process may present it: by its
    /// session key or by process) and this session's agent harness pid for
    /// `aida role enter <seat> --bind-pid`. Read-only; always exits 0.
    // trace:BUG-1934 | ai:claude
    // trace:TASK-2159 | ai:claude
    Status {
        /// Emit JSON instead of text.
        #[clap(long)]
        json: bool,
    },

    /// Install a seat grant's session key into the calling shell's session
    /// keyring. Run by the `aida role enter` eval text with the secret on
    /// stdin; not for direct use.
    // trace:BUG-1934 | ai:claude
    #[clap(hide = true)]
    AdoptKey {
        /// The grant id.
        id: String,
    },

    /// Emit a deduplicated informational tool-hook expiry notice.
    // trace:BUG-2014 | ai:codex
    // trace:TASK-1744 | ai:agy
    #[clap(hide = true)]
    ExpiryNotice {
        /// Target hook client protocol (`claude`, `codex`, or `agy`).
        #[clap(long, default_value = "claude")]
        client: String,
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

    /// Install a starter set of global roles (implementer, product, advisor,
    /// reviewer, integrator) at ~/.aida/roles/.
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
        // trace:BUG-1294 | ai:claude
        #[clap(long, conflicts_with = "content", allow_hyphen_values = true)]
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
