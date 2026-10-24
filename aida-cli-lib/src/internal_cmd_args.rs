//! `aida internal_cmd` clap types, owned by `internal_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

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

    /// Enforce the advisor-lock gate at the commit boundary. Called by the
    /// scaffolded git pre-commit hook so the gate binds any vendor's commit,
    /// not just Claude. Exits non-zero (aborting the commit) only under
    /// `[locking] posture = "enforce"` when this worktree is locked by a
    /// different advisor than the committing session's token. Silent under
    /// the default `off` posture.
    // trace:TASK-1140 | ai:claude
    LockingGate,

    /// Warn when a worktree-scoped session's commit touches paths outside its
    /// session worktree. Called by the scaffolded git pre-commit hook so the
    /// check binds any vendor's commit, not just Claude. Warn-only: it names
    /// the stray paths on stderr and always exits 0, so it can never abort a
    /// commit. Silent for a commit inside the session worktree and for a
    /// session that is not worktree-scoped.
    // trace:TASK-1178 | ai:claude
    WorktreeScopeGate,

    /// Query effective solo mode for the Claude advisor-code-guard PreToolUse
    /// hook. Exits 0 when solo mode is active and 1 when it is off; prints
    /// nothing.
    // trace:BUG-1748 | ai:codex
    SoloActive,

    /// Print the environment correction for a session inside a lane
    /// worktree: shell lines setting AIDA_SESSION_ROLE to the role its lease
    /// records, and exporting the lease's child grant or unsetting an
    /// inherited grant for another seat (and the inherited purpose). Prints
    /// nothing outside a lane or when the environment already matches.
    /// Read-only. Called by the SessionStart hook, which appends the lines to
    /// $CLAUDE_ENV_FILE. Exits non-zero, printing nothing, when the lease
    /// cannot be turned into safe lines.
    // trace:STORY-1662 | ai:claude
    LaneEnv {
        /// The session directory to resolve the lane for (default: cwd).
        #[clap(long, value_name = "PATH")]
        cwd: Option<PathBuf>,
    },
}
