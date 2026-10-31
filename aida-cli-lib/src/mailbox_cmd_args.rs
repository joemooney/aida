//! `aida mailbox_cmd` clap types, owned by `mailbox_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// What `aida mailbox wake-service` does.
// trace:STORY-HOME-096 | ai:claude
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeServiceAction {
    Install,
    Uninstall,
    Status,
}

/// Inter-agent mailbox — peer↔peer messaging between agents (distinct from
/// operator→agent briefs and top-down directives). Hybrid storage: a fast
/// local layer now, a git-canonical durable digest in a later slice.
// trace:STORY-493 | ai:claude
#[derive(Subcommand, Debug)]
pub enum MailboxCommand {
    /// Read bounded unread message IDs without bodies or acknowledgement.
    // trace:TASK-2347 | ai:codex
    // trace:STORY-1843 | ai:claude
    WakeSnapshot {
        /// Checkout the logical mail view is read from (defaults to the main
        /// checkout); every worktree of its repository is included.
        #[clap(long)]
        local_root: Option<PathBuf>,
        /// One of this process's own inboxes; narrows, never widens, the
        /// audience a notice would read.
        #[clap(long, default_value = "advisor")]
        recipient: String,
        /// Print the versioned read-only envelope as JSON.
        #[clap(long)]
        json: bool,
    },
    /// Watch every session armed for mail wake and wake its Codex thread
    /// when its own inbox has new mail (the service `wake-service` installs).
    // trace:STORY-HOME-096 | ai:claude
    #[clap(group(
        ArgGroup::new("recovery")
            .args(["accepted", "retry"])
            .multiple(false)
            .requires("recover")
    ))]
    WakeRun {
        /// Project whose armed sessions are watched (defaults to this one).
        #[clap(long)]
        project: Option<PathBuf>,
        /// The `codex` binary that queues the wake (defaults to PATH).
        #[clap(long)]
        codex: Option<PathBuf>,
        /// Seconds between passes (1..3600).
        #[clap(long, default_value_t = 30)]
        interval: u64,
        /// Seconds new mail waits so one wake can carry a burst (0..300).
        #[clap(long, default_value_t = 5.0)]
        batch_seconds: f64,
        /// Run one pass and print each armed session's result.
        #[clap(long)]
        once: bool,
        /// Resolve an uncertain wake on this Codex thread, after inspecting it.
        /// Exactly one of --accepted or --retry is required with it.
        #[clap(long, value_name = "THREAD", requires = "recovery")]
        recover: Option<String>,
        /// With --recover: the thread received the wake.
        #[clap(long)]
        accepted: bool,
        /// With --recover: the thread did not receive it; offer it again.
        #[clap(long)]
        retry: bool,
    },
    /// Install, remove or inspect this project's mail-wake user service.
    // trace:STORY-HOME-096 | ai:claude
    WakeService {
        #[clap(value_enum)]
        action: WakeServiceAction,
        /// The `codex` binary the service uses (defaults to PATH at install).
        #[clap(long)]
        codex: Option<PathBuf>,
        /// Print what would change without touching systemd.
        #[clap(long)]
        dry_run: bool,
    },
    /// Preview legacy mailbox reconciliation before recipient migration.
    // trace:STORY-1843 | ai:codex
    Migrate {
        /// Produce a read-only report. Write-mode migration is not available.
        #[clap(long, required = true)]
        dry_run: bool,
        /// Print the reconciliation report as JSON.
        #[clap(long)]
        json: bool,
    },

    /// Mail retention: check whether worktree mail can be retired safely.
    // trace:STORY-1843 | ai:claude
    Retention {
        #[clap(subcommand)]
        command: RetentionCommand,
    },

    /// Show mailbox read latency for the active identity (or an explicit recipient).
    // trace:TASK-1271 | ai:codex
    Latency {
        #[clap(long = "for")]
        recipient: Option<String>,
        #[clap(long)]
        json: bool,
    },

    /// Send a message to another agent, or `--broadcast` to all.
    Send {
        /// Recipient agent id (e.g. `codex`). Omit and pass `--broadcast` to reach everyone.
        #[clap(long)]
        to: Option<String>,

        /// Broadcast to every agent instead of a single recipient.
        #[clap(long, conflicts_with = "to")]
        broadcast: bool,

        /// The message body. For rich multi-line text, prefer --body-file or --stdin so the shell
        /// cannot expand backticks or `$()` before AIDA receives the text.
        body: Option<String>,

        /// Short subject shown by inbox and notice views.
        #[clap(long, value_name = "SUBJECT")]
        subject: Option<String>,

        /// Read the message body from a file instead of a shell argument.
        #[clap(long, value_name = "PATH")]
        body_file: Option<PathBuf>,

        /// Read the message body from stdin instead of a shell argument.
        #[clap(long)]
        stdin: bool,

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

        /// The seat whose claim this message relays. Use it when the body
        /// reproduces another seat's measurement, finding or argument; the
        /// reader then sees "via <you>, originally <seat>". Omit it for
        /// claims you verified yourself.
        // trace:BUG-1534 | ai:claude
        #[clap(long, value_name = "SEAT")]
        relayed_from: Option<String>,

        /// Send a body that says "you"/"your" even though another recipient
        /// already received the same body (or this is a broadcast). By
        /// default that send is refused, because each reader takes the claim
        /// as addressed to itself.
        // trace:BUG-1534 | ai:claude
        #[clap(long)]
        allow_second_person: bool,

        /// When the role in --to has no reader right now, queue the message
        /// for it: the first eligible reader to check its mail takes it,
        /// once. Without this, a send to a role with no reader is refused.
        /// A role that has readers delivers to them as usual.
        // trace:STORY-1843 | ai:claude
        #[clap(long, requires = "to")]
        queue_for_role: bool,
    },

    /// Show your inbox: mail addressed to this session's own inboxes (its
    /// handle or user, role and agent type) + broadcasts, from every worktree
    /// of this repository and the canonical store. Reading marks the shown
    /// unread mail seen; `--all` is the operator-wide read-only view.
    // trace:STORY-1843 | ai:claude
    Inbox {
        /// Narrow to one of your own inboxes. Naming any other inbox is a
        /// read-only inspection of this checkout and the store; nothing is
        /// marked.
        agent: Option<String>,

        /// Operator-wide view: every message across all agents, oldest-first,
        /// with its recipient shown. Read-only — does not mark anything seen.
        #[clap(long, conflicts_with = "agent")]
        all: bool,

        /// Show only archived messages. Read-only — does not mark anything seen.
        // trace:TASK-1211 | ai:codex
        #[clap(long, conflicts_with_all = ["all", "unread"])]
        archived: bool,

        /// Show the inbox WITHOUT marking it seen (writes no read receipt).
        /// Lets a hook or a glance surface mail without
        /// consuming the unread flag — reading/acking stays an explicit act
        /// (a plain `aida mailbox inbox`).
        // trace:STORY-585
        #[clap(long, alias = "no-mark")]
        peek: bool,

        /// Show only UNREAD messages (no read receipt; mail an older version
        /// marked read with a timestamp watermark shows as read (legacy)).
        // trace:STORY-585
        #[clap(long)]
        unread: bool,

        /// Number of already-read messages to keep in the default newest-first
        /// inbox tail. Use 0 for unread-only default behavior.
        // trace:TASK-1211 | ai:codex
        #[clap(long, default_value_t = 5)]
        recent_read_tail: usize,
    },

    // trace:STORY-585
    /// Ambient unread-mail notice for an agent's context: a capped, plain,
    /// role+user-scoped summary of unread messages, or nothing when the inbox
    /// is caught up. Never marks anything seen. The SessionStart / per-turn
    /// hook calls this; reading/acking stays the explicit `aida mailbox inbox`.
    Notice {
        /// Narrow to one of this session's own inboxes. Default: all of them
        /// (its handle or user, the session role and the agent type).
        agent: Option<String>,

        /// Max messages to list before collapsing the rest into "+N more"
        /// (default: 5). Keeps the per-turn context injection bounded.
        #[clap(long)]
        cap: Option<usize>,
    },

    // trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude
    /// Advisor handshake: a launched non-advisor agent mails the advisor
    /// role once (handle, role, spec, path) and reports the ack, or `advisor
    /// unreachable: <reason>` when none arrives in time. For an advisor,
    /// lists the hellos waiting for an ack; reading them with `aida mailbox
    /// inbox` acks them
    Hello {
        /// Per-turn hook mode: send only on SessionStart, print only a new
        /// ack or an overdue hello, and stay silent otherwise
        #[clap(long)]
        hook: bool,
    },

    /// Operator overview: agents with mail waiting + unread / urgent-unread
    /// counts, most-recent-activity first.
    List {
        /// Dead-letter view: only recipients that have UNREAD mail but match no
        /// known role or registered agent — misaddressed handoffs (a typo'd
        /// agent name, a wrong role) that no live identity will ever read.
        // trace:BUG-679
        #[clap(long)]
        stranded: bool,
    },

    /// Read one message by id or unique id prefix without changing unread state.
    // trace:BUG-1297 | ai:codex
    Read {
        /// Message id or unique id prefix to read.
        message_id: String,
    },

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

    /// Archive read mailbox messages from the default inbox while preserving them for audit.
    // trace:TASK-1211 | ai:codex
    Archive {
        /// Message id to archive. Must already be read by the target inbox.
        message_id: Option<String>,

        /// Sweep read mail older than this point: a relative duration (`30d`,
        /// `12h`, `2w`, or `24 hours ago`), an ISO date (`2026-05-01`, local
        /// midnight), a zone-less ISO datetime (local time), or RFC3339.
        // trace:TASK-1509 | ai:claude
        #[clap(long, conflicts_with = "message_id")]
        older_than: Option<String>,

        /// Required with --older-than; documents that unread mail is never touched.
        #[clap(long, requires = "older_than")]
        read_only: bool,

        /// Whose read-watermark gates archive safety (default: current inbox identities).
        #[clap(long)]
        agent: Option<String>,
    },

    /// Maintenance alias for `archive --older-than ... --read-only`.
    // trace:TASK-1211 | ai:codex
    Gc {
        /// Sweep read mail older than this point (default: 30d). Same forms
        /// as `aida mail archive --older-than`.
        // trace:TASK-1509 | ai:claude
        #[clap(long, default_value = "30d")]
        older_than: String,

        /// Whose read-watermark gates archive safety (default: current inbox identities).
        #[clap(long)]
        agent: Option<String>,

        /// Instead of the age sweep, archive mail addressed to phantom
        /// recipients (flags, numbers, spec IDs, commit SHAs, unknown
        /// @mention handles) and re-address whitespace-padded recipients,
        /// printing each message it touches.
        // trace:TASK-1766 | ai:claude
        #[clap(long)]
        phantoms: bool,
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

/// `aida mailbox retention` subcommands.
// trace:STORY-1843 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RetentionCommand {
    /// Report whether every AIDA process on this machine that can write mail
    /// takes a write permit, and record that it does. A worktree whose
    /// mailbox holds mail is kept on removal or reset until that is recorded.
    /// Recording runs only for a human operator at an interactive terminal,
    /// and only when the report shows every running mail writer taking a
    /// permit.
    Activate {
        /// Report only; record nothing.
        #[clap(long)]
        dry_run: bool,
        /// Print the report as JSON.
        #[clap(long)]
        json: bool,
    },
    /// Drop worktrees that were deleted from disk but are still registered
    /// with Git. While registered, each one keeps the mail view partial,
    /// which also blocks acknowledging mail. A deleted worktree's mailbox
    /// went with it, so there is nothing left to retain.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-118 | ai:claude
    Prune {
        /// Also drop worktrees Git has locked. A lock normally marks a
        /// worktree on removable media; only a deleted worktree whose parent
        /// directory is still present is ever dropped.
        #[clap(long)]
        include_locked: bool,
        /// Report only; drop nothing.
        #[clap(long)]
        dry_run: bool,
        /// Print the report as JSON.
        #[clap(long)]
        json: bool,
    },
}
