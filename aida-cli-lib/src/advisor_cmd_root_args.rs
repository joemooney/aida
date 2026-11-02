//! `aida advisor_cmd_root` clap types, owned by `advisor_cmd_root.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Live-advisor registration. The `--no-human=both` orchestrator reads
/// `~/.aida/advisor.toml` to decide whether to **fork** the live advisor
/// session (full in-flight context) instead of **cold-booting** a fresh
/// headless advisor (substrate-only). Register once per advisor session;
/// `unregister` clears the file.
// trace:STORY-360 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum AdvisorCommand {
    /// Record the current Claude or Codex session as the live advisor — writes
    /// `~/.aida/advisor.toml`. With no `--uuid`, reads `CLAUDE_CODE_SESSION_ID`
    /// (Claude Code sets it on every session), else `CODEX_THREAD_ID` (a Codex
    /// thread, which `advisor watch` forks with `codex exec fork`). Pass
    /// `--uuid <id>` to register a specific Claude session id.
    // trace:STORY-2255 | ai:claude
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

    /// Presence-gated fork-from-live watch loop: while `away`, fork the live
    /// advisor only when `aida awaiting` is non-empty and run a garden +
    /// mailbox-triage + escalate pass headless. Opt-in by invocation; exits on
    /// `aida home` (or away-TTL). The forked advisor only does safe/bounded work
    /// and escalates the rest.
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

        /// Deprecated compatibility flag. Forks are driven by `aida awaiting`,
        /// not elapsed time.
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
        // trace:BUG-1294 | ai:claude
        #[clap(long, value_name = "TOPIC", allow_hyphen_values = true)]
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
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        template: String,

        /// TASK description (optional).
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
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
